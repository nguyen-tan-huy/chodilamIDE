//! Watches the project folder so changes made outside the IDE (git pull, nvim, a terminal
//! command) reach jdtls, the project tree, open editors and the Maven model.
use crate::lsp_workspace::FileChange;
use crate::workspace::Workspace;
use futures::StreamExt;
use gpui::Context;
use notify::event::{ModifyKind, RenameMode};
use notify::{Event, EventKind, RecursiveMode, Watcher};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Build output, VCS and IDE state: never interesting and often huge.
const IGNORED_DIRS: &[&str] = &["target", ".git", "node_modules", ".idea", ".nvim", ".settings", "bin", "out"];

pub fn ignored(root: &Path, p: &Path) -> bool {
    let rel = p.strip_prefix(root).unwrap_or(p);
    if rel.components().any(|c| IGNORED_DIRS.contains(&c.as_os_str().to_string_lossy().as_ref())) {
        return true;
    }
    let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    // editor temp/backup files, our own atomic-save temp files
    name.ends_with('~') || name.ends_with(".swp") || name.ends_with(".swx") || name.ends_with(".tmp") || name.contains(".chodilamide-tmp") || name.starts_with(".#")
}

/// notify event → (path, change) list.
pub fn classify(ev: &Event) -> Vec<(PathBuf, FileChange)> {
    let kind = match ev.kind {
        EventKind::Create(_) => FileChange::Created,
        EventKind::Remove(_) => FileChange::Deleted,
        EventKind::Modify(ModifyKind::Name(RenameMode::From)) => FileChange::Deleted,
        EventKind::Modify(ModifyKind::Name(RenameMode::To)) => FileChange::Created,
        EventKind::Modify(ModifyKind::Name(RenameMode::Both)) if ev.paths.len() == 2 => {
            return vec![(ev.paths[0].clone(), FileChange::Deleted), (ev.paths[1].clone(), FileChange::Created)];
        }
        EventKind::Modify(ModifyKind::Metadata(_)) | EventKind::Access(_) => return vec![],
        EventKind::Modify(_) => FileChange::Changed,
        _ => return vec![],
    };
    ev.paths.iter().map(|p| (p.clone(), kind)).collect()
}

/// Merges one batch: last change per path wins, created-then-deleted disappears.
pub fn merge(events: Vec<(PathBuf, FileChange)>) -> Vec<(PathBuf, FileChange)> {
    let mut map: BTreeMap<PathBuf, FileChange> = BTreeMap::new();
    for (p, k) in events {
        let merged = match (map.get(&p), k) {
            (Some(FileChange::Created), FileChange::Deleted) => {
                map.remove(&p);
                continue;
            }
            (Some(FileChange::Created), FileChange::Changed) => FileChange::Created,
            _ => k,
        };
        map.insert(p, merged);
    }
    map.into_iter().collect()
}

impl Workspace {
    /// Starts the recursive watch; events are batched for 300 ms before being applied.
    pub(crate) fn start_watcher(&mut self, cx: &mut Context<Self>) {
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<Vec<(PathBuf, FileChange)>>();
        let root = self.root.clone();
        let root2 = root.clone();
        let watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            if let Ok(ev) = res {
                let changes: Vec<_> = classify(&ev).into_iter().filter(|(p, _)| !ignored(&root2, p)).collect();
                if !changes.is_empty() {
                    let _ = tx.unbounded_send(changes);
                }
            }
        });
        let mut watcher = match watcher {
            Ok(w) => w,
            Err(e) => {
                self.message = Some(format!("Không theo dõi được thư mục project: {e}"));
                return;
            }
        };
        if let Err(e) = watcher.watch(&root, RecursiveMode::Recursive) {
            self.message = Some(format!("Không theo dõi được thư mục project: {e} (tăng fs.inotify.max_user_watches?)"));
            return;
        }
        self.watcher = Some(watcher);
        cx.spawn(async move |this, cx| {
            while let Some(first) = rx.next().await {
                let mut batch = first;
                cx.background_executor().timer(Duration::from_millis(300)).await;
                while let Ok(more) = rx.try_recv() {
                    batch.extend(more);
                }
                if this.update(cx, |ws, cx| ws.on_disk_changes(merge(batch), cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn on_disk_changes(&mut self, mut changes: Vec<(PathBuf, FileChange)>, cx: &mut Context<Self>) {
        // the IDE's own saves were already reported (didSave / reload): telling jdtls again
        // made it rebuild twice, and a pom.xml re-imported Maven twice
        changes.retain(|(path, kind)| {
            if *kind == FileChange::Deleted {
                return true;
            }
            let Some(t) = self.tabs.iter().find(|t| t.read(cx).path() == Some(path)) else { return true };
            let Ok(disk) = std::fs::read_to_string(path) else { return true };
            t.read(cx).editor.buffer.text() != disk
        });
        if changes.is_empty() {
            return;
        }
        self.tree.refresh();
        self.refresh_git(cx);
        for (p, _) in &changes {
            self.jsf_invalidate(p);
        }
        // jdtls: the same notification the IDE's own file actions send
        self.notify_files(&changes);
        // open editors follow the disk when they have no unsaved edits
        let mut notes = Vec::new();
        for (path, kind) in &changes {
            let Some(t) = self.tabs.iter().find(|t| t.read(cx).path() == Some(path)).cloned() else { continue };
            match kind {
                FileChange::Deleted => notes.push(format!("{} đã bị xóa bên ngoài", crate::tree::file_name(path))),
                _ => {
                    let Ok(disk) = std::fs::read_to_string(path) else { continue };
                    let (same, dirty) = t.read_with(cx, |v, _| (v.editor.buffer.text() == disk, v.is_dirty()));
                    if same {
                        continue; // our own save
                    }
                    if dirty {
                        notes.push(format!("{} đổi bên ngoài nhưng đang có sửa chưa lưu - không nạp lại", crate::tree::file_name(path)));
                    } else {
                        t.update(cx, |v, cx| {
                            v.editor.buffer.reload_text(&disk);
                            let len = v.editor.buffer.len();
                            let head = v.editor.head.min(len);
                            v.editor.set_cursor(head, false);
                            cx.notify()
                        });
                        notes.push(format!("Đã nạp lại {} (đổi bên ngoài)", crate::tree::file_name(path)));
                    }
                }
            }
        }
        // a pom changed outside (git pull…): re-resolve the model (also re-imports in jdtls)
        let pom_changed = changes.iter().any(|(p, _)| p.file_name().is_some_and(|n| n == "pom.xml"));
        if pom_changed && !self.model_loading() {
            self.reload_model(cx);
        } else if let Some(n) = notes.first() {
            self.message = Some(if notes.len() > 1 { format!("{n} (+{} file khác)", notes.len() - 1) } else { n.clone() });
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignore_and_merge() {
        let root = Path::new("/p");
        assert!(ignored(root, Path::new("/p/core/target/classes/A.class")));
        assert!(ignored(root, Path::new("/p/.git/index")));
        assert!(ignored(root, Path::new("/p/core/src/.A.java.chodilamide-tmp")));
        assert!(!ignored(root, Path::new("/p/core/src/main/java/A.java")));
        assert!(!ignored(root, Path::new("/p/core/pom.xml")));
        let a = PathBuf::from("/p/A.java");
        let b = PathBuf::from("/p/B.java");
        let merged = merge(vec![(a.clone(), FileChange::Created), (a.clone(), FileChange::Changed), (b.clone(), FileChange::Created), (b.clone(), FileChange::Deleted)]);
        assert_eq!(merged, vec![(a, FileChange::Created)]);
    }

    /// A real inotify round trip: a file written in the project arrives as an event.
    #[test]
    fn watcher_sees_external_write() {
        let dir = std::env::temp_dir().join(format!("watch-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut w = notify::recommended_watcher(move |res: notify::Result<Event>| {
            if let Ok(ev) = res {
                for c in classify(&ev) {
                    let _ = tx.send(c);
                }
            }
        })
        .unwrap();
        w.watch(&dir, RecursiveMode::Recursive).unwrap();
        let f = dir.join("src/Moi.java");
        std::fs::write(&f, "class Moi {}").unwrap();
        let got = rx.recv_timeout(Duration::from_secs(5)).expect("no event");
        assert_eq!(got.0, f);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
