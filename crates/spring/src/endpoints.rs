//! HTTP endpoints of Spring MVC / WebFlux controllers (IntelliJ's Endpoints tool window):
//! `@RequestMapping` on the class + `@Get/Post/Put/Delete/Patch/RequestMapping` on methods.
use crate::line_of;
use project_model::Project;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct Endpoint {
    /// GET, POST… or "*" for a @RequestMapping without method
    pub method: String,
    pub path: String,
    pub class: String,
    pub handler: String,
    pub file: PathBuf,
    /// 0-based line of the handler's mapping annotation
    pub line: usize,
}

const MAPPINGS: &[(&str, &str)] = &[
    ("GetMapping", "GET"),
    ("PostMapping", "POST"),
    ("PutMapping", "PUT"),
    ("DeleteMapping", "DELETE"),
    ("PatchMapping", "PATCH"),
    ("RequestMapping", ""),
];

/// `(…)` starting at `open` → index just past the matching `)` (strings respected).
pub(crate) fn balanced(text: &str, open: usize) -> Option<usize> {
    let b = text.as_bytes();
    let (mut depth, mut i, mut quote) = (0i32, open, None);
    while i < b.len() {
        match (quote, b[i]) {
            (Some(q), c) if c == q && b[i - 1] != b'\\' => quote = None,
            (Some(_), _) => {}
            (None, b'"' | b'\'') => quote = Some(b[i]),
            (None, b'(') => depth += 1,
            (None, b')') => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Top-level `a = x, b = {…}` split.
fn split_args(args: &str) -> Vec<&str> {
    let (mut out, mut depth, mut start, mut quote) = (Vec::new(), 0i32, 0, None);
    for (i, c) in args.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"') => quote = Some('"'),
            (None, '{' | '(') => depth += 1,
            (None, '}' | ')') => depth -= 1,
            (None, ',') if depth == 0 => {
                out.push(&args[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&args[start..]);
    out.into_iter().map(str::trim).filter(|s| !s.is_empty()).collect()
}

/// String literals of an expression (`"a"`, `{"a", "b"}`); a constant becomes `{NAME}`.
fn paths_of(expr: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = expr;
    while let Some(i) = rest.find('"') {
        let Some(j) = rest[i + 1..].find('"') else { break };
        out.push(rest[i + 1..i + 1 + j].to_string());
        rest = &rest[i + 2 + j..];
    }
    if out.is_empty() {
        let e = expr.trim().trim_matches(['{', '}']).trim();
        if !e.is_empty() {
            out.push(format!("{{{e}}}"));
        }
    }
    out
}

struct Mapping {
    paths: Vec<String>,
    methods: Vec<String>,
}

fn parse_mapping(verb: &str, args: Option<&str>) -> Mapping {
    let mut m = Mapping { paths: Vec::new(), methods: if verb.is_empty() { Vec::new() } else { vec![verb.to_string()] } };
    for a in split_args(args.unwrap_or("")) {
        match a.split_once('=').filter(|(k, _)| k.trim().chars().all(|c| c.is_alphanumeric())) {
            Some((k, v)) => match k.trim() {
                "value" | "path" => m.paths.extend(paths_of(v)),
                "method" => m.methods.extend(v.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '.').filter_map(|t| t.strip_prefix("RequestMethod.")).map(String::from)),
                _ => {}
            },
            None => m.paths.extend(paths_of(a)),
        }
    }
    if m.paths.is_empty() {
        m.paths.push(String::new());
    }
    m
}

/// Skips annotations (and `//` comments) from `i`; returns where the declaration starts.
pub(crate) fn skip_annotations(text: &str, mut i: usize) -> Option<usize> {
    let b = text.as_bytes();
    loop {
        while i < b.len() && (b[i] as char).is_whitespace() {
            i += 1;
        }
        if i < b.len() && b[i] == b'/' && b.get(i + 1) == Some(&b'/') {
            i = text[i..].find('\n').map_or(b.len(), |n| i + n);
            continue;
        }
        if i < b.len() && b[i] == b'@' && !text[i..].starts_with("@interface") {
            i += 1;
            while i < b.len() && ((b[i] as char).is_alphanumeric() || b[i] == b'.' || b[i] == b'_') {
                i += 1;
            }
            let mut j = i;
            while j < b.len() && (b[j] as char).is_whitespace() {
                j += 1;
            }
            if j < b.len() && b[j] == b'(' {
                i = balanced(text, j)?;
            }
            continue;
        }
        return Some(i);
    }
}

/// What an annotation ending at `i` is attached to: Ok(class name) or Err(method name).
pub(crate) fn target_after(text: &str, i: usize) -> Option<Result<String, String>> {
    let i = skip_annotations(text, i)?;
    let end = text[i..].find(['{', ';', '=']).map_or(text.len(), |n| i + n);
    let decl = &text[i..end];
    let words: Vec<&str> = decl.split(|c: char| !(c.is_alphanumeric() || c == '_')).filter(|w| !w.is_empty()).collect();
    if let Some(p) = words.iter().position(|w| matches!(*w, "class" | "interface" | "record" | "enum")) {
        return words.get(p + 1).map(|n| Ok(n.to_string()));
    }
    let paren = decl.find('(')?;
    let name = decl[..paren].trim_end().rsplit(|c: char| !(c.is_alphanumeric() || c == '_')).next()?;
    Some(Err(name.to_string()))
}

fn join(prefix: &str, path: &str) -> String {
    let joined = format!("/{}/{}", prefix.trim_matches('/'), path.trim_start_matches('/'));
    let mut out = String::new();
    for c in joined.chars() {
        if !(c == '/' && out.ends_with('/')) {
            out.push(c);
        }
    }
    if out.len() > 1 && out.ends_with('/') && !path.ends_with('/') {
        out.pop();
    }
    out
}

pub fn parse_controller(text: &str, file: &Path) -> Vec<Endpoint> {
    if !(text.contains("@RestController") || text.contains("@Controller")) || text.contains("@FeignClient") {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut class = String::new();
    let mut prefixes: Vec<String> = vec![String::new()];
    let mut from = 0;
    while let Some(at) = text[from..].find('@').map(|n| from + n) {
        from = at + 1;
        let name_end = text[at + 1..].find(|c: char| !(c.is_alphanumeric() || c == '_')).map_or(text.len(), |n| at + 1 + n);
        let name = &text[at + 1..name_end];
        let Some((_, verb)) = MAPPINGS.iter().find(|(a, _)| *a == name) else { continue };
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
        let m = parse_mapping(verb, args);
        match target_after(text, end) {
            Some(Ok(c)) => {
                class = c;
                prefixes = m.paths;
            }
            Some(Err(handler)) => {
                if class.is_empty() {
                    class = class_name(text).unwrap_or_default();
                }
                let methods = if m.methods.is_empty() { vec!["*".to_string()] } else { m.methods };
                for p in &prefixes {
                    for path in &m.paths {
                        for verb in &methods {
                            out.push(Endpoint { method: verb.clone(), path: join(p, path), class: class.clone(), handler: handler.clone(), file: file.to_path_buf(), line: line_of(text, at) });
                        }
                    }
                }
            }
            None => {}
        }
        from = end;
    }
    out
}

fn class_name(text: &str) -> Option<String> {
    let re = regex::Regex::new(r"\b(?:class|interface)\s+(\w+)").unwrap();
    re.captures(text).map(|c| c[1].to_string())
}

pub fn endpoints(project: &Project) -> Vec<Endpoint> {
    let mut out = Vec::new();
    for f in crate::java_files(project) {
        let Ok(text) = std::fs::read_to_string(&f) else { continue };
        out.extend(parse_controller(&crate::strip_comments(&text), &f));
    }
    out.sort_by(|a, b| a.path.cmp(&b.path).then(a.method.cmp(&b.method)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controller_mappings() {
        let src = r#"
@RestController
@RequestMapping(value = {"/api/v1/", "/v1"}, produces = "application/json")
public class OrderController {
    @GetMapping
    public List<Order> list() { return null; }

    @GetMapping("/{id}")
    public Order get(@PathVariable("id") String id) { return null; }

    @PostMapping(path = "/", consumes = "x")
    @ResponseStatus(HttpStatus.CREATED)
    public Order create(@RequestBody Order o) { return o; }

    @RequestMapping(value = "/sync", method = {RequestMethod.PUT, RequestMethod.PATCH})
    public void sync() {}

    @DeleteMapping(Paths.ONE)
    public void del() {}
}"#;
        let e = parse_controller(src, Path::new("/x/OrderController.java"));
        let got: Vec<String> = e.iter().map(|e| format!("{} {} {}", e.method, e.path, e.handler)).collect();
        assert_eq!(
            got,
            [
                "GET /api/v1 list",
                "GET /v1 list",
                "GET /api/v1/{id} get",
                "GET /v1/{id} get",
                "POST /api/v1/ create",
                "POST /v1/ create",
                "PUT /api/v1/sync sync",
                "PATCH /api/v1/sync sync",
                "PUT /v1/sync sync",
                "PATCH /v1/sync sync",
                "DELETE /api/v1/{Paths.ONE} del",
                "DELETE /v1/{Paths.ONE} del",
            ]
        );
        assert_eq!(e[0].class, "OrderController");
        assert_eq!(e[0].line, 4);
        assert!(parse_controller("@FeignClient(\"x\") interface A { @GetMapping(\"/a\") void a(); }", Path::new("a")).is_empty());
    }
}
