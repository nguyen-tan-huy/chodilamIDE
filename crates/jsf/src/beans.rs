//! EL bean name → Java file index (port of jsf/bean_index.lua), webapp roots and the .xhtml
//! usage search (jsf/nav.lua's find_xhtml_usages).
use crate::el;
use project_model::Project;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// CDI @Named, JSF @ManagedBean, Spring @Component + stereotypes.
const ANNOTATIONS: &[(&str, bool)] = &[("Named", false), ("ManagedBean", false), ("Component", true), ("Service", true), ("Controller", true), ("Repository", true)];

#[derive(Debug, Clone, PartialEq)]
pub struct Bean {
    pub name: String,
    pub class_name: String,
    pub fqcn: String,
    pub file: PathBuf,
}

#[derive(Debug, Default, Clone)]
pub struct BeanIndex {
    /// name → every bean with that name (one per app of a multi-app repo)
    pub candidates: BTreeMap<String, Vec<Bean>>,
}

fn explicit_name(args: &str) -> Option<String> {
    let a = args.trim_start();
    if let Some(r) = a.strip_prefix('"') {
        return r.find('"').map(|e| r[..e].to_string());
    }
    for key in ["name", "value"] {
        let mut rest = a;
        while let Some(i) = rest.find(key) {
            let after = rest[i + key.len()..].trim_start();
            if let Some(v) = after.strip_prefix('=').map(str::trim_start).and_then(|v| v.strip_prefix('"')) {
                return v.find('"').map(|e| v[..e].to_string());
            }
            rest = &rest[i + key.len()..];
        }
    }
    None
}

/// The bean a Java file declares: only annotations BEFORE the class declaration count
/// (`@Inject @Named("x") Foo foo;` is an injection point).
pub fn parse_bean_file(text: &str, file: &Path) -> Option<Bean> {
    let mut pkg = None;
    let mut class = None;
    let mut header_end = 0;
    for line in text.lines() {
        let l = line.trim_start();
        if pkg.is_none() {
            if let Some(p) = l.strip_prefix("package ") {
                pkg = Some(p.trim().trim_end_matches(';').trim().to_string());
            }
        }
        header_end += line.len() + 1;
        if l.starts_with("//") || l.starts_with("/*") || l.starts_with('*') {
            continue;
        }
        if let Some(name) = class_name_in(line) {
            class = Some(name);
            break;
        }
    }
    let class_name = class?;
    let header = &text[..header_end.min(text.len())];
    // first bean annotation; with arguments when it has them
    let mut best: Option<(bool, Option<String>)> = None;
    let mut i = 0;
    while let Some(at) = header[i..].find('@') {
        let s = i + at + 1;
        let e = s + header[s..].find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(header.len() - s);
        let ann = &header[s..e];
        i = e;
        let Some(&(_, spring)) = ANNOTATIONS.iter().find(|(a, _)| *a == ann) else { continue };
        let rest = header[e..].trim_start();
        let args = rest.strip_prefix('(').and_then(|r| r.find(')').map(|c| r[..c].to_string()));
        best = Some((spring, args));
        break;
    }
    let (spring, args) = best?;
    let name = args.as_deref().and_then(explicit_name).filter(|n| !n.is_empty()).unwrap_or_else(|| el::decapitalize(&class_name, spring));
    let fqcn = pkg.map(|p| format!("{p}.{class_name}")).unwrap_or_else(|| class_name.clone());
    Some(Bean { name, class_name, fqcn, file: file.to_path_buf() })
}

fn class_name_in(line: &str) -> Option<String> {
    let mut rest = line;
    while let Some(i) = rest.find("class") {
        let before_ok = i == 0 || !rest.as_bytes()[i - 1].is_ascii_alphanumeric() && rest.as_bytes()[i - 1] != b'_';
        let after = &rest[i + 5..];
        if before_ok && after.starts_with(char::is_whitespace) {
            let name: String = after.trim_start().chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            if !name.is_empty() {
                return Some(name);
            }
        }
        rest = &rest[i + 5..];
    }
    None
}

fn walk(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name();
        let name = name.to_string_lossy();
        if p.is_dir() {
            if name != "target" && name != "node_modules" && !name.starts_with('.') {
                walk(&p, ext, out);
            }
        } else if p.extension().is_some_and(|x| x == ext) {
            out.push(p);
        }
    }
}

impl BeanIndex {
    /// Scans every module's main Java source roots.
    pub fn build(project: &Project) -> Self {
        let mut files = Vec::new();
        for m in &project.modules {
            for sr in m.source_roots.iter().filter(|s| s.kind == "main" && s.lang == "java") {
                walk(Path::new(&sr.path), "java", &mut files);
            }
        }
        let mut idx = BeanIndex::default();
        for f in files {
            let Ok(text) = std::fs::read_to_string(&f) else { continue };
            if !ANNOTATIONS.iter().any(|(a, _)| text.contains(&format!("@{a}"))) {
                continue;
            }
            if let Some(b) = parse_bean_file(&text, &f) {
                idx.candidates.entry(b.name.clone()).or_default().push(b);
            }
        }
        idx
    }

    /// Bean `name` as seen from `near` (the page): with several, the one sharing the longest
    /// path prefix with it.
    pub fn pick(&self, name: &str, near: &Path) -> Option<&Bean> {
        let c = self.candidates.get(name)?;
        let near = near.to_string_lossy();
        c.iter().max_by_key(|b| b.file.to_string_lossy().bytes().zip(near.bytes()).take_while(|(a, b)| a == b).count())
    }

    pub fn by_file(&self, file: &Path) -> Vec<&Bean> {
        self.candidates.values().flatten().filter(|b| b.file == file).collect()
    }

    pub fn beans(&self) -> impl Iterator<Item = &Bean> {
        self.candidates.values().filter_map(|v| v.first())
    }
}

/// Where a module's Facelets live: src/main/webapp (WAR) and META-INF/resources (JAR).
pub fn webapp_roots(project: &Project, near: Option<&Path>) -> Vec<PathBuf> {
    let mut mods: Vec<&project_model::Module> = project.modules.iter().collect();
    if let Some(n) = near {
        // the page's own module first
        mods.sort_by_key(|m| !n.starts_with(&m.content_root));
    }
    let mut out: Vec<PathBuf> = Vec::new();
    for m in mods {
        for rel in ["src/main/webapp", "src/main/resources/META-INF/resources"] {
            let d = Path::new(&m.content_root).join(rel);
            if d.is_dir() && !out.contains(&d) {
                out.push(d);
            }
        }
    }
    out
}

/// Every .xhtml under the webapp roots (every module's content root when there are none).
pub fn xhtml_files(project: &Project) -> Vec<PathBuf> {
    let mut dirs = webapp_roots(project, None);
    if dirs.is_empty() {
        dirs = project.modules.iter().map(|m| PathBuf::from(&m.content_root)).collect();
    }
    let mut out = Vec::new();
    for d in dirs {
        walk(&d, "xhtml", &mut out);
    }
    out.sort();
    out.dedup();
    out
}

/// Resolves an include/template path written in `from` (relative first, then each webapp root).
pub fn resolve_include(project: &Project, from: &Path, rel: &str) -> Option<PathBuf> {
    if rel.contains("#{") || rel.contains("${") {
        return None;
    }
    let path = rel.split(['?', '#']).next().unwrap_or(rel);
    if !path.starts_with('/') {
        let c = from.parent()?.join(path);
        if c.is_file() {
            return Some(c);
        }
    }
    webapp_roots(project, Some(from)).into_iter().map(|r| r.join(path.trim_start_matches('/'))).find(|c| c.is_file())
}

pub fn resolve_composite(project: &Project, from: &Path, lib: &str, tag: &str) -> Option<PathBuf> {
    webapp_roots(project, Some(from)).into_iter().map(|r| r.join("resources").join(lib).join(format!("{tag}.xhtml"))).find(|c| c.is_file())
}

/// One .xhtml usage: (file, 0-based line, byte col, line text).
pub type Usage = (PathBuf, usize, usize, String);

/// EL usages of `prop` on bean `bean` (`#{bean.prop}` / `#{bean['prop']}`), of the whole bean
/// (`prop` None), or of `prop` through any chain (`bean` None - members of non-bean classes).
/// By-name matches directly on one of `other_beans` (`#{otherBean.prop}`) are skipped: that
/// member belongs to the other bean's class, not to the class being searched.
pub fn find_usages(files: &[PathBuf], bean: Option<&str>, prop: Option<&str>, other_beans: &[String]) -> Vec<Usage> {
    let mut out = Vec::new();
    for f in files {
        let Ok(text) = std::fs::read_to_string(f) else { continue };
        for sp in el::el_spans(&text) {
            let body = &text[sp.body..sp.end];
            for (off, len) in matches_in(body, bean, prop) {
                if bean.is_none() && on_other_bean(body, off, other_beans) {
                    continue;
                }
                let at = sp.body + off;
                let line = text[..at].matches('\n').count();
                let ls = text[..at].rfind('\n').map(|i| i + 1).unwrap_or(0);
                let le = text[at..].find('\n').map(|i| at + i).unwrap_or(text.len());
                let _ = len;
                if !out.iter().any(|u: &Usage| u.0 == *f && u.1 == line) {
                    out.push((f.clone(), line, at - ls, text[ls..le].trim().to_string()));
                }
            }
        }
    }
    out
}

/// Is the by-name match at `off` a direct member of a bean head (`#{someBean.prop}`)?
fn on_other_bean(body: &str, off: usize, beans: &[String]) -> bool {
    let before = body[..off].trim_end();
    let before = before.strip_suffix('.').or_else(|| before.strip_suffix(['\'', '"']).and_then(|b| b.trim_end().strip_suffix('['))).unwrap_or(before).trim_end();
    let head_len = before.bytes().rev().take_while(|c| c.is_ascii_alphanumeric() || *c == b'_').count();
    if head_len == 0 {
        return false;
    }
    let head = &before[before.len() - head_len..];
    // a chain head: not itself after '.', ']' or ')'
    let prev = before[..before.len() - head_len].trim_end().bytes().last();
    !matches!(prev, Some(b'.' | b']' | b')')) && beans.iter().any(|b| b == head)
}

fn word_at(s: &str, i: usize, w: &str) -> bool {
    let b = s.as_bytes();
    s[i..].starts_with(w)
        && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_' || b[i - 1] == b'.'))
        && b.get(i + w.len()).is_none_or(|c| !(c.is_ascii_alphanumeric() || *c == b'_'))
}

/// (offset, len) of each match in one EL body.
fn matches_in(body: &str, bean: Option<&str>, prop: Option<&str>) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let member_after = |i: usize| -> Option<usize> {
        let p = prop?;
        let rest = &body[i..];
        let t = rest.trim_start();
        let skip = rest.len() - t.len();
        if let Some(r) = t.strip_prefix('.') {
            let r2 = r.trim_start();
            let at = i + skip + 1 + (r.len() - r2.len());
            return (r2.starts_with(p) && r2.as_bytes().get(p.len()).is_none_or(|c| !(c.is_ascii_alphanumeric() || *c == b'_'))).then_some(at);
        }
        if let Some(r) = t.strip_prefix('[') {
            let r2 = r.trim_start();
            let q = r2.chars().next().filter(|c| *c == '\'' || *c == '"')?;
            let inner = &r2[1..];
            return (inner.starts_with(p) && inner[p.len()..].starts_with(q)).then_some(i + skip + 1 + (r.len() - r2.len()) + 1);
        }
        None
    };
    match bean {
        Some(bn) => {
            let mut i = 0;
            while let Some(k) = body[i..].find(bn) {
                let at = i + k;
                i = at + bn.len();
                if !word_at(body, at, bn) {
                    continue;
                }
                match prop {
                    None => out.push((at, bn.len())),
                    Some(p) => {
                        if let Some(m) = member_after(at + bn.len()) {
                            out.push((m, p.len()));
                        }
                    }
                }
            }
        }
        None => {
            let p = prop.unwrap_or_default();
            let mut i = 0;
            while let Some(k) = body[i..].find(p) {
                let at = i + k;
                i = at + p.len().max(1);
                let before = body[..at].trim_end();
                let ok_end = body.as_bytes().get(at + p.len()).is_none_or(|c| !(c.is_ascii_alphanumeric() || *c == b'_'));
                let dot = before.ends_with('.') && before.len() > 1;
                let bracket = (before.ends_with('\'') || before.ends_with('"')) && before[..before.len() - 1].trim_end().ends_with('[');
                if ok_end && (dot || bracket) {
                    out.push((at, p.len()));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bean_files() {
        let src = "package com.x;\n\nimport javax.inject.Named;\n\n@Named\n@ViewScoped\npublic class HelloBean implements Serializable {\n  @Inject @Named(\"other\") Foo foo;\n}\n";
        let b = parse_bean_file(src, Path::new("/p/HelloBean.java")).unwrap();
        assert_eq!((b.name.as_str(), b.fqcn.as_str()), ("helloBean", "com.x.HelloBean"));
        let src = "package a;\n@Component(\"orderView\")\nclass OrderController {}";
        assert_eq!(parse_bean_file(src, Path::new("/p/O.java")).unwrap().name, "orderView");
        let src = "package a;\n@Service(value = \"svc\")\npublic class S {}";
        assert_eq!(parse_bean_file(src, Path::new("/p/S.java")).unwrap().name, "svc");
        let src = "package a;\n@Service\npublic class URLService {}";
        assert_eq!(parse_bean_file(src, Path::new("/p/U.java")).unwrap().name, "URLService");
        // annotation only on a field → not a bean
        let src = "package a;\n// class in a comment\npublic class Plain {\n @Named(\"x\") Foo f;\n}";
        assert!(parse_bean_file(src, Path::new("/p/P.java")).is_none());
    }

    #[test]
    fn usage_matching() {
        let body = "helloBean.customer.name ne helloBean['customer'] and otherhelloBean.customer";
        assert_eq!(matches_in(body, Some("helloBean"), Some("customer")).len(), 2);
        assert_eq!(matches_in(body, Some("helloBean"), None).len(), 2);
        assert_eq!(matches_in(body, None, Some("name")).len(), 1);
        assert!(matches_in("helloBean.customerId", Some("helloBean"), Some("customer")).is_empty());
    }
}

#[cfg(test)]
mod fixture_tests {
    use super::*;

    /// java-debug-model's sample project, when it is checked out next to this repo.
    #[test]
    fn sample_project_usages() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../java-debug-model/test-fixtures/sample-project/module-a");
        if !root.is_dir() {
            return;
        }
        let files = { let mut v = Vec::new(); walk(&root.join("src/main/webapp"), "xhtml", &mut v); v };
        let name = find_usages(&files, Some("helloBean"), Some("name"), &[]);
        // L9, L12, L18 (bracket) and L21 (name.bytes) of hello.xhtml
        let lines: Vec<usize> = name.iter().filter(|u| u.0.ends_with("hello.xhtml")).map(|u| u.1 + 1).collect();
        assert_eq!(lines, vec![9, 12, 18, 21]);
        let city = find_usages(&files, None, Some("city"), &["helloBean".to_string()]);
        // helloBean.name is the bean's own member: never a by-name hit of another class
        assert!(find_usages(&files, None, Some("name"), &["helloBean".to_string()]).iter().all(|u| !u.3.contains("helloBean.name}")));
        assert_eq!(city.iter().filter(|u| u.0.ends_with("hello.xhtml")).count(), 2);
    }
}
