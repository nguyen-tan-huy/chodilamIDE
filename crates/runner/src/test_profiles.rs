//! `.nvim/java-debug-model/test-profiles.json` - the test entries java-debug-model shows
//! next to run configurations (same file, same fields; we add `class`/`method` so a profile
//! can be re-run exactly; nvim ignores unknown fields).
use anyhow::Result;
use project_model::cache::{state_dir, write_atomic};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::Path;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TestProfile {
    pub name: String,
    /// absolute path of the test file
    pub file: String,
    /// 1-based line inside the test method (nvim convention)
    #[serde(default)]
    pub lnum: usize,
    /// "nearest_method" or "class"
    #[serde(default = "default_scope")]
    pub scope: String,
    /// "run" or "debug" (how it was last started)
    #[serde(default)]
    pub variant: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn default_scope() -> String {
    "nearest_method".into()
}

fn path(root: &str) -> std::path::PathBuf {
    state_dir(root).join("test-profiles.json")
}

pub fn load(root: &str) -> Vec<TestProfile> {
    std::fs::read_to_string(path(root)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

/// Adds or replaces (by name).
pub fn upsert(root: &str, p: TestProfile) -> Result<()> {
    let mut all = load(root);
    match all.iter_mut().find(|x| x.name == p.name) {
        Some(x) => *x = p,
        None => all.push(p),
    }
    write_atomic(&path(root), &serde_json::to_vec(&all)?)
}

/// The Java method declared at or above 1-based `lnum` (for profiles written by nvim,
/// which store only a line inside the method).
pub fn method_near(text: &str, lnum: usize) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let start = lnum.saturating_sub(1).min(lines.len().saturating_sub(1));
    for l in (0..=start).rev() {
        let t = lines[l].trim();
        if t.starts_with("//") || t.starts_with('*') || t.starts_with('@') {
            continue;
        }
        // "void name(" / "public String name(" … but not calls or control statements
        if let Some(paren) = t.find('(') {
            let head = &t[..paren];
            let words: Vec<&str> = head.split_whitespace().collect();
            if words.len() >= 2 && !head.contains('=') && !head.contains('.') && !["if", "for", "while", "switch", "catch", "return", "new"].contains(&words[0]) {
                return words.last().map(|w| w.to_string());
            }
        }
    }
    None
}

/// `module/target/{classes|test-classes}/a/b/C.class` exists?
pub fn class_file_exists(module_dir: &Path, class: &str, test: bool) -> bool {
    let top = class.split('$').next().unwrap_or(class);
    let rel = format!("{}.class", top.replace('.', "/"));
    module_dir.join(if test { "target/test-classes" } else { "target/classes" }).join(rel).is_file()
}

/// The class file exists AND is not older than its source: a stale .class (the build skipped
/// test sources, or it ran before the save) must be recompiled before running.
pub fn class_file_fresh(module_dir: &Path, class: &str, test: bool, sourcepath: &[String]) -> bool {
    let top = class.split('$').next().unwrap_or(class);
    let rel = format!("{}.class", top.replace('.', "/"));
    let class_file = module_dir.join(if test { "target/test-classes" } else { "target/classes" }).join(rel);
    let mtime = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let Some(ct) = mtime(&class_file) else { return false };
    match crate::compile::source_file(sourcepath, class).and_then(|s| mtime(&s)) {
        Some(st) => ct >= st,
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_class_detected() {
        let m = std::env::temp_dir().join(format!("fresh-test-{}", std::process::id()));
        let src = m.join("src/test/java/a");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(m.join("target/test-classes/a")).unwrap();
        let roots = vec![m.join("src/test/java").display().to_string()];
        std::fs::write(src.join("T.java"), "class T {}").unwrap();
        assert!(!class_file_fresh(&m, "a.T", true, &roots), "no class yet");
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(m.join("target/test-classes/a/T.class"), "x").unwrap();
        assert!(class_file_fresh(&m, "a.T", true, &roots));
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(src.join("T.java"), "class T { int edited; }").unwrap();
        assert!(!class_file_fresh(&m, "a.T$Inner", true, &roots), "source edited after the build");
        std::fs::remove_dir_all(&m).unwrap();
    }

    #[test]
    fn nvim_profile_roundtrip_and_method_lookup() {
        let root = std::env::temp_dir().join(format!("tp-test-{}", std::process::id()));
        let r = root.display().to_string();
        std::fs::create_dir_all(state_dir(&r)).unwrap();
        // what java-debug-model writes
        std::fs::write(path(&r), r#"[{"variant":"debug","scope":"nearest_method","name":"GiaTest (nearest test)","file":"/p/GiaTest.java","lnum":13}]"#).unwrap();
        let mut all = load(&r);
        assert_eq!(all[0].lnum, 13);
        all[0].variant = "run".into();
        upsert(&r, all[0].clone()).unwrap();
        upsert(&r, TestProfile { name: "GiaTest.giamSai".into(), file: "/p/GiaTest.java".into(), lnum: 20, class: Some("vn.GiaTest".into()), method: Some("giamSai".into()), ..Default::default() }).unwrap();
        let again = load(&r);
        assert_eq!(again.len(), 2);
        assert_eq!(again[0].variant, "run");
        std::fs::remove_dir_all(&root).unwrap();

        let src = "class T {\n  @Test\n  void giamSai() throws Exception {\n    int x = f(1);\n    if (x > 0) {\n      assertEquals(1, x);\n    }\n  }\n}\n";
        assert_eq!(method_near(src, 6).as_deref(), Some("giamSai"));
        assert_eq!(method_near(src, 3).as_deref(), Some("giamSai"));
    }
}
