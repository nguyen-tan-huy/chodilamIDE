//! Project discovery and the state files shared with wezterm.lua (the old shell):
//! MYAPP_PROJECT_ROOTS / MYAPP_PROJECTS / MYAPP_PROJECT_DEPTH (env or ~/.config/chodilamide/myapp.env),
//! ~/.local/state/chodilamide/{projects.json,recent.json}, and nvim's per-project session
//! files (~/.local/state/chodilamide-nvim/chodilamide-sessions/<root with / → %>.json).
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const APP: &str = "chodilamide";

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).unwrap_or_default())
}

fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".config"))
}

fn state_base() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".local/state"))
}

pub fn state_dir() -> PathBuf {
    state_base().join(APP)
}

/// `KEY` from the environment, else from myapp.env (last assignment wins, like wezterm.lua).
pub fn setting(key: &str) -> Option<String> {
    if let Ok(v) = std::env::var(key) {
        return Some(v);
    }
    let text = std::fs::read_to_string(config_dir().join(APP).join("myapp.env")).ok()?;
    parse_env(&text, key)
}

fn parse_env(text: &str, key: &str) -> Option<String> {
    text.lines()
        .filter_map(|l| l.trim().split_once('='))
        .filter(|(k, _)| k.trim() == key)
        .map(|(_, v)| v.trim().to_string())
        .last()
}

fn expand(p: &str) -> PathBuf {
    match p.strip_prefix('~') {
        Some(rest) => home().join(rest.trim_start_matches('/')),
        None => PathBuf::from(p),
    }
}

fn split_paths(s: &str) -> Vec<PathBuf> {
    let sep = if cfg!(windows) { ';' } else { ':' };
    s.split(sep).filter(|p| !p.is_empty()).map(expand).collect()
}

pub fn project_roots() -> Vec<PathBuf> {
    match setting("MYAPP_PROJECT_ROOTS") {
        Some(s) => split_paths(&s),
        None => ["code", "work", "src", "dev", "Projects", "projects", "Git-projects", "git", "repos", "workspace"].iter().map(|d| home().join(d)).collect(),
    }
}

/// Git repositories 1..depth levels below each root, plus MYAPP_PROJECTS and recent ones.
pub fn find_projects() -> Vec<PathBuf> {
    let depth: usize = setting("MYAPP_PROJECT_DEPTH").and_then(|d| d.parse().ok()).unwrap_or(2);
    let mut found: Vec<PathBuf> = Vec::new();
    for root in project_roots() {
        scan_git(&root, depth, &mut found);
    }
    found.extend(setting("MYAPP_PROJECTS").map(|s| split_paths(&s)).unwrap_or_default());
    found.extend(read_recent().into_iter().filter(|p| p.is_dir()));
    let mut seen = std::collections::HashSet::new();
    found.retain(|p| seen.insert(p.clone()));
    found
}

fn scan_git(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth == 0 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut subdirs: Vec<PathBuf> = rd.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| e.path()).collect();
    subdirs.sort();
    for d in subdirs {
        if d.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')) {
            continue;
        }
        if d.join(".git").exists() {
            out.push(d.clone());
        }
        scan_git(&d, depth - 1, out);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenProject {
    pub cwd: String,
    pub title: String,
}

pub fn read_open() -> Vec<OpenProject> {
    std::fs::read_to_string(state_dir().join("projects.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn write_open(projects: &[PathBuf]) {
    let list: Vec<OpenProject> = projects.iter().map(|p| OpenProject { cwd: p.display().to_string(), title: title(p) }).collect();
    if let Ok(bytes) = serde_json::to_vec(&list) {
        let _ = project_model::cache::write_atomic(&state_dir().join("projects.json"), &bytes);
    }
}

pub fn read_recent() -> Vec<PathBuf> {
    std::fs::read_to_string(state_dir().join("recent.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default()
        .into_iter()
        .map(PathBuf::from)
        .collect()
}

/// Moves `p` to the front of recent.json (newest first, like wezterm.lua).
pub fn touch_recent(p: &Path) {
    let mut list = read_recent();
    list.retain(|x| x != p);
    list.insert(0, p.to_path_buf());
    list.truncate(30);
    let strings: Vec<String> = list.iter().map(|p| p.display().to_string()).collect();
    if let Ok(bytes) = serde_json::to_vec(&strings) {
        let _ = project_model::cache::write_atomic(&state_dir().join("recent.json"), &bytes);
    }
}

pub fn title(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| p.display().to_string())
}

/// "~/x" for display.
pub fn short(p: &Path) -> String {
    let h = home();
    match p.strip_prefix(&h) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => p.display().to_string(),
    }
}

// ---- per-project editor session (same file nvim's chodilamide_session.lua uses) ----

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SessionBuffer {
    pub file: String,
    /// 1-based line, 0-based byte column (nvim cursor convention)
    pub line: usize,
    pub col: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Session {
    pub root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
    #[serde(default, deserialize_with = "list_or_empty")]
    pub buffers: Vec<SessionBuffer>,
}

fn list_or_empty<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<SessionBuffer>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(v).unwrap_or_default())
}

pub fn session_path(root: &Path) -> PathBuf {
    let nvim_app = setting("MYAPP_NVIM_APPNAME").unwrap_or_else(|| format!("{APP}-nvim"));
    let name: String = root.display().to_string().chars().map(|c| if matches!(c, '/' | '\\' | ':') { '%' } else { c }).collect();
    state_base().join(nvim_app).join("chodilamide-sessions").join(format!("{name}.json"))
}

pub fn read_session(root: &Path) -> Option<Session> {
    serde_json::from_str(&std::fs::read_to_string(session_path(root)).ok()?).ok()
}

pub fn write_session(s: &Session) {
    if let Ok(bytes) = serde_json::to_vec(s) {
        let _ = project_model::cache::write_atomic(&session_path(Path::new(&s.root)), &bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_file_and_session_name() {
        assert_eq!(parse_env("A=1\n MYAPP_PROJECT_DEPTH = 3 \nMYAPP_PROJECT_DEPTH=4", "MYAPP_PROJECT_DEPTH").as_deref(), Some("4"));
        assert_eq!(parse_env("X=1", "Y"), None);
        let p = session_path(Path::new("/home/u/Git-projects/product-service"));
        assert!(p.to_string_lossy().ends_with("chodilamide-sessions/%home%u%Git-projects%product-service.json"), "{}", p.display());
        let s: Session = serde_json::from_str(r#"{"root":"/p","current":"/p/A.java","buffers":[{"line":48,"col":0,"file":"/p/A.java"}]}"#).unwrap();
        assert_eq!(s.buffers[0].line, 48);
        let empty: Session = serde_json::from_str(r#"{"root":"/p","buffers":{}}"#).unwrap();
        assert!(empty.buffers.is_empty());
    }

    #[test]
    fn finds_git_repos_to_depth() {
        let root = std::env::temp_dir().join(format!("proj-test-{}", std::process::id()));
        for d in ["a/.git", "group/b/.git", "group/deep/c/d/.git", ".hidden/.git"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        let mut out = Vec::new();
        scan_git(&root, 2, &mut out);
        let names: Vec<String> = out.iter().map(|p| p.strip_prefix(&root).unwrap().display().to_string()).collect();
        assert_eq!(names, ["a", "group/b"]);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
