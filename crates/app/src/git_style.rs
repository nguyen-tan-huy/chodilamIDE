//! Small building blocks shared by the Git tool window and the Git log: icon
//! buttons with tooltips, status badges, author avatars and checkboxes.
use crate::theme::{self, c};
use crate::ui_icons::{Ico, icon, tip};
use gpui::{Div, Hsla, Stateful, div, prelude::*, px};

/// A square icon-only button; the label lives in the tooltip.
pub fn icon_btn(id: &'static str, ico: Ico, label: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .size(px(26.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .hover(|s| s.bg(c(theme::ROW_HOVER)))
        .tooltip(tip(label))
        .child(icon(ico, 15., c(theme::FG_DIM)))
}

/// The one-letter git status (M, A, D, R, ?, !) as a small colored badge.
pub fn status_badge(letter: char, color: u32) -> Div {
    let mut bg: Hsla = c(color);
    bg.a = 0.16;
    div()
        .flex_none()
        .w(px(18.))
        .h(px(16.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .bg(bg)
        .text_size(px(10.5))
        .font_weight(gpui::FontWeight::BOLD)
        .text_color(c(color))
        .child(letter.to_string())
}

/// A round avatar with the author's initial, colored by the name.
pub fn avatar(name: &str, size: f32) -> Div {
    const COLORS: [u32; 6] = [0x0a84ff, 0x30d158, 0xff9f0a, 0xbf5af2, 0xff375f, 0x64d2ff];
    let h = name.bytes().fold(0usize, |a, b| a.wrapping_mul(31).wrapping_add(b as usize));
    let initial = name.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().to_string()).unwrap_or_default();
    div()
        .flex_none()
        .size(px(size))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(gpui::rgb(COLORS[h % COLORS.len()]))
        .text_size(px(size * 0.5))
        .font_weight(gpui::FontWeight::BOLD)
        .text_color(gpui::white())
        .child(initial)
}

/// A checkbox: empty, checked (accent) or a conflict mark.
pub fn check_box(checked: bool, conflict: bool) -> Div {
    let d = div().flex_none().size(px(14.)).rounded(px(4.)).flex().items_center().justify_center();
    if conflict {
        d.bg(c(theme::ERROR)).child(div().text_size(px(10.)).font_weight(gpui::FontWeight::BOLD).text_color(gpui::white()).child("!"))
    } else if checked {
        d.bg(c(theme::ACCENT)).child(icon(Ico::Check, 11., gpui::white()))
    } else {
        d.border_1().border_color(c(theme::FG_DIM))
    }
}

/// A small rounded count next to a section title.
pub fn count_pill(n: usize) -> Div {
    div().flex_none().px(px(6.)).rounded_full().bg(c(theme::ROW_HOVER)).text_size(px(10.5)).text_color(c(theme::FG_DIM)).child(n.to_string())
}

/// "2026-10-09 16:12" → "09/10 16:12" (same year) or "09/10/25".
pub fn short_date(d: &str, this_year: &str) -> String {
    let (Some(y), Some(m), Some(day)) = (d.get(0..4), d.get(5..7), d.get(8..10)) else { return d.to_string() };
    if y == this_year { format!("{day}/{m} {}", d.get(11..16).unwrap_or("")) } else { format!("{day}/{m}/{}", &y[2..]) }
}
