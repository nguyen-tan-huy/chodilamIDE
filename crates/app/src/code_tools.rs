//! Small IntelliJ tool windows: TODO (every TODO/FIXME comment), Bookmarks (F11 / Shift+F11)
//! and Local History (every save kept, compare / restore without git).
use crate::search::Hit;
use crate::workspace::Workspace;
use gpui::{Context, Window};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Lines of `text` holding a TODO / FIXME in a comment: (0-based line, byte col, the text).
pub fn todos(text: &str) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        for tag in ["TODO", "FIXME"] {
            let Some(col) = line.find(tag) else { continue };
            // inside a comment: // … TODO, /* … TODO, * TODO, # TODO, <!-- TODO
            let before = &line[..col];
            let in_comment = before.contains("//") || before.contains("/*") || before.trim_start().starts_with('*') || before.trim_start().starts_with('#') || before.contains("<!--");
            let word = !line[col + tag.len()..].starts_with(|c: char| c.is_alphanumeric());
            if in_comment && word {
                out.push((i, col, line[col..].trim_end_matches("*/").trim_end_matches("-->").trim().to_string()));
                break;
            }
        }
    }
    out
}

/// ~/.local/share/chodilamide/history/<hash of the path>/
fn history_dir(file: &Path) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    file.hash(&mut h);
    let base = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).unwrap_or_default()).join(".local/share"));
    base.join("chodilamide/history").join(format!("{:016x}", h.finish()))
}

const KEEP_VERSIONS: usize = 60;

/// Keeps a copy of `text` as the newest version of `file` (skipped when unchanged).
pub fn record_version(file: &Path, text: &str) {
    let dir = history_dir(file);
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let _ = std::fs::write(dir.join("path.txt"), file.display().to_string());
    let mut versions = list_versions(file);
    if versions.first().is_some_and(|(_, p)| std::fs::read_to_string(p).is_ok_and(|t| t == text)) {
        return;
    }
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let _ = std::fs::write(dir.join(format!("{ms}.v")), text);
    versions = list_versions(file);
    for (_, old) in versions.iter().skip(KEEP_VERSIONS) {
        let _ = std::fs::remove_file(old);
    }
}

/// Saved versions, newest first: (unix ms, file).
pub fn list_versions(file: &Path) -> Vec<(u128, PathBuf)> {
    let mut v: Vec<(u128, PathBuf)> = std::fs::read_dir(history_dir(file))
        .map(|rd| rd.flatten().filter_map(|e| Some((e.path().file_stem()?.to_str()?.parse().ok()?, e.path()))).collect())
        .unwrap_or_default();
    v.sort_by(|a, b| b.0.cmp(&a.0));
    v
}

/// The original file of a stored version (`…/history/<hash>/<ms>.v`).
pub fn version_origin(version: &Path) -> Option<PathBuf> {
    if version.extension().is_none_or(|e| e != "v") {
        return None;
    }
    std::fs::read_to_string(version.parent()?.join("path.txt")).ok().map(PathBuf::from)
}

fn local_time(ms: u128) -> String {
    // local offset from `date +%z` is overkill: show UTC-free relative age too
    let secs = (ms / 1000) as i64;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(secs);
    let age = now - secs;
    let rel = if age < 60 {
        format!("{age} giây trước")
    } else if age < 3600 {
        format!("{} phút trước", age / 60)
    } else if age < 86_400 {
        format!("{} giờ trước", age / 3600)
    } else {
        format!("{} ngày trước", age / 86_400)
    };
    rel
}

impl Workspace {
    fn bookmarks_file(&self) -> PathBuf {
        project_model::cache::state_dir(&self.root_str()).join("chodilamide-bookmarks.json")
    }

    pub(crate) fn load_bookmarks(&mut self) {
        let Ok(s) = std::fs::read_to_string(self.bookmarks_file()) else { return };
        let Ok(map) = serde_json::from_str::<BTreeMap<String, Vec<usize>>>(&s) else { return };
        self.bookmarks = map.into_iter().map(|(k, v)| (PathBuf::from(k), v.into_iter().collect())).collect();
    }

    fn save_bookmarks(&self) {
        let map: BTreeMap<String, Vec<usize>> = self.bookmarks.iter().filter(|(_, v)| !v.is_empty()).map(|(k, v)| (k.display().to_string(), v.iter().copied().collect())).collect();
        if let Ok(s) = serde_json::to_string_pretty(&map) {
            let _ = project_model::cache::write_atomic(&self.bookmarks_file(), s.as_bytes());
        }
    }

    /// F11 on the caret line.
    pub(crate) fn toggle_bookmark(&mut self, cx: &mut Context<Self>) {
        let Some(t) = self.active.and_then(|i| self.tabs.get(i)).cloned() else { return };
        let Some(path) = t.read(cx).path().cloned() else { return };
        let line = t.read(cx).editor.cursor_line_col().0;
        let set = self.bookmarks.entry(path.clone()).or_default();
        if !set.remove(&line) {
            set.insert(line);
        }
        let lines: Vec<usize> = set.iter().copied().collect();
        t.update(cx, |v, cx| {
            v.bookmarks = lines;
            cx.notify()
        });
        self.save_bookmarks();
    }

    /// Bookmarks of this file for its tab (called when a tab opens).
    pub(crate) fn tab_bookmarks(&self, path: &Path) -> Vec<usize> {
        self.bookmarks.get(path).map(|s| s.iter().copied().collect()).unwrap_or_default()
    }

    /// Shift+F11: every bookmark.
    pub(crate) fn show_bookmarks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut hits = Vec::new();
        for (path, lines) in &self.bookmarks {
            let text = std::fs::read_to_string(path).unwrap_or_default();
            let all: Vec<&str> = text.lines().collect();
            for &l in lines {
                hits.push(Hit::snippet(path.clone(), l, all.get(l).copied().unwrap_or(""), 0, 0));
            }
        }
        if hits.is_empty() {
            self.message = Some("Chưa có bookmark nào (F11 để đặt)".into());
            return cx.notify();
        }
        self.open_list(format!("Bookmarks ({})", hits.len()), hits, window, cx);
    }

    /// TODO tool window: TODO / FIXME comments of the whole project.
    pub(crate) fn show_todos(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        cx.spawn_in(window, async move |this, cx| {
            let hits = cx
                .background_executor()
                .spawn(async move {
                    let mut hits = Vec::new();
                    for rel in crate::search::list_files(&root) {
                        let path = root.join(&rel);
                        let Ok(text) = std::fs::read_to_string(&path) else { continue };
                        if !(text.contains("TODO") || text.contains("FIXME")) {
                            continue;
                        }
                        for (line, col, t) in todos(&text) {
                            let len = t.len();
                            let lt = text.lines().nth(line).unwrap_or("");
                            let mut h = Hit::snippet(path.clone(), line, lt, col, len.min(lt.len() - col));
                            h.text = t;
                            h.mark = Some((0, 4.min(h.text.len())));
                            hits.push(h);
                        }
                    }
                    hits
                })
                .await;
            let _ = this.update_in(cx, |ws, window, cx| {
                if hits.is_empty() {
                    ws.message = Some("Không có TODO / FIXME nào".into());
                    return cx.notify();
                }
                ws.open_list(format!("TODO ({})", hits.len()), hits, window, cx);
            });
        })
        .detach();
    }

    /// Local History of the active file: versions kept on every save.
    pub(crate) fn show_local_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.active.and_then(|i| self.tabs.get(i)).and_then(|t| t.read(cx).path().cloned()) else { return };
        let path = version_origin(&path).unwrap_or(path);
        let current = std::fs::read_to_string(&path).unwrap_or_default();
        let hits: Vec<Hit> = list_versions(&path)
            .into_iter()
            .map(|(ms, v)| {
                let old = std::fs::read_to_string(&v).unwrap_or_default();
                let changed: usize = vcs::diff_lines(&old, &current).iter().map(|h| h.old.len().max(h.new.len())).sum();
                let text = format!("{:<16} {} dòng khác bản hiện tại", local_time(ms), changed);
                Hit { path: v, line: 0, col: 0, mark: None, text }
            })
            .collect();
        if hits.is_empty() {
            self.message = Some("Chưa có lịch sử (được ghi mỗi lần lưu)".into());
            return cx.notify();
        }
        self.open_list(format!("Local History: {} ({})", crate::tree::file_name(&path), hits.len()), hits, window, cx);
    }

    /// On a Local History version tab: put that text back into the original file's tab.
    pub(crate) fn restore_version(&mut self, version: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(orig), Ok(text)) = (version_origin(&version), std::fs::read_to_string(&version)) else { return };
        self.open_file(orig.clone(), window, cx);
        if let Some(t) = self.active.and_then(|i| self.tabs.get(i)) {
            t.update(cx, |v, cx| {
                v.editor.buffer.break_group();
                v.editor.anchor = 0;
                v.editor.head = v.editor.buffer.len();
                v.editor.insert(&text);
                v.editor.buffer.break_group();
                v.editor.set_cursor(0, false);
                cx.notify()
            });
        }
        self.message = Some(format!("Đã khôi phục {} (chưa lưu - Ctrl+S, Ctrl+Z để bỏ)", crate::tree::file_name(&orig)));
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn todo_comments() {
        let src = "// TODO: fix\nString todo = \"TODO\";\n  /* FIXME later */\n# TODO yml\nint TODOS;\n";
        let t = todos(src);
        assert_eq!(t.iter().map(|x| (x.0, x.2.as_str())).collect::<Vec<_>>(), [(0, "TODO: fix"), (2, "FIXME later"), (3, "TODO yml")]);
    }

    #[test]
    fn versions_roundtrip() {
        let dir = std::env::temp_dir().join(format!("lh-test-{}", std::process::id()));
        // SAFETY: test-local data dir
        unsafe { std::env::set_var("XDG_DATA_HOME", &dir) };
        let f = PathBuf::from("/tmp/some/A.java");
        record_version(&f, "v1");
        record_version(&f, "v1"); // unchanged: not stored again
        std::thread::sleep(std::time::Duration::from_millis(5));
        record_version(&f, "v2");
        let v = list_versions(&f);
        assert_eq!(v.len(), 2);
        assert_eq!(std::fs::read_to_string(&v[0].1).unwrap(), "v2");
        assert_eq!(version_origin(&v[1].1), Some(f));
        let _ = std::fs::remove_dir_all(&dir);
        unsafe { std::env::remove_var("XDG_DATA_HOME") };
    }
}
