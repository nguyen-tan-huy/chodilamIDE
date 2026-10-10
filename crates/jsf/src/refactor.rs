//! JSF refactorings (port of java-debug-model's jsf/refactor.lua): renaming a bean property
//! in the pages, the declarations that go with a property (field + accessors), the page map,
//! and the "◇ N xhtml" usage counts of a bean's members.
use crate::el::{self, capitalize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// `#{bean.old}` → `#{bean.new}` and `#{bean['old']}` → `#{bean['new']}` (whitespace allowed
/// around `.`/`[`), whole words only. Returns the new text and the number of replacements.
pub fn replace_el_refs(text: &str, bean: &str, old: &str, new: &str) -> (String, usize) {
    let t = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let (mut last, mut n, mut from) = (0, 0, 0);
    while let Some(i) = text[from..].find(bean).map(|k| from + k) {
        from = i + bean.len();
        if i > 0 && is_ident(t[i - 1]) || t.get(from).is_some_and(|&c| is_ident(c)) {
            continue;
        }
        let mut j = from;
        while t.get(j).is_some_and(|c| c.is_ascii_whitespace()) {
            j += 1;
        }
        let (start, quote) = match t.get(j) {
            Some(b'.') => {
                j += 1;
                while t.get(j).is_some_and(|c| c.is_ascii_whitespace()) {
                    j += 1;
                }
                (j, None)
            }
            Some(b'[') => {
                j += 1;
                while t.get(j).is_some_and(|c| c.is_ascii_whitespace()) {
                    j += 1;
                }
                match t.get(j) {
                    Some(&q @ (b'\'' | b'"')) => (j + 1, Some(q)),
                    _ => continue,
                }
            }
            _ => continue,
        };
        let end = start + old.len();
        if !text[start..].starts_with(old) {
            continue;
        }
        let ok = match quote {
            Some(q) => t.get(end) == Some(&q),
            None => !t.get(end).is_some_and(|&c| is_ident(c)),
        };
        if ok {
            out.push_str(&text[last..start]);
            out.push_str(new);
            last = end;
            n += 1;
            from = end;
        }
    }
    out.push_str(&text[last..]);
    (out, n)
}

/// The Java names that make up property `prop`: the field and its accessors.
pub fn property_members(prop: &str) -> Vec<String> {
    let cap = capitalize(prop);
    vec![prop.to_string(), format!("get{cap}"), format!("is{cap}"), format!("set{cap}")]
}

/// The new name of `member` when property `old` becomes `new` (getOld → getNew, old → new).
pub fn renamed_member(member: &str, old: &str, new: &str) -> String {
    if member == old {
        return new.to_string();
    }
    let cap_old = capitalize(old);
    match member.strip_suffix(&cap_old) {
        Some(prefix) => format!("{prefix}{}", capitalize(new)),
        None => new.to_string(),
    }
}

/// Declaration of one of `names` in a Java source: (0-based line, byte col of the name, name).
/// A declaration = a line starting with an access modifier with `name` followed by `(`, `;`
/// or `=`.
pub fn declarations(text: &str, names: &[String]) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let t = line.trim_start();
        if !(t.starts_with("public") || t.starts_with("private") || t.starts_with("protected")) {
            continue;
        }
        // the declared name is the identifier right before the first `(`, `=` or `;`
        let Some(d) = line.find(['(', '=', ';']) else { continue };
        let head = line[..d].trim_end();
        let start = head.len() - head.bytes().rev().take_while(|&c| is_ident(c)).count();
        let name = &head[start..];
        if names.iter().any(|n| n == name) {
            out.push((i, start, name.to_string()));
        }
    }
    out
}

/// Every `#{bean.member}` / `#{bean['member']}` in `text`: member → count.
pub fn member_usages(text: &str, bean: &str) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    let t = text.as_bytes();
    let mut from = 0;
    while let Some(i) = text[from..].find(bean).map(|k| from + k) {
        from = i + bean.len();
        if i > 0 && is_ident(t[i - 1]) || t.get(from).is_some_and(|&c| is_ident(c)) {
            continue;
        }
        let rest = text[from..].trim_start();
        let name = if let Some(r) = rest.strip_prefix('.') {
            r.trim_start().bytes().take_while(|&c| is_ident(c)).map(char::from).collect::<String>()
        } else if let Some(r) = rest.strip_prefix('[') {
            let r = r.trim_start();
            match r.chars().next() {
                Some(q @ ('\'' | '"')) => r[1..].split(q).next().unwrap_or("").to_string(),
                _ => String::new(),
            }
        } else {
            String::new()
        };
        if !name.is_empty() {
            *out.entry(name).or_insert(0) += 1;
        }
    }
    out
}

/// References one page makes: (kind, target) - include/template/link/component.
pub fn page_refs(text: &str) -> Vec<(&'static str, String)> {
    let mut refs = Vec::new();
    for tag in el::tags(text, text.len()) {
        let a = &tag.attrs;
        let r = if tag.name == "ui:include" {
            a.get("src").map(|v| ("include", v.value.clone()))
        } else if tag.name == "ui:composition" || tag.name == "ui:decorate" {
            a.get("template").map(|v| ("template", v.value.clone()))
        } else if let Some(o) = a.get("outcome").filter(|o| o.value.contains(".xhtml")) {
            Some(("link", o.value.clone()))
        } else {
            a.get("action").filter(|o| o.value.contains(".xhtml") && !o.value.contains("#{")).map(|v| ("link", v.value.clone()))
        };
        if let Some((k, target)) = r.filter(|(_, t)| !t.contains("#{") && !t.contains("${")) {
            refs.push((k, target.split('?').next().unwrap_or(&target).to_string()));
        }
    }
    for (prefix, lib) in el::composite_namespaces(text) {
        let open = format!("<{prefix}:");
        let mut from = 0;
        while let Some(i) = text[from..].find(&open).map(|k| from + k) {
            from = i + open.len();
            let tag: String = text[from..].chars().take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '-').collect();
            if !tag.is_empty() {
                refs.push(("component", format!("/resources/{lib}/{tag}.xhtml")));
            }
        }
    }
    refs.sort();
    refs.dedup();
    refs
}

/// Normalizes `a/b/../c` style paths (no filesystem access).
fn normalize(p: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    format!("/{}", parts.join("/"))
}

#[derive(Debug, Clone, PartialEq)]
pub struct PageEntry {
    /// webapp-relative, e.g. `/admin/users.xhtml`
    pub rel: String,
    pub file: PathBuf,
    /// (kind, target as written, resolved file when it exists)
    pub refs: Vec<(&'static str, String, Option<PathBuf>)>,
    /// referenced by some other page
    pub used: bool,
}

/// Page map of the webapp roots: what every page uses, and which pages nothing references.
pub fn page_map(roots: &[PathBuf], files: &[PathBuf]) -> Vec<PageEntry> {
    let rel_of = |f: &Path| -> String {
        roots.iter().find_map(|r| f.strip_prefix(r).ok()).map(|p| format!("/{}", p.display())).unwrap_or_else(|| f.display().to_string())
    };
    let mut pages: BTreeMap<String, PageEntry> = BTreeMap::new();
    for f in files {
        let text = std::fs::read_to_string(f).unwrap_or_default();
        let rel = rel_of(f);
        let dir = rel.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default();
        let refs = page_refs(&text).into_iter().map(|(k, t)| (k, t.clone(), Some(if t.starts_with('/') { normalize(&t) } else { normalize(&format!("{dir}/{t}")) }))).collect::<Vec<_>>();
        pages.insert(rel.clone(), PageEntry { rel, file: f.clone(), refs: refs.into_iter().map(|(k, t, r)| (k, t, r.map(PathBuf::from))).collect(), used: false });
    }
    let targets: Vec<String> = pages.values().flat_map(|p| p.refs.iter().filter_map(|(_, _, r)| r.as_ref().map(|r| r.display().to_string()))).collect();
    for t in &targets {
        if let Some(p) = pages.get_mut(t) {
            p.used = true;
        }
    }
    let files_by_rel: BTreeMap<String, PathBuf> = pages.iter().map(|(k, v)| (k.clone(), v.file.clone())).collect();
    for p in pages.values_mut() {
        for r in p.refs.iter_mut() {
            r.2 = r.2.as_ref().and_then(|rel| files_by_rel.get(&rel.display().to_string()).cloned());
        }
    }
    pages.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_refs_in_pages() {
        let page = r##"#{userBean.name} #{userBean . name} #{userBean['name']} #{userBean.names} #{otheruserBean.name} #{userBean.nameX()}"##;
        let (out, n) = replace_el_refs(page, "userBean", "name", "fullName");
        assert_eq!(n, 3);
        assert_eq!(out, r##"#{userBean.fullName} #{userBean . fullName} #{userBean['fullName']} #{userBean.names} #{otheruserBean.name} #{userBean.nameX()}"##);
        assert_eq!(renamed_member("getName", "name", "fullName"), "getFullName");
        assert_eq!(renamed_member("isActive", "active", "enabled"), "isEnabled");
        assert_eq!(renamed_member("name", "name", "fullName"), "fullName");
        let java = "class A {\n  private String name;\n  public String getName() { return name; }\n  public void setName(String name) { this.name = name; }\n}";
        let d = declarations(java, &property_members("name"));
        assert_eq!(d, [(1, 17, "name".to_string()), (2, 16, "getName".to_string()), (3, 14, "setName".to_string())]);
        let u = member_usages(r##"#{userBean.name} #{userBean.save()} #{userBean['name']}"##, "userBean");
        assert_eq!(u.get("name"), Some(&2));
        assert_eq!(u.get("save"), Some(&1));
    }

    #[test]
    fn refs_of_a_page() {
        let page = r##"<ui:composition template="/WEB-INF/layout.xhtml" xmlns:my="http://xmlns.jcp.org/jsf/composite/comp">
<ui:include src="parts/header.xhtml?x=1"/><my:card/><h:link outcome="list.xhtml"/><ui:include src="#{b.page}"/></ui:composition>"##;
        let r = page_refs(page);
        assert_eq!(
            r,
            [
                ("component", "/resources/comp/card.xhtml".to_string()),
                ("include", "parts/header.xhtml".to_string()),
                ("link", "list.xhtml".to_string()),
                ("template", "/WEB-INF/layout.xhtml".to_string()),
            ]
        );
        assert_eq!(normalize("/a/b/../c/./d.xhtml"), "/a/c/d.xhtml");
    }
}
