//! "Breakpoint condition" dialog (right-click a breakpoint / Ctrl+Shift+F8), like IntelliJ's.
use crate::dialogs::{self, DialogCancel, DialogConfirm};
use crate::input::TextInput;
use crate::theme::{self, c};
use crate::workspace::Workspace;
use gpui::{AnyElement, Context, Focusable, Window, div, prelude::*};
use std::path::PathBuf;

impl Workspace {
    pub(crate) fn open_bp_dialog(&mut self, path: PathBuf, line: usize, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.bp_conditions.get(&path).and_then(|c| c.get(&line)).cloned().unwrap_or_default();
        let input = cx.new(|cx| TextInput::new("biểu thức Java, vd: name.equals(\"Lan\") && count > 2", cx));
        input.update(cx, |i, cx| i.set_text(&current, cx));
        crate::dialogs::focus_later(input.read(cx).focus_handle(cx), window, cx);
        self.bp_dialog = Some((path, line, input));
        cx.notify();
    }

    fn close_bp_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.bp_dialog = None;
        if let Some(i) = self.active {
            self.activate(i, window, cx);
        }
        cx.notify();
    }

    pub(crate) fn render_bp_dialog(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (path, line, input) = self.bp_dialog.clone()?;
        let exists = self.breakpoints.get(&path).is_some_and(|l| l.contains(&line));
        let title = format!("Breakpoint {}:{}", crate::tree::file_name(&path), line + 1);
        let body = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().text_color(c(theme::FG_DIM)).child("Chỉ dừng khi điều kiện đúng (để trống = luôn dừng). Dùng được cho cả Debug app và Debug unit test."))
            .child(input);
        let footer = div()
            .flex()
            .flex_row()
            .gap_2()
            .when(exists, |d| {
                d.child(dialogs::button("bp-remove", "Xóa breakpoint", false, cx, |ws: &mut Workspace, window, cx| {
                    if let Some((p, l, _)) = ws.bp_dialog.clone() {
                        ws.remove_breakpoint(p, l, cx);
                    }
                    ws.close_bp_dialog(window, cx)
                }))
            })
            .child(dialogs::button("bp-cancel", "Hủy (Esc)", false, cx, |ws: &mut Workspace, window, cx| ws.close_bp_dialog(window, cx)))
            .child(dialogs::button("bp-save", "Lưu (Enter)", true, cx, |ws: &mut Workspace, window, cx| ws.confirm_bp_dialog(window, cx)));
        let dialog = dialogs::modal("bp-body", &title, 620., body, footer)
            .on_action(cx.listener(|ws, _: &DialogConfirm, window, cx| ws.confirm_bp_dialog(window, cx)))
            .on_action(cx.listener(|ws, _: &DialogCancel, window, cx| ws.close_bp_dialog(window, cx)));
        Some(dialog.into_any_element())
    }

    fn confirm_bp_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((p, l, i)) = self.bp_dialog.clone() {
            let cond = i.read(cx).content.clone();
            self.set_breakpoint_condition(p, l, &cond, cx);
        }
        self.close_bp_dialog(window, cx)
    }
}
