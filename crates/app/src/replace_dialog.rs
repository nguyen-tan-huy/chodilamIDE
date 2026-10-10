//! Replace in Files (Ctrl+Shift+R): literal, case-sensitive text across the project, with a
//! preview count first. Open tabs are edited in their buffer (undoable), other files on disk.
use crate::dialogs::{self, DialogCancel, DialogConfirm, DialogNext, DialogPrev, Field};
use crate::theme::{self, c};
use crate::workspace::Workspace;
use gpui::{AnyElement, Context, Focusable, SharedString, Window, div, prelude::*};
use std::path::PathBuf;

pub struct ReplaceInFiles {
    find: Field,
    replace: Field,
    /// (files, occurrences) for the text last previewed
    preview: Option<(String, usize, usize)>,
    error: Option<String>,
}

/// Every occurrence of `find` replaced by `rep` (literal, case-sensitive).
pub fn replace_literal(text: &str, find: &str, rep: &str) -> (String, usize) {
    if find.is_empty() {
        return (text.to_string(), 0);
    }
    let n = text.matches(find).count();
    (if n > 0 { text.replace(find, rep) } else { text.to_string() }, n)
}

impl Workspace {
    pub(crate) fn open_replace_in_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // the selection of the active tab is the default search text, like IntelliJ
        let initial = self
            .active
            .and_then(|i| self.tabs.get(i))
            .map(|t| {
                let v = t.read(cx);
                v.editor.buffer.slice(v.editor.selection())
            })
            .filter(|s| !s.is_empty() && !s.contains('\n'))
            .unwrap_or_default();
        let r = ReplaceInFiles {
            find: Field::new("Tìm", "chữ cần thay (phân biệt hoa/thường)", &initial, cx),
            replace: Field::new("Thay bằng", "", "", cx),
            preview: None,
            error: None,
        };
        dialogs::focus_later(r.find.input.read(cx).focus_handle(cx), window, cx);
        self.replace_files = Some(r);
        cx.notify();
    }

    fn close_replace_in_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_files = None;
        if let Some(i) = self.active {
            self.activate(i, window, cx);
        }
        cx.notify();
    }

    /// Text of `path`: the open tab's buffer, else the file.
    fn current_text(&self, path: &std::path::Path, cx: &Context<Self>) -> Option<String> {
        match self.tabs.iter().find(|t| t.read(cx).path().map(|p| p.as_path()) == Some(path)) {
            Some(t) => Some(t.read(cx).editor.buffer.text()),
            None => std::fs::read_to_string(path).ok(),
        }
    }

    fn replace_targets(&self, find: &str, cx: &Context<Self>) -> Vec<(PathBuf, usize)> {
        let files = crate::search::list_files(&self.root);
        files
            .iter()
            .map(|rel| self.root.join(rel))
            .filter_map(|p| {
                let n = self.current_text(&p, cx)?.matches(find).count();
                (n > 0).then_some((p, n))
            })
            .collect()
    }

    /// First Enter previews the count; the second (same text) replaces.
    fn confirm_replace_in_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.replace_files.as_ref() else { return };
        let (find, rep) = (r.find.text(cx), r.replace.text(cx));
        if find.is_empty() {
            if let Some(r) = self.replace_files.as_mut() {
                r.error = Some("Nhập chữ cần tìm".into());
            }
            return cx.notify();
        }
        let previewed = r.preview.as_ref().is_some_and(|(q, _, _)| *q == find);
        let targets = self.replace_targets(&find, cx);
        if !previewed {
            let total = targets.iter().map(|(_, n)| n).sum();
            if let Some(r) = self.replace_files.as_mut() {
                r.preview = Some((find, targets.len(), total));
                r.error = None;
            }
            return cx.notify();
        }
        let (mut files, mut total, mut changed) = (0, 0, Vec::new());
        for (path, _) in targets {
            let open = self.tabs.iter().find(|t| t.read(cx).path() == Some(&path)).cloned();
            let Some(text) = self.current_text(&path, cx) else { continue };
            let (new, n) = replace_literal(&text, &find, &rep);
            if n == 0 {
                continue;
            }
            match open {
                Some(t) => t.update(cx, |v, cx| {
                    v.editor.buffer.break_group();
                    v.editor.anchor = 0;
                    v.editor.head = v.editor.buffer.len();
                    v.editor.insert(&new);
                    v.editor.buffer.break_group();
                    v.editor.set_cursor(0, false);
                    cx.notify()
                }),
                None => {
                    if std::fs::write(&path, new).is_err() {
                        continue;
                    }
                    changed.push((path.clone(), crate::lsp_workspace::FileChange::Changed));
                }
            }
            files += 1;
            total += n;
        }
        self.notify_files(&changed);
        self.message = Some(format!("Đã thay {total} chỗ trong {files} file (tab đang mở chưa lưu - Ctrl+S)"));
        self.close_replace_in_files(window, cx);
    }

    pub(crate) fn render_replace_in_files(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let r = self.replace_files.as_ref()?;
        let status = match (&r.error, &r.preview) {
            (Some(e), _) => div().text_color(c(theme::ERROR)).child(SharedString::from(e.clone())),
            (None, Some((q, f, n))) if *q == r.find.text(cx) => div().text_color(c(theme::FG)).child(SharedString::from(format!("{n} chỗ trong {f} file - Enter lần nữa để thay tất cả"))),
            _ => div().text_color(c(theme::FG_DIM)).child("Enter: xem số chỗ sẽ thay"),
        };
        let body = div().flex().flex_col().gap_2().child(r.find.row()).child(r.replace.row()).child(status);
        let footer = div()
            .flex()
            .gap_2()
            .child(dialogs::button("rf-cancel", "Hủy (Esc)", false, cx, |ws: &mut Workspace, window, cx| ws.close_replace_in_files(window, cx)))
            .child(dialogs::button("rf-ok", "Xem trước / Thay (Enter)", true, cx, |ws: &mut Workspace, window, cx| ws.confirm_replace_in_files(window, cx)));
        Some(
            dialogs::modal("rf-body", "Thay thế trong project", 620., body, footer)
                .on_action(cx.listener(|ws, _: &DialogConfirm, window, cx| ws.confirm_replace_in_files(window, cx)))
                .on_action(cx.listener(|ws, _: &DialogCancel, window, cx| ws.close_replace_in_files(window, cx)))
                .on_action(cx.listener(|ws, _: &DialogNext, window, cx| {
                    if let Some(r) = ws.replace_files.as_ref() {
                        dialogs::focus_step(&[&r.find, &r.replace], true, window, cx)
                    }
                }))
                .on_action(cx.listener(|ws, _: &DialogPrev, window, cx| {
                    if let Some(r) = ws.replace_files.as_ref() {
                        dialogs::focus_step(&[&r.find, &r.replace], false, window, cx)
                    }
                }))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn literal_replace() {
        assert_eq!(super::replace_literal("a.b a.b A.B", "a.b", "x"), ("x x A.B".to_string(), 2));
        assert_eq!(super::replace_literal("abc", "", "x"), ("abc".to_string(), 0));
    }
}
