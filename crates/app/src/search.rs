//! Project file listing, fuzzy file matching and text search - UI-free.
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher};
use std::path::{Path, PathBuf};

const SKIP_DIRS: &[&str] = &["target", "node_modules", ".git", ".idea", ".nvim", "build", "out", "bin"];

/// Every file under `root`, relative paths, honoring .gitignore and skipping build output.
pub fn list_files(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(true)
        // .gitignore applies even when `root` itself is not a repository (independent modules)
        .require_git(false)
        .filter_entry(|e| !(e.file_type().is_some_and(|t| t.is_dir()) && SKIP_DIRS.contains(&e.file_name().to_string_lossy().as_ref())))
        .build();
    for e in walker.flatten() {
        if e.file_type().is_some_and(|t| t.is_file()) {
            if let Ok(rel) = e.path().strip_prefix(root) {
                out.push(rel.to_string_lossy().to_string());
            }
        }
    }
    out.sort();
    out
}

/// Best matches for `query` (fuzzy, path-aware, ignores Vietnamese diacritics in smart mode).
pub fn fuzzy_files(files: &[String], query: &str, limit: usize) -> Vec<String> {
    if query.trim().is_empty() {
        return files.iter().take(limit).cloned().collect();
    }
    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    let mut hits = pattern.match_list(files.iter(), &mut matcher);
    // tie-break: shorter paths first (the file itself before deep matches)
    hits.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.len().cmp(&b.0.len())));
    hits.into_iter().take(limit).map(|(s, _)| s.clone()).collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub path: PathBuf,
    /// 0-based line and byte column of the match
    pub line: usize,
    pub col: usize,
    pub text: String,
    /// byte range of the match inside `text` (drawn bold in lists)
    pub mark: Option<(usize, usize)>,
}

impl Hit {
    /// A hit whose `text` is `line` trimmed (≤200 chars) with the match `col..col+len` marked.
    pub fn snippet(path: PathBuf, line_no: usize, line: &str, col: usize, len: usize) -> Self {
        let lead = line.len() - line.trim_start().len();
        let text: String = line.trim().chars().take(200).collect();
        let mark = col.checked_sub(lead).map(|s| (s, (s + len).min(text.len()))).filter(|(s, e)| s < e && text.is_char_boundary(*s) && text.is_char_boundary(*e));
        Hit { path, line: line_no, col, text, mark }
    }
}

/// Case-insensitive (unless the query has capitals) substring search across the project.
pub fn search_text(root: &Path, files: &[String], query: &str, limit: usize) -> Vec<Hit> {
    let mut hits = Vec::new();
    if query.is_empty() {
        return hits;
    }
    let case_sensitive = query.chars().any(|c| c.is_uppercase());
    let needle = if case_sensitive { query.to_string() } else { query.to_lowercase() };
    for rel in files {
        let path = root.join(rel);
        let Ok(content) = std::fs::read_to_string(&path) else { continue }; // skips binaries
        for (line_no, line) in content.lines().enumerate() {
            let hay = if case_sensitive { line.to_string() } else { line.to_lowercase() };
            if let Some(col) = hay.find(&needle) {
                // lowercasing may shift byte offsets in rare scripts; clamp to a char boundary
                let mut col = col.min(line.len());
                while !line.is_char_boundary(col) {
                    col -= 1;
                }
                hits.push(Hit::snippet(path.clone(), line_no, line, col, needle.len()));
                if hits.len() >= limit {
                    return hits;
                }
            }
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_fuzzy_and_grep() {
        let root = std::env::temp_dir().join(format!("search-test-{}", std::process::id()));
        std::fs::create_dir_all(root.join("web/src/main/java/app")).unwrap();
        std::fs::create_dir_all(root.join("web/target/classes")).unwrap();
        std::fs::write(root.join("web/src/main/java/app/ProductWebApplication.java"), "class A {\n  // Xin chào Việt Nam\n}\n").unwrap();
        std::fs::write(root.join("web/pom.xml"), "<project/>").unwrap();
        std::fs::write(root.join("web/target/classes/Junk.class"), "x").unwrap();

        let files = list_files(&root);
        assert_eq!(files, ["web/pom.xml", "web/src/main/java/app/ProductWebApplication.java"]);
        assert_eq!(fuzzy_files(&files, "pwa", 5)[0], "web/src/main/java/app/ProductWebApplication.java");
        assert_eq!(fuzzy_files(&files, "pom", 5)[0], "web/pom.xml");

        let hits = search_text(&root, &files, "việt", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].line, 1);
        assert_eq!(hits[0].text, "// Xin chào Việt Nam");
        assert!(search_text(&root, &files, "Việt", 10).len() == 1);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
