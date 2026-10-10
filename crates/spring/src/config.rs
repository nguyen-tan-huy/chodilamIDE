//! application.properties / application.yml: parsing into flat keys, the files of a project,
//! profiles, and `${…}` placeholders in Java.
use crate::{canonical, line_of, walk};
use project_model::Project;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// full dotted key, e.g. `spring.datasource.url` (list items as `a.b[0]`)
    pub key: String,
    /// None for a YAML section (`spring:` with children)
    pub value: Option<String>,
    /// 0-based line and byte column of the key's own (last) segment
    pub line: usize,
    pub col: usize,
    /// YAML indentation of the line (0 for properties)
    pub indent: usize,
}

pub fn parse_properties(text: &str) -> Vec<Entry> {
    let mut out = Vec::new();
    let mut continued = false;
    for (i, raw) in text.lines().enumerate() {
        let was_continued = continued;
        continued = raw.ends_with('\\') && !raw.ends_with("\\\\");
        if was_continued {
            continue;
        }
        let line = raw.trim_start();
        if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
            continue;
        }
        let col = raw.len() - line.len();
        let end = line.find(['=', ':']).or_else(|| line.find(char::is_whitespace)).unwrap_or(line.len());
        let key = line[..end].trim_end().to_string();
        if key.is_empty() {
            continue;
        }
        let value = line[end..].trim_start_matches(['=', ':']).trim().to_string();
        out.push(Entry { key, value: Some(value), line: i, col, indent: 0 });
    }
    out
}

/// Drops a trailing ` # comment` outside quotes.
fn strip_comment(s: &str) -> &str {
    let mut quote = None;
    let bytes = s.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        match (quote, b) {
            (Some(q), c) if c == q => quote = None,
            (None, b'\'' | b'"') => quote = Some(b),
            (None, b'#') if i == 0 || bytes[i - 1] == b' ' || bytes[i - 1] == b'\t' => return s[..i].trim_end(),
            _ => {}
        }
    }
    s.trim_end()
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2 && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\''))) {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// `key: value` → (key, value, byte offset of the value), None when not a mapping line.
fn split_mapping(s: &str) -> Option<(String, Option<String>)> {
    let mut quote = None;
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (None, '\'' | '"') if i == 0 => quote = Some(c),
            (None, ':') if s[i + 1..].is_empty() || s[i + 1..].starts_with([' ', '\t']) => {
                let key = unquote(&s[..i]);
                let v = s[i + 1..].trim();
                return Some((key, (!v.is_empty()).then(|| unquote(v))));
            }
            _ => {}
        }
    }
    None
}

/// Flattens a Spring YAML file; every document (`---`) is parsed into the same list.
pub fn parse_yaml(text: &str) -> Vec<Entry> {
    let mut out = Vec::new();
    // (indent of the key, full key path)
    let mut stack: Vec<(usize, String)> = Vec::new();
    // next list index per parent path
    let mut list_idx: std::collections::HashMap<String, usize> = Default::default();
    let mut block_scalar: Option<usize> = None;
    for (i, raw) in text.lines().enumerate() {
        let body = strip_comment(raw);
        let trimmed = body.trim_start();
        let indent = body.len() - trimmed.len();
        if let Some(bi) = block_scalar {
            if trimmed.is_empty() || indent > bi {
                continue;
            }
            block_scalar = None;
        }
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("---") || trimmed.starts_with("...") {
            stack.clear();
            list_idx.clear();
            continue;
        }
        while stack.last().is_some_and(|(ind, _)| *ind >= indent) {
            stack.pop();
        }
        let parent = stack.last().map(|(_, k)| k.clone()).unwrap_or_default();
        let (mut item_indent, mut rest, mut base) = (indent, trimmed, parent.clone());
        if let Some(r) = trimmed.strip_prefix("- ").or_else(|| (trimmed == "-").then_some("")) {
            let n = list_idx.entry(parent.clone()).or_insert(0);
            base = format!("{parent}[{n}]");
            *n += 1;
            rest = r.trim_start();
            item_indent = body.len() - rest.len();
            match split_mapping(rest) {
                Some(_) => {
                    // "- key: v": the item is a map; later "  key2: v" lines continue it
                    stack.push((indent, base.clone()));
                }
                None => {
                    out.push(Entry { key: base, value: (!rest.is_empty()).then(|| unquote(rest)), line: i, col: indent, indent });
                    continue;
                }
            }
        }
        let Some((k, v)) = split_mapping(rest) else { continue };
        let key = if base.is_empty() { k } else { format!("{base}.{k}") };
        let col = body.len() - rest.len();
        if matches!(v.as_deref(), Some("|" | ">" | "|-" | ">-" | "|+" | ">+")) {
            block_scalar = Some(item_indent);
        }
        out.push(Entry { key: key.clone(), value: v.clone(), line: i, col, indent: item_indent });
        if v.is_none() {
            stack.push((item_indent, key));
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Properties,
    Yaml,
}

pub fn kind_of(path: &Path) -> Option<Kind> {
    let name = path.file_name()?.to_str()?;
    let stem_ok = name.starts_with("application") || name.starts_with("bootstrap");
    if !stem_ok {
        return None;
    }
    match path.extension()?.to_str()? {
        "properties" => Some(Kind::Properties),
        "yml" | "yaml" => Some(Kind::Yaml),
        _ => None,
    }
}

pub fn parse(path: &Path, text: &str) -> Vec<Entry> {
    match kind_of(path) {
        Some(Kind::Yaml) => parse_yaml(text),
        _ => parse_properties(text),
    }
}

/// `application-dev.yml` → Some("dev")
pub fn profile_of(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    let p = stem.strip_prefix("application-").or_else(|| stem.strip_prefix("bootstrap-"))?;
    (!p.is_empty()).then(|| p.to_string())
}

/// application*/bootstrap* config files in every module's resources (main first, then test).
pub fn config_files(project: &Project) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for m in &project.modules {
        for rel in ["src/main/resources", "src/test/resources"] {
            let dir = Path::new(&m.content_root).join(rel);
            let mut found = Vec::new();
            walk(&dir, &["properties", "yml", "yaml"], &mut found);
            found.retain(|p| kind_of(p).is_some());
            found.sort();
            out.extend(found);
        }
    }
    out.dedup();
    out
}

/// Profiles a project defines: `application-<p>.*` files and YAML documents activated by
/// `spring.config.activate.on-profile` / `spring.profiles`.
pub fn profiles(project: &Project) -> Vec<String> {
    let mut set = BTreeSet::new();
    for f in config_files(project) {
        if let Some(p) = profile_of(&f) {
            set.insert(p);
        }
        if kind_of(&f) == Some(Kind::Yaml) {
            let Ok(text) = std::fs::read_to_string(&f) else { continue };
            for e in parse_yaml(&text) {
                if matches!(e.key.as_str(), "spring.config.activate.on-profile" | "spring.profiles") {
                    for p in e.value.unwrap_or_default().split([',', '|', '&', '!', ' ']).filter(|p| !p.is_empty()) {
                        set.insert(p.to_string());
                    }
                }
            }
        }
    }
    set.into_iter().collect()
}

/// Key at the cursor in a config file: the entry on that line.
pub fn entry_at(entries: &[Entry], line: usize) -> Option<&Entry> {
    entries.iter().find(|e| e.line == line)
}

/// YAML: the dotted path new keys typed on `line` (with that indentation) would live under.
pub fn yaml_parent(entries: &[Entry], line: usize, indent: usize) -> String {
    entries
        .iter()
        .filter(|e| e.line < line && e.indent < indent && e.value.is_none())
        .max_by_key(|e| e.line)
        .map(|e| {
            // every section between it and the root must also enclose `line`; the most recent
            // shallower section is the parent
            e.key.clone()
        })
        .unwrap_or_default()
}

/// `${key:default}` around byte `pos` → (key, start, end) of the key text.
pub fn placeholder_at(text: &str, pos: usize) -> Option<(String, usize, usize)> {
    let line_start = text[..pos.min(text.len())].rfind('\n').map_or(0, |i| i + 1);
    let before = &text[line_start..pos.min(text.len())];
    let open = before.rfind("${")?;
    if before[open..].contains('}') {
        return None;
    }
    let start = line_start + open + 2;
    let rest = &text[start..];
    let len = rest.find(|c: char| c == ':' || c == '}' || c == '"' || c == '\n').unwrap_or(rest.len());
    if pos > start + len {
        return None; // in the default value
    }
    Some((rest[..len].to_string(), start, start + len))
}

/// Every `${key…}` in `text` → (key, line, byte col).
pub fn placeholders(text: &str) -> Vec<(String, usize, usize)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = text[from..].find("${") {
        let start = from + i + 2;
        let rest = &text[start..];
        let len = rest.find(|c: char| c == ':' || c == '}' || c == '"' || c == '\n').unwrap_or(rest.len());
        if len > 0 {
            let line = line_of(text, start);
            let col = start - text[..start].rfind('\n').map_or(0, |n| n + 1);
            out.push((rest[..len].to_string(), line, col));
        }
        from = start;
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub struct Location {
    pub path: PathBuf,
    pub line: usize,
    pub col: usize,
    /// line text for lists
    pub text: String,
}

fn loc(path: &Path, text: &str, line: usize, col: usize) -> Location {
    Location { path: path.to_path_buf(), line, col, text: text.lines().nth(line).unwrap_or("").trim().to_string() }
}

/// Where `key` is set: entries in every config file (relaxed binding; a section matches too).
pub fn definitions(project: &Project, key: &str) -> Vec<Location> {
    let want = canonical(key);
    let mut out = Vec::new();
    for f in config_files(project) {
        let Ok(text) = std::fs::read_to_string(&f) else { continue };
        for e in parse(&f, &text) {
            if canonical(&e.key) == want {
                out.push(loc(&f, &text, e.line, e.col));
            }
        }
    }
    out
}

/// Java code reading `key`: `${key}` placeholders, and `@ConfigurationProperties(prefix)`
/// classes whose prefix covers it (plus the matching field when there is one).
pub fn java_usages(project: &Project, key: &str) -> Vec<Location> {
    let want = canonical(key);
    let mut out = Vec::new();
    let re = regex::Regex::new(r#"@ConfigurationProperties\s*\(\s*(?:(?:value|prefix)\s*=\s*)?"([^"]+)""#).unwrap();
    for f in crate::java_files(project) {
        let Ok(text) = std::fs::read_to_string(&f) else { continue };
        if text.contains("${") {
            for (k, line, col) in placeholders(&text) {
                if canonical(&k) == want {
                    out.push(loc(&f, &text, line, col));
                }
            }
        }
        for cap in re.captures_iter(&text) {
            let prefix = canonical(&cap[1]);
            let m = cap.get(1).unwrap();
            let Some(rest) = want.strip_prefix(&prefix) else { continue };
            if !(rest.is_empty() || rest.starts_with('.')) {
                continue;
            }
            // the field for the next segment (`app.mail.host` under prefix `app.mail` → `host`)
            let field = rest.trim_start_matches('.').split(['.', '[']).next().unwrap_or("");
            let field_hit = (!field.is_empty())
                .then(|| {
                    text.lines().enumerate().skip(line_of(&text, m.start())).find_map(|(i, l)| {
                        let t = l.trim_start();
                        let decl = t.starts_with("private ") || t.starts_with("protected ") || t.starts_with("public ");
                        if !decl || t.contains('(') {
                            return None;
                        }
                        let name = t.trim_end_matches(';').split(['=', ';']).next()?.split_whitespace().last()?;
                        (canonical(name) == field).then(|| (i, l.find(name).unwrap_or(0)))
                    })
                })
                .flatten();
            match field_hit {
                Some((line, col)) => out.push(loc(&f, &text, line, col)),
                None => {
                    let line = line_of(&text, m.start());
                    out.push(loc(&f, &text, line, m.start() - text[..m.start()].rfind('\n').map_or(0, |n| n + 1)));
                }
            }
        }
    }
    out
}

/// Every key set in the project's config files (for completion next to the jar metadata).
pub fn project_keys(project: &Project) -> Vec<(String, Option<String>)> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for f in config_files(project) {
        let Ok(text) = std::fs::read_to_string(&f) else { continue };
        for e in parse(&f, &text).into_iter().filter(|e| e.value.is_some() && !e.key.contains('[')) {
            if seen.insert(e.key.clone()) {
                out.push((e.key, e.value));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn properties_and_yaml_flatten() {
        let p = parse_properties("# c\nserver.port=8080\nspring.datasource.url : jdbc:x\n  app.name  hello\nlong=a\\\n  b\n");
        let keys: Vec<_> = p.iter().map(|e| (e.key.as_str(), e.value.as_deref().unwrap_or(""), e.line)).collect();
        assert_eq!(keys, [("server.port", "8080", 1), ("spring.datasource.url", "jdbc:x", 2), ("app.name", "hello", 3), ("long", "a\\", 4)]);

        let y = "server:\n  port: 8080 # comment\nspring:\n  datasource:\n    url: \"jdbc:x#1\"\n  profiles:\n    active: dev\napp:\n  hosts:\n    - a\n    - b\n  users:\n    - name: x\n      role: admin\n  text: |\n    key: not a key\n  after: 1\n---\nspring.config.activate.on-profile: prod\nserver.port: 80\n";
        let e = parse_yaml(y);
        let keys: Vec<_> = e.iter().filter(|e| e.value.is_some()).map(|e| (e.key.as_str(), e.value.as_deref().unwrap())).collect();
        assert_eq!(
            keys,
            [
                ("server.port", "8080"),
                ("spring.datasource.url", "jdbc:x#1"),
                ("spring.profiles.active", "dev"),
                ("app.hosts[0]", "a"),
                ("app.hosts[1]", "b"),
                ("app.users[0].name", "x"),
                ("app.users[0].role", "admin"),
                ("app.text", "|"),
                ("app.after", "1"),
                ("spring.config.activate.on-profile", "prod"),
                ("server.port", "80"),
            ]
        );
        let port = e.iter().find(|e| e.key == "server.port").unwrap();
        assert_eq!((port.line, port.col), (1, 2));
        // typing on a new line indented under "datasource:" (line 3)
        assert_eq!(yaml_parent(&e, 5, 4), "spring.datasource");
        assert_eq!(yaml_parent(&e, 5, 2), "spring");
        assert_eq!(yaml_parent(&e, 5, 0), "");
    }

    #[test]
    fn placeholders_and_relaxed_names() {
        let java = "@Value(\"${app.mail-host:localhost}\")\nString h;\n@Value(\"${server.port}\") int p;";
        assert_eq!(placeholders(java), [("app.mail-host".to_string(), 0, 10), ("server.port".to_string(), 2, 10)]);
        let at = java.find("mail").unwrap();
        assert_eq!(placeholder_at(java, at).map(|p| p.0), Some("app.mail-host".into()));
        assert_eq!(placeholder_at(java, java.find("localhost").unwrap()), None);
        assert_eq!(canonical("app.mail-host"), canonical("app.mailHost"));
        assert_eq!(profile_of(Path::new("/r/application-dev.yml")).as_deref(), Some("dev"));
        assert_eq!(profile_of(Path::new("/r/application.yml")), None);
        assert!(kind_of(Path::new("/r/pom.xml")).is_none());
    }
}
