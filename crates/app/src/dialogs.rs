//! Shared bits for modal dialogs (breakpoint condition, run configurations, settings):
//! Enter = save, Esc = cancel, Tab / Shift+Tab = next / previous field.
//! Layout: title, a scrollable body, and a footer that always stays visible.
use crate::input::TextInput;
use crate::theme::{self, c};
use gpui::{
    AnyElement, App, Context, Div, Entity, Focusable, KeyBinding, MouseButton, SharedString, Window, actions, div, prelude::*, px,
    relative,
};

actions!(dialog, [DialogConfirm, DialogCancel, DialogNext, DialogPrev]);

pub fn bind_keys(cx: &mut App) {
    let ctx = Some("Dialog");
    cx.bind_keys([
        KeyBinding::new("enter", DialogConfirm, ctx),
        KeyBinding::new("escape", DialogCancel, ctx),
        KeyBinding::new("tab", DialogNext, ctx),
        KeyBinding::new("shift-tab", DialogPrev, ctx),
        // Enter in the Watches box adds the watch
        KeyBinding::new("enter", DialogConfirm, Some("WatchBox")),
    ]);
}

/// Width of the label column, wide enough that no label wraps.
pub const LABEL_W: f32 = 230.;

/// A labelled single-line text field; `width` None = fill the row.
pub struct Field {
    pub label: &'static str,
    pub input: Entity<TextInput>,
    pub width: Option<f32>,
}

impl Field {
    pub fn new<T: 'static>(label: &'static str, hint: &'static str, value: &str, cx: &mut Context<T>) -> Self {
        let input = cx.new(|cx| TextInput::new(hint, cx));
        input.update(cx, |i, cx| i.set_value(value, cx));
        Self { label, input, width: None }
    }

    /// Short field for numbers / sizes.
    pub fn narrow(mut self, w: f32) -> Self {
        self.width = Some(w);
        self
    }

    pub fn text(&self, cx: &App) -> String {
        self.input.read(cx).content.trim().to_string()
    }

    pub fn set(&self, value: &str, cx: &mut App) {
        self.input.update(cx, |i, cx| i.set_value(value, cx));
    }

    pub fn row(&self) -> Div {
        let input = div().child(self.input.clone());
        let input = match self.width {
            Some(w) => input.w(px(w)).flex_none(),
            None => input.flex_1(),
        };
        label_row(self.label).child(input)
    }
}

/// "Label ……… control" row; the label never wraps.
pub fn label_row(label: &'static str) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .min_h(px(28.))
        .child(div().w(px(LABEL_W)).flex_none().whitespace_nowrap().overflow_hidden().text_color(c(theme::FG_DIM)).child(label))
}

/// A small group heading inside a dialog.
pub fn section(title: &'static str) -> Div {
    section_owned(title.to_string())
}

pub fn section_owned(title: String) -> Div {
    div()
        .mt_2()
        .pb_1()
        .border_b_1()
        .border_color(c(theme::BORDER))
        .text_color(c(theme::ACCENT))
        .child(title)
}

/// Moves focus to the next/previous field (wrapping).
pub fn focus_step(fields: &[&Field], forward: bool, window: &mut Window, cx: &mut App) {
    if fields.is_empty() {
        return;
    }
    let current = fields.iter().position(|f| f.input.read(cx).focus_handle(cx).is_focused(window));
    let n = fields.len();
    let next = match current {
        Some(i) if forward => (i + 1) % n,
        Some(i) => (i + n - 1) % n,
        None => 0,
    };
    window.focus(&fields[next].input.read(cx).focus_handle(cx));
}

/// Segmented choice (theme, module, JDK, on/off): one option selected.
pub fn choice_row<T: 'static>(
    label: &'static str,
    options: &[String],
    selected: usize,
    id: &'static str,
    cx: &mut Context<T>,
    on_pick: impl Fn(&mut T, usize, &mut Context<T>) + Clone + 'static,
) -> Div {
    label_row(label).child(div().flex_1().flex().flex_row().flex_wrap().gap_1().children(options.iter().enumerate().map(|(i, o)| {
        let pick = on_pick.clone();
        div()
            .id((id, i))
            .px_3()
            .border_1()
            .whitespace_nowrap()
            .border_color(c(if i == selected { theme::ACCENT } else { theme::BORDER }))
            .mx_1()
            .rounded_full()
            .when(i == selected, |d| d.bg(c(theme::ROW_ACTIVE)))
            .hover(|s| s.bg(c(theme::ROW_HOVER)))
            .child(SharedString::from(o.clone()))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                pick(this, i, cx);
                cx.notify()
            }))
    })))
}

pub fn button<T: 'static>(id: &'static str, label: &'static str, primary: bool, cx: &mut Context<T>, on_click: impl Fn(&mut T, &mut Window, &mut Context<T>) + 'static) -> impl IntoElement {
    div()
        .id(id)
        .debug_selector(move || format!("button:{id}"))
        .px_4()
        .py(px(3.))
        .rounded_full()
        .border_1()
        .whitespace_nowrap()
        .border_color(c(theme::BORDER))
        .when(primary, |d| d.bg(c(theme::ACCENT)).text_color(gpui::white()))
        .hover(|s| s.opacity(0.85))
        .child(label)
        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| on_click(this, window, cx)))
}

/// Centered modal: title, scrollable body, fixed footer - never taller than the window.
pub fn modal(id: &'static str, title: &str, width: f32, body: impl IntoElement, footer: impl IntoElement) -> Div {
    div()
        .occlude()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .pt(px(40.))
        .pb(px(40.))
        .bg(gpui::rgba(0x00000066))
        .child(
            div()
                .key_context("Dialog")
                .w(px(width))
                .max_w(relative(0.95))
                .max_h(relative(1.))
                .flex()
                .flex_col()
                .bg(c(theme::PANEL_BG))
                .border_1()
                .border_color(c(theme::BORDER))
                .shadow_lg().occlude().rounded(px(18.))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .px_4()
                        .py_2()
                        .border_b_1()
                        .border_color(c(theme::BORDER))
                        .text_size(px(15.))
                        .text_color(c(theme::FG))
                        .child(SharedString::from(title.to_string())),
                )
                .child(div().id(id).flex_1().min_h(px(0.)).overflow_y_scroll().px_4().py_2().child(body))
                .child(div().flex().flex_row().items_center().justify_end().gap_2().px_4().py_2().border_t_1().border_color(c(theme::BORDER)).child(footer)),
        )
}

/// Dropdown select with a filter box (fonts: every installed font, each shown in itself).
pub struct Select {
    pub open: bool,
    pub filter: Entity<TextInput>,
    pub scroll: gpui::UniformListScrollHandle,
}

impl Select {
    pub fn new<T: 'static>(cx: &mut Context<T>) -> Self {
        Self { open: false, filter: cx.new(|cx| TextInput::new("gõ để lọc…", cx)), scroll: gpui::UniformListScrollHandle::new() }
    }
}

/// Height of an option row (uniform_list measures one row for all).
const SELECT_ROW_H: f32 = 24.;

/// Renders a Select: a button with the current value, and when open a filter + list.
/// `preview_font` shows each option in its own font family.
#[allow(clippy::too_many_arguments)]
pub fn select_row<T: 'static>(
    label: &'static str,
    id: &'static str,
    value: &str,
    select: &Select,
    options: &[String],
    preview_font: bool,
    cx: &mut Context<T>,
    toggle: impl Fn(&mut T, &mut Window, &mut Context<T>) + 'static,
    pick: impl Fn(&mut T, String, &mut Window, &mut Context<T>) + Clone + 'static,
) -> AnyElement {
    let q = select.filter.read(cx).content.to_lowercase();
    let matching: Vec<&String> = options.iter().filter(|o| q.is_empty() || o.to_lowercase().contains(&q)).collect();
    let total = matching.len();
    let shown: Vec<String> = matching.into_iter().cloned().collect();
    let current = value.to_string();
    let button = div()
        .id(id)
        .flex_1()
        .flex()
        .justify_between()
        .px_2()
        .border_1()
        .rounded_sm()
        .border_color(c(if select.open { theme::ACCENT } else { theme::BORDER }))
        .bg(c(theme::BG))
        .hover(|s| s.bg(c(theme::ROW_HOVER)))
        .child(div().when(preview_font, |d| d.font_family(SharedString::from(current.clone()))).child(SharedString::from(current.clone())))
        .child(div().text_color(c(theme::FG_DIM)).child(if select.open { "▴" } else { "▾" }))
        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| {
            toggle(this, window, cx);
            cx.notify()
        }));
    let row = label_row(label).child(button);
    if !select.open {
        return row.into_any_element();
    }
    let n = shown.len();
    // virtualized: with every installed font listed, only the visible rows are built (and shaped)
    let shown = std::sync::Arc::new(shown);
    let list = gpui::uniform_list(SharedString::from(format!("{id}-uni")), n, cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
        let _: &mut T = this;
        range
            .map(|i| {
                let o = shown[i].clone();
                let pick = pick.clone();
                let is_current = o == current;
                div()
                    .id((id, i))
                    .h(px(SELECT_ROW_H))
                    .flex()
                    .items_center()
                    .px_2()
                    .whitespace_nowrap()
                    .mx_1()
                    .rounded_md()
                    .when(is_current, |d| d.bg(c(theme::ROW_ACTIVE)))
                    .hover(|s| s.bg(c(theme::ROW_HOVER)))
                    .when(preview_font, |d| d.font_family(SharedString::from(o.clone())))
                    .child(SharedString::from(o.clone()))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| {
                        pick(this, o.clone(), window, cx);
                        cx.notify()
                    }))
            })
            .collect::<Vec<_>>()
    }))
    .track_scroll(select.scroll.clone());
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(row)
        .child(
            div()
                .ml(px(LABEL_W + 12.))
                .flex()
                .flex_col()
                .gap_1()
                .p_1()
                .border_1()
                .border_color(c(theme::BORDER))
                .rounded_sm()
                .bg(c(theme::BG))
                .child(select.filter.clone())
                .child(
                    div()
                        .id(SharedString::from(format!("{id}-list")))
                        .debug_selector(move || format!("select-list:{id}"))
                        .h(px((n as f32 * SELECT_ROW_H).min(240.)))
                        // the wheel scrolls this list only, not the dialog around it too
                        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                        .child(list.size_full()),
                )
                .child(div().px_2().text_size(px(11.)).text_color(c(theme::FG_DIM)).child(if n < total {
                    format!("đang hiện {n}/{total} - gõ để lọc")
                } else {
                    format!("{total} mục")
                })),
        )
        .into_any_element()
}

/// Focuses `handle` after the current event is fully handled. Dialogs are often opened from
/// a mouse click, and gpui re-focuses the clicked area when that click finishes - focusing
/// right away would be undone, and Enter / typing would not reach the dialog.
pub fn focus_later(handle: gpui::FocusHandle, window: &mut Window, cx: &mut App) {
    window.defer(cx, move |window, _| window.focus(&handle));
}
