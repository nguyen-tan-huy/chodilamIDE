//! Effective-POM and classpath parsing. Deliberately mirrors the pattern-based
//! extractor in `resolver/maven.lua` (not a full XML parser) so both apps produce
//! the same model from the same `mvn` output.
use regex::Regex;
use std::sync::OnceLock;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedDep {
    pub group_id: Option<String>,
    pub artifact_id: Option<String>,
    pub version: Option<String>,
    pub scope: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedPom {
    pub group_id: Option<String>,
    pub artifact_id: Option<String>,
    pub version: Option<String>,
    pub packaging: String,
    pub source_dir: Option<String>,
    pub test_source_dir: Option<String>,
    pub modules: Vec<String>,
    pub dependencies: Vec<ParsedDep>,
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

/// Lua `xml:match("<tag>(.-)</tag>")`: first occurrence, non-greedy, raw.
fn first_block<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let s = xml.find(&open)? + open.len();
    let e = xml[s..].find(&close)? + s;
    Some(&xml[s..e])
}

fn extract_tag(xml: &str, tag: &str) -> Option<String> {
    first_block(xml, tag).map(|v| unescape(v.trim()))
}

/// Removes every `<tag>...</tag>` (non-greedy), like Lua `gsub`.
fn strip_blocks(xml: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
    loop {
        let Some(s) = rest.find(&open) else { break };
        let Some(e) = rest[s + open.len()..].find(&close) else { break };
        out.push_str(&rest[..s]);
        rest = &rest[s + open.len() + e + close.len()..];
    }
    out.push_str(rest);
    out
}

fn top_level_tag(project_xml: &str, tag: &str) -> Option<String> {
    let mut stripped = project_xml.to_string();
    for nested in ["parent", "dependencies", "build", "profiles", "properties", "dependencyManagement"] {
        stripped = strip_blocks(&stripped, nested);
    }
    extract_tag(&stripped, tag)
}

fn project_open() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"<project[ >]").unwrap())
}

/// Every top-level `<project>...</project>` block (one per reactor module).
pub fn split_projects(xml: &str) -> Vec<&str> {
    let re = project_open();
    let mut projects = Vec::new();
    let mut pos = 0;
    'outer: while let Some(m) = re.find_at(xml, pos) {
        let s = m.start();
        let Some(gt) = xml[s..].find('>') else { break };
        let mut cursor = s + gt + 1;
        let mut depth = 1;
        while depth > 0 {
            let open = re.find_at(xml, cursor);
            let Some(close_s) = xml[cursor..].find("</project>").map(|i| i + cursor) else {
                break 'outer;
            };
            match open {
                Some(o) if o.start() < close_s => {
                    depth += 1;
                    cursor = o.end();
                }
                _ => {
                    depth -= 1;
                    cursor = close_s + "</project>".len();
                    if depth == 0 {
                        projects.push(&xml[s..cursor]);
                        pos = cursor;
                    }
                }
            }
        }
    }
    if projects.is_empty() && re.is_match(xml) {
        projects.push(xml);
    }
    projects
}

pub fn parse_single(xml: &str) -> ParsedPom {
    let parent = first_block(xml, "parent");
    let group_id = top_level_tag(xml, "groupId").or_else(|| parent.and_then(|p| extract_tag(p, "groupId")));
    let artifact_id = top_level_tag(xml, "artifactId");
    let version = top_level_tag(xml, "version").or_else(|| parent.and_then(|p| extract_tag(p, "version")));
    let packaging = top_level_tag(xml, "packaging").unwrap_or_else(|| "jar".into());

    let build = first_block(xml, "build");
    let source_dir = build.and_then(|b| extract_tag(b, "sourceDirectory"));
    let test_source_dir = build.and_then(|b| extract_tag(b, "testSourceDirectory"));

    let mut modules = Vec::new();
    if let Some(block) = first_block(xml, "modules") {
        let mut rest = block;
        while let Some(m) = first_block(rest, "module") {
            modules.push(m.trim().to_string());
            let end = rest.find("</module>").unwrap() + "</module>".len();
            rest = &rest[end..];
        }
    }

    // Same as the Lua: the FIRST <dependencies> block, which in effective-pom output is
    // usually <dependencyManagement>'s. Kept identical on purpose - see ROADMAP-STATUS.md.
    let mut dependencies = Vec::new();
    if let Some(block) = first_block(xml, "dependencies") {
        let mut rest = block;
        while let Some(d) = first_block(rest, "dependency") {
            dependencies.push(ParsedDep {
                group_id: extract_tag(d, "groupId"),
                artifact_id: extract_tag(d, "artifactId"),
                version: extract_tag(d, "version"),
                scope: extract_tag(d, "scope").unwrap_or_else(|| "compile".into()),
            });
            let end = rest.find("</dependency>").unwrap() + "</dependency>".len();
            rest = &rest[end..];
        }
    }

    ParsedPom { group_id, artifact_id, version, packaging, source_dir, test_source_dir, modules, dependencies }
}

pub fn parse_effective_pom(xml: &str) -> Vec<ParsedPom> {
    split_projects(xml).into_iter().map(parse_single).collect()
}

/// `dependency:build-classpath` output → ordered jar paths.
pub fn parse_classpath(txt: &str) -> Vec<String> {
    // Maven writes the platform's path separator: ';' on Windows ("C:\…\a.jar;C:\…\b.jar",
    // where ':' is part of every path), ':' elsewhere
    parse_classpath_sep(txt, if cfg!(windows) { ';' } else { ':' })
}

pub fn parse_classpath_sep(txt: &str, sep: char) -> Vec<String> {
    let mut best = txt.trim();
    for line in txt.lines() {
        if line.contains(sep) && line.len() > best.len() {
            best = line;
        }
    }
    best.split(sep).map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
}

/// `.../<artifact>/<version>/<file>.jar` → (artifact, version).
pub fn jar_artifact_version(jar: &str) -> Option<(&str, &str)> {
    let file = jar.strip_suffix(".jar")?;
    let mut parts = file.rsplitn(4, ['/', '\\']);
    let _file = parts.next()?;
    let version = parts.next()?;
    let artifact = parts.next()?;
    parts.next()?;
    if artifact.is_empty() || version.is_empty() {
        return None;
    }
    Some((artifact, version))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classpath_windows_and_unix() {
        let win = r"C:\Users\a\.m2\repository\com\google\code\findbugs\jsr305\3.0.2\jsr305-3.0.2.jar;C:\Users\a\.m2\repository\org\slf4j\slf4j-api\1.7.36\slf4j-api-1.7.36.jar";
        let w = parse_classpath_sep(win, ';');
        assert_eq!(w.len(), 2);
        assert!(w[0].starts_with(r"C:\Users"));
        assert_eq!(jar_artifact_version(&w[1]), Some(("slf4j-api", "1.7.36")));
        let u = parse_classpath_sep("/h/.m2/a/b/1/b-1.jar:/h/.m2/c/d/2/d-2.jar\n", ':');
        assert_eq!(u, ["/h/.m2/a/b/1/b-1.jar", "/h/.m2/c/d/2/d-2.jar"]);
    }

    const REACTOR: &str = r#"[INFO] Scanning
<?xml version="1.0"?>
<projects>
<project xmlns="http://maven.apache.org/POM/4.0.0">
  <parent><groupId>g.parent</groupId><artifactId>root</artifactId><version>1.0</version></parent>
  <artifactId>core</artifactId>
  <dependencyManagement><dependencies>
    <dependency><groupId>x</groupId><artifactId>managed</artifactId><version>9</version></dependency>
  </dependencies></dependencyManagement>
  <dependencies>
    <dependency><groupId>a</groupId><artifactId>b</artifactId><version>1</version><scope>test</scope></dependency>
  </dependencies>
  <build><sourceDirectory>/p/core/src/main/java</sourceDirectory>
    <testSourceDirectory>/p/core/src/test/java</testSourceDirectory></build>
</project>
<project xmlns="http://maven.apache.org/POM/4.0.0">
  <groupId>g</groupId><artifactId>web</artifactId><version>2.0</version><packaging>war</packaging>
  <modules><module> sub </module></modules>
</project>
</projects>"#;

    #[test]
    fn splits_and_parses_reactor_output() {
        let ps = parse_effective_pom(REACTOR);
        assert_eq!(ps.len(), 2);
        let core = &ps[0];
        assert_eq!(core.group_id.as_deref(), Some("g.parent"));
        assert_eq!(core.artifact_id.as_deref(), Some("core"));
        assert_eq!(core.version.as_deref(), Some("1.0"));
        assert_eq!(core.packaging, "jar");
        assert_eq!(core.source_dir.as_deref(), Some("/p/core/src/main/java"));
        assert_eq!(core.test_source_dir.as_deref(), Some("/p/core/src/test/java"));
        // Lua parity: first <dependencies> block wins (the managed one).
        assert_eq!(core.dependencies[0].artifact_id.as_deref(), Some("managed"));
        assert_eq!(core.dependencies[0].scope, "compile");
        let web = &ps[1];
        assert_eq!(web.packaging, "war");
        assert_eq!(web.modules, vec!["sub".to_string()]);
    }

    #[test]
    fn classpath_and_jar_coords() {
        let cp = parse_classpath("noise\n/r/a/b/1.0/b-1.0.jar:/r/c/d/2/d-2.jar\n");
        assert_eq!(cp.len(), 2);
        assert_eq!(jar_artifact_version(&cp[0]), Some(("b", "1.0")));
        assert_eq!(parse_classpath(""), Vec::<String>::new());
    }
}
