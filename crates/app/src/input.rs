//! Single-line text input with IME support - used by the find bar and pickers.
//! Adapted from gpui's examples/input.rs (the probe that passed the Vietnamese test).
use crate::theme::{self, c};
use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, KeyBinding, LayoutId, MouseButton,
    MouseDownEvent, Pixels, Point, ShapedLine, SharedString, Style, TextRun, UTF16Selection, UnderlineStyle,
    Window, actions, div, fill, point, prelude::*, px, relative, size,
};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

actions!(text_input, [Backspace, Delete, Left, Right, SelectLeft, SelectRight, SelectAll, Home, End, Paste, Copy, Cut, WordBackspace]);

pub fn bind_keys(cx: &mut App) {
    let ctx = Some("TextInput");
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, ctx),
        KeyBinding::new("ctrl-backspace", WordBackspace, ctx),
        KeyBinding::new("delete", Delete, ctx),
        KeyBinding::new("left", Left, ctx),
        KeyBinding::new("right", Right, ctx),
        KeyBinding::new("shift-left", SelectLeft, ctx),
        KeyBinding::new("shift-right", SelectRight, ctx),
        KeyBinding::new("ctrl-a", SelectAll, ctx),
        KeyBinding::new("home", Home, ctx),
        KeyBinding::new("end", End, ctx),
        KeyBinding::new("ctrl-v", Paste, ctx),
        KeyBinding::new("ctrl-c", Copy, ctx),
        KeyBinding::new("ctrl-x", Cut, ctx),
    ]);
}

pub enum InputEvent {
    Changed(String),
}

pub struct TextInput {
    focus: FocusHandle,
    pub content: String,
    placeholder: SharedString,
    selected: Range<usize>,
    reversed: bool,
    marked: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    /// no frame: the parent draws it
    pub bare: bool,
    /// shows bullets instead of the text (tokens); copy / cut are disabled
    pub secret: bool,
}

impl EventEmitter<InputEvent> for TextInput {}

impl TextInput {
    pub fn new(placeholder: &str, cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            content: String::new(),
            placeholder: placeholder.to_string().into(),
            selected: 0..0,
            reversed: false,
            marked: None,
            last_layout: None,
            last_bounds: None,
            bare: false,
            secret: false,
        }
    }

    /// Sets the value with the cursor at the end (form fields).
    pub fn set_value(&mut self, text: &str, cx: &mut Context<Self>) {
        self.content = text.to_string();
        self.selected = self.content.len()..self.content.len();
        self.reversed = false;
        self.marked = None;
        cx.emit(InputEvent::Changed(self.content.clone()));
        cx.notify();
    }

    /// Sets the value with everything selected, so typing replaces it (find, rename).
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.content = text.to_string();
        self.selected = 0..self.content.len();
        self.reversed = false;
        self.marked = None;
        cx.emit(InputEvent::Changed(self.content.clone()));
        cx.notify();
    }

    fn cursor(&self) -> usize {
        if self.reversed { self.selected.start } else { self.selected.end }
    }

    fn move_to(&mut self, at: usize, cx: &mut Context<Self>) {
        self.selected = at..at;
        cx.notify();
    }

    fn select_to(&mut self, at: usize, cx: &mut Context<Self>) {
        if self.reversed { self.selected.start = at } else { self.selected.end = at }
        if self.selected.end < self.selected.start {
            self.reversed = !self.reversed;
            self.selected = self.selected.end..self.selected.start;
        }
        cx.notify();
    }

    fn prev(&self, at: usize) -> usize {
        self.content.grapheme_indices(true).rev().find_map(|(i, _)| (i < at).then_some(i)).unwrap_or(0)
    }

    fn next(&self, at: usize) -> usize {
        self.content.grapheme_indices(true).find_map(|(i, _)| (i > at).then_some(i)).unwrap_or(self.content.len())
    }

    fn replace(&mut self, range: Range<usize>, text: &str, cx: &mut Context<Self>) {
        self.content.replace_range(range.clone(), text);
        let at = range.start + text.len();
        self.selected = at..at;
        cx.emit(InputEvent::Changed(self.content.clone()));
        cx.notify();
    }

    fn to_utf16(&self, at: usize) -> usize {
        self.content[..at.min(self.content.len())].encode_utf16().count()
    }

    fn from_utf16(&self, u: usize) -> usize {
        let mut units = 0;
        for (i, ch) in self.content.char_indices() {
            if units >= u {
                return i;
            }
            units += ch.len_utf16();
        }
        self.content.len()
    }

    fn range_from_utf16(&self, r: &Range<usize>) -> Range<usize> {
        self.from_utf16(r.start)..self.from_utf16(r.end)
    }

    fn range_to_utf16(&self, r: &Range<usize>) -> Range<usize> {
        self.to_utf16(r.start)..self.to_utf16(r.end)
    }

    /// Byte offset of `i` in what is drawn: bullets (3 bytes each) in a secret field.
    fn shown_index(&self, i: usize) -> usize {
        if self.secret { self.content[..i.min(self.content.len())].chars().count() * '•'.len_utf8() } else { i }
    }

    /// Inverse of [`shown_index`]: a byte offset in the drawn text back to the content.
    fn content_index(&self, shown: usize) -> usize {
        if self.secret { self.content.char_indices().nth(shown / '•'.len_utf8()).map(|(i, _)| i).unwrap_or(self.content.len()) } else { shown }
    }

    fn index_for_point(&self, p: Point<Pixels>) -> usize {
        match (&self.last_bounds, &self.last_layout) {
            (Some(b), Some(l)) => self.content_index(l.closest_index_for_x(p.x - b.left())).min(self.content.len()),
            _ => 0,
        }
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(&mut self, r: Range<usize>, actual: &mut Option<Range<usize>>, _: &mut Window, _: &mut Context<Self>) -> Option<String> {
        let r = self.range_from_utf16(&r);
        actual.replace(self.range_to_utf16(&r));
        Some(self.content[r].to_string())
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        Some(UTF16Selection { range: self.range_to_utf16(&self.selected), reversed: self.reversed })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|r| self.range_to_utf16(r))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(&mut self, r: Option<Range<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        let range = r.map(|r| self.range_from_utf16(&r)).or(self.marked.clone()).unwrap_or(self.selected.clone());
        self.marked = None;
        self.replace(range, text, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        r: Option<Range<usize>>,
        text: &str,
        _sel: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = r.map(|r| self.range_from_utf16(&r)).or(self.marked.clone()).unwrap_or(self.selected.clone());
        self.replace(range.clone(), text, cx);
        self.marked = (!text.is_empty()).then(|| range.start..range.start + text.len());
    }

    fn bounds_for_range(&mut self, r: Range<usize>, bounds: Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        let l = self.last_layout.as_ref()?;
        let r = self.range_from_utf16(&r);
        Some(Bounds::from_corners(
            point(bounds.left() + l.x_for_index(self.shown_index(r.start)), bounds.top()),
            point(bounds.left() + l.x_for_index(self.shown_index(r.end)), bounds.bottom()),
        ))
    }

    fn character_index_for_point(&mut self, p: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        Some(self.to_utf16(self.index_for_point(p)))
    }
}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus.is_focused(window);
        div()
            .key_context("TextInput")
            .track_focus(&self.focus)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(|this, _: &Backspace, _, cx| {
                if this.selected.is_empty() {
                    let p = this.prev(this.cursor());
                    this.select_to(p, cx)
                }
                this.replace(this.selected.clone(), "", cx)
            }))
            .on_action(cx.listener(|this, _: &WordBackspace, _, cx| {
                let end = this.cursor();
                let text = &this.content[..end];
                let start = text.trim_end().rfind(|ch: char| !ch.is_alphanumeric()).map(|i| i + 1).unwrap_or(0);
                this.replace(start..end, "", cx)
            }))
            .on_action(cx.listener(|this, _: &Delete, _, cx| {
                if this.selected.is_empty() {
                    let n = this.next(this.cursor());
                    this.select_to(n, cx)
                }
                this.replace(this.selected.clone(), "", cx)
            }))
            .on_action(cx.listener(|this, _: &Left, _, cx| {
                let t = if this.selected.is_empty() { this.prev(this.cursor()) } else { this.selected.start };
                this.move_to(t, cx)
            }))
            .on_action(cx.listener(|this, _: &Right, _, cx| {
                let t = if this.selected.is_empty() { this.next(this.cursor()) } else { this.selected.end };
                this.move_to(t, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectLeft, _, cx| this.select_to(this.prev(this.cursor()), cx)))
            .on_action(cx.listener(|this, _: &SelectRight, _, cx| this.select_to(this.next(this.cursor()), cx)))
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| {
                this.selected = 0..this.content.len();
                cx.notify()
            }))
            .on_action(cx.listener(|this, _: &Home, _, cx| this.move_to(0, cx)))
            .on_action(cx.listener(|this, _: &End, _, cx| this.move_to(this.content.len(), cx)))
            .on_action(cx.listener(|this, _: &Paste, _, cx| {
                if let Some(t) = cx.read_from_clipboard().and_then(|i| i.text()) {
                    this.replace(this.selected.clone(), &t.replace('\n', " "), cx)
                }
            }))
            .on_action(cx.listener(|this, _: &Copy, _, cx| {
                if !this.selected.is_empty() && !this.secret {
                    cx.write_to_clipboard(ClipboardItem::new_string(this.content[this.selected.clone()].to_string()))
                }
            }))
            .on_action(cx.listener(|this, _: &Cut, _, cx| {
                if !this.selected.is_empty() && !this.secret {
                    cx.write_to_clipboard(ClipboardItem::new_string(this.content[this.selected.clone()].to_string()));
                    this.replace(this.selected.clone(), "", cx)
                }
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                window.focus(&this.focus);
                let at = this.index_for_point(ev.position);
                if ev.modifiers.shift { this.select_to(at, cx) } else { this.move_to(at, cx) }
            }))
            .w_full()
            .when(!self.bare, |d| d.px_2().py_1().bg(c(theme::BG)).border_1().border_color(c(if focused { theme::ACCENT } else { theme::BORDER })).rounded_sm())
            .child(InputElement { input: cx.entity() })
    }
}

struct InputElement {
    input: Entity<TextInput>,
}

impl IntoElement for InputElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

struct InputPrepaint {
    line: Option<ShapedLine>,
    cursor: Option<gpui::PaintQuad>,
    selection: Option<gpui::PaintQuad>,
}

impl Element for InputElement {
    type RequestLayoutState = ();
    type PrepaintState = InputPrepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> InputPrepaint {
        let input = self.input.read(cx);
        let style = window.text_style();
        let (text, color): (SharedString, _) = if input.content.is_empty() {
            (input.placeholder.clone(), c(theme::FG_DIM))
        } else {
            (if input.secret { "•".repeat(input.content.chars().count()).into() } else { input.content.clone().into() }, c(theme::FG))
        };
        let run = TextRun { len: text.len(), font: style.font(), color, background_color: None, underline: None, strikethrough: None };
        let runs = match &input.marked {
            Some(m) if !input.content.is_empty() => [
                TextRun { len: m.start, ..run.clone() },
                TextRun { len: m.end - m.start, underline: Some(UnderlineStyle { color: Some(color), thickness: px(1.), wavy: false }), ..run.clone() },
                TextRun { len: text.len() - m.end, ..run },
            ]
            .into_iter()
            .filter(|r| r.len > 0)
            .collect(),
            _ => vec![run],
        };
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window.text_system().shape_line(text, font_size, &runs, None);
        // selection only shows in the focused field (otherwise every form row looks selected)
        let focused = input.focus.is_focused(window);
        let sel = if focused { input.selected.clone() } else { input.cursor()..input.cursor() };
        let (cursor, selection) = if sel.is_empty() {
            let x = if input.content.is_empty() { px(0.) } else { line.x_for_index(input.shown_index(input.cursor())) };
            (Some(fill(Bounds::new(point(bounds.left() + x, bounds.top()), size(px(2.), bounds.size.height)), c(theme::CURSOR))), None)
        } else {
            (
                None,
                Some(fill(
                    Bounds::from_corners(point(bounds.left() + line.x_for_index(input.shown_index(sel.start)), bounds.top()), point(bounds.left() + line.x_for_index(input.shown_index(sel.end)), bounds.bottom())),
                    c(theme::SELECTION),
                )),
            )
        };
        InputPrepaint { line: Some(line), cursor, selection }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        pre: &mut InputPrepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.input.read(cx).focus.clone();
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.input.clone()), cx);
        let line = pre.line.take().unwrap();
        // clip to the field: long text / placeholders must not spill over neighbouring buttons
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            if let Some(s) = pre.selection.take() {
                window.paint_quad(s)
            }
            let _ = line.paint(bounds.origin, window.line_height(), window, cx);
            if focus.is_focused(window) {
                if let Some(cur) = pre.cursor.take() {
                    window.paint_quad(cur);
                }
            }
        });
        self.input.update(cx, |i, _| {
            i.last_layout = Some(line);
            i.last_bounds = Some(bounds);
        });
    }
}

#[cfg(test)]
mod secret_tests {
    use super::*;
    use gpui::TestAppContext;

    /// A secret field draws bullets, and clicks / the cursor still map onto the real text.
    #[gpui::test]
    fn secret_field_maps_offsets_between_bullets_and_text(cx: &mut TestAppContext) {
        let input = cx.new(|cx| TextInput::new("token", cx));
        input.update(cx, |i, cx| {
            i.secret = true;
            i.set_value("aấb", cx); // 'ấ' is 3 bytes
        });
        input.read_with(cx, |i, _| {
            assert_eq!(i.content.len(), 5);
            // after 'a' and 'ấ' there are two bullets of 3 bytes
            assert_eq!(i.shown_index(4), 6);
            assert_eq!(i.content_index(6), 4);
            assert_eq!(i.content_index(9), 5, "past the end clamps to the text");
        });
    }
}
