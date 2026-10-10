//! Spring Boot support without a Spring language server, like IntelliJ's Spring plugin:
//! application.properties / .yml keys (parse, completion metadata from the jars, navigation
//! to and from `@Value("${…}")` / `@ConfigurationProperties`), active profiles, HTTP endpoints
//! and beans. Pure text scanning - fast, works before jdtls is ready.
pub mod beans;
pub mod config;
pub mod endpoints;
pub mod metadata;

use project_model::Project;
use std::path::{Path, PathBuf};

/// Every file under `dir` with extension `ext` (skips target/, .git/, node_modules/).
pub(crate) fn walk(dir: &Path, exts: &[&str], out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name();
        let name = name.to_string_lossy();
        if p.is_dir() {
            if !name.starts_with('.') && name != "target" && name != "node_modules" {
                walk(&p, exts, out);
            }
        } else if p.extension().and_then(|x| x.to_str()).is_some_and(|x| exts.contains(&x)) {
            out.push(p);
        }
    }
}

/// Main Java sources of every module.
pub fn java_files(project: &Project) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for m in &project.modules {
        for sr in m.source_roots.iter().filter(|s| s.kind == "main" && s.lang == "java") {
            walk(Path::new(&sr.path), &["java"], &mut out);
        }
    }
    out.sort();
    out.dedup();
    out
}

/// 0-based line of byte offset `pos`.
pub(crate) fn line_of(text: &str, pos: usize) -> usize {
    text[..pos.min(text.len())].bytes().filter(|b| *b == b'\n').count()
}

/// Spring's relaxed binding: `my-prop`, `myProp`, `my_prop`, `MY_PROP` are the same key.
pub fn canonical(key: &str) -> String {
    key.chars().filter(|c| *c != '-' && *c != '_').flat_map(|c| c.to_lowercase()).collect()
}

/// `text` with `//` and `/* */` comments blanked (same length and newlines, so offsets and
/// lines stay valid); string and char literals are kept, so `"http://x"` survives.
pub fn strip_comments(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = b.to_vec();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'"' | b'\'' => {
                let q = b[i];
                // text block """ … """
                if q == b'"' && b[i..].starts_with(b"\"\"\"") {
                    i += 3;
                    while i < b.len() && !b[i..].starts_with(b"\"\"\"") {
                        i += 1;
                    }
                    i += 3;
                    continue;
                }
                i += 1;
                while i < b.len() && b[i] != q && b[i] != b'\n' {
                    if b[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
                i += 1;
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    out[i] = b' ';
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                while i < b.len() && !b[i..].starts_with(b"*/") {
                    if b[i] != b'\n' {
                        out[i] = b' ';
                    }
                    i += 1;
                }
                for _ in 0..2 {
                    if i < b.len() {
                        out[i] = b' ';
                        i += 1;
                    }
                }
            }
            _ => i += 1,
        }
    }
    // only ASCII bytes were replaced (by spaces), multi-byte chars inside comments are
    // replaced byte by byte - still valid UTF-8 since every byte became ' '
    String::from_utf8(out).unwrap_or_else(|_| text.to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn comments_blanked_strings_kept() {
        let s = "a // x\n/* y\n z */ \"http://h\" // Tiếng Việt\nb";
        let o = super::strip_comments(s);
        assert_eq!(o.len(), s.len());
        assert_eq!(o.lines().count(), s.lines().count());
        assert!(o.contains("\"http://h\"") && !o.contains('x') && !o.contains('y') && !o.contains("Việt") && o.ends_with('b'));
    }
}
