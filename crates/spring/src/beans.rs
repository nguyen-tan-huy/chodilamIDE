//! Spring beans: stereotype classes (@Component, @Service…) and @Bean factory methods, with
//! the types they can be injected as - for the Beans list and "go to bean" from an
//! injection point.
use crate::endpoints::{balanced, target_after};
use crate::line_of;
use project_model::Project;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct Bean {
    pub name: String,
    /// Component, Service, Repository, Controller, RestController, Configuration, Bean…
    pub kind: String,
    /// class (stereotype) or return type (@Bean)
    pub ty: String,
    /// simple names it is assignable to: the type, its interfaces and superclass
    pub assignable: Vec<String>,
    pub file: PathBuf,
    pub line: usize,
}

const STEREOTYPES: &[&str] = &["Component", "Service", "Repository", "Controller", "RestController", "Configuration", "ControllerAdvice", "RestControllerAdvice", "Named", "SpringBootApplication"];

fn decapitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        // "URLHelper" stays as is (java.beans.Introspector)
        Some(f) if s.chars().nth(1).is_some_and(|n| n.is_uppercase()) => format!("{f}{}", c.as_str()),
        Some(f) => format!("{}{}", f.to_lowercase(), c.as_str()),
        None => String::new(),
    }
}

/// Simple name of a type expression: `java.util.List<Foo>` → `List`.
fn simple(ty: &str) -> String {
    let t = ty.split('<').next().unwrap_or(ty).trim();
    t.rsplit('.').next().unwrap_or(t).to_string()
}

/// Supertypes named in a class header: `class A extends B implements C, D<E>`.
fn supertypes(header: &str) -> Vec<String> {
    let mut out = Vec::new();
    for kw in ["extends", "implements"] {
        if let Some(i) = header.find(&format!(" {kw} ")) {
            let rest = &header[i + kw.len() + 2..];
            let rest = rest.split(" implements ").next().unwrap_or(rest);
            let mut depth = 0;
            let mut cur = String::new();
            for c in rest.chars() {
                match c {
                    '<' => depth += 1,
                    '>' => depth -= 1,
                    ',' if depth == 0 => out.push(simple(&std::mem::take(&mut cur))),
                    _ if depth == 0 => cur.push(c),
                    _ => {}
                }
            }
            if !cur.trim().is_empty() {
                out.push(simple(&cur));
            }
        }
    }
    out
}

fn string_arg(args: &str) -> Option<String> {
    let i = args.find('"')?;
    let j = args[i + 1..].find('"')?;
    Some(args[i + 1..i + 1 + j].to_string())
}

pub fn parse_beans(text: &str, file: &Path) -> Vec<Bean> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = text[from..].find('@').map(|n| from + n) {
        from = at + 1;
        let name_end = text[at + 1..].find(|c: char| !(c.is_alphanumeric() || c == '_')).map_or(text.len(), |n| at + 1 + n);
        let ann = &text[at + 1..name_end];
        let is_bean = ann == "Bean";
        if !is_bean && !STEREOTYPES.contains(&ann) {
            continue;
        }
        let after = text[name_end..].trim_start();
        let (args, end) = if after.starts_with('(') {
            let open = text.len() - after.len();
            match balanced(text, open) {
                Some(close) => (Some(&text[open + 1..close - 1]), close),
                None => continue,
            }
        } else {
            (None, name_end)
        };
        let explicit = args.and_then(string_arg);
        match target_after(text, end) {
            Some(Ok(class)) if !is_bean => {
                let decl_start = crate::endpoints::skip_annotations(text, end).unwrap_or(end);
                let header = text[decl_start..].split('{').next().unwrap_or("");
                let header = header.split_whitespace().collect::<Vec<_>>().join(" ");
                let mut assignable = vec![class.clone()];
                assignable.extend(supertypes(&header));
                out.push(Bean { name: explicit.unwrap_or_else(|| decapitalize(&class)), kind: ann.to_string(), ty: class, assignable, file: file.to_path_buf(), line: line_of(text, at) });
            }
            Some(Err(method)) if is_bean => {
                // return type = the word before the method name
                let decl_start = crate::endpoints::skip_annotations(text, end).unwrap_or(end);
                let decl = &text[decl_start..text[decl_start..].find('(').map_or(text.len(), |n| decl_start + n)];
                let words: Vec<&str> = decl.split_whitespace().collect();
                let ty = words.len().checked_sub(2).and_then(|i| words.get(i)).map(|t| simple(t)).unwrap_or_default();
                out.push(Bean { name: explicit.unwrap_or(method), kind: "Bean".into(), assignable: vec![ty.clone()], ty, file: file.to_path_buf(), line: line_of(text, at) });
            }
            _ => {}
        }
        from = end;
    }
    out
}

pub fn beans(project: &Project) -> Vec<Bean> {
    let mut out = Vec::new();
    for f in crate::java_files(project) {
        let Ok(text) = std::fs::read_to_string(&f) else { continue };
        if STEREOTYPES.iter().any(|s| text.contains(&format!("@{s}"))) || text.contains("@Bean") {
            out.extend(parse_beans(&crate::strip_comments(&text), &f));
        }
    }
    out.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.name.cmp(&b.name)));
    out
}

/// Beans that can be injected where `ty` (simple name) is expected.
pub fn candidates<'a>(all: &'a [Bean], ty: &str) -> Vec<&'a Bean> {
    let ty = simple(ty);
    all.iter().filter(|b| b.assignable.iter().any(|a| *a == ty)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereotypes_and_factory_methods() {
        let src = r#"
@Service("mailer")
public class SmtpMailService extends BaseService<Mail> implements MailService, Closeable {
}
@Configuration
class AppConfig {
    @Bean
    @Primary
    public RestTemplate restTemplate(RestTemplateBuilder b) { return b.build(); }

    @Bean(name = "clock")
    java.time.Clock systemClock() { return null; }
}"#;
        let b = parse_beans(src, Path::new("/x/A.java"));
        let got: Vec<String> = b.iter().map(|b| format!("{} {} {} {:?}", b.kind, b.name, b.ty, b.assignable)).collect();
        assert_eq!(
            got,
            [
                r#"Service mailer SmtpMailService ["SmtpMailService", "BaseService", "MailService", "Closeable"]"#,
                r#"Configuration appConfig AppConfig ["AppConfig"]"#,
                r#"Bean restTemplate RestTemplate ["RestTemplate"]"#,
                r#"Bean clock Clock ["Clock"]"#,
            ]
        );
        assert_eq!(candidates(&b, "MailService").len(), 1);
        assert_eq!(decapitalize("URLHelper"), "URLHelper");
    }
}
