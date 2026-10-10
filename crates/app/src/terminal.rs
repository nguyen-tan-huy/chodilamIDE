//! Integrated terminal (Alt+F12): alacritty_terminal for the PTY + VT parsing, drawn with
//! gpui. Replaces the shell/lazygit tabs the WezTerm shell used to give each project.
//! Keys go to the shell before IDE shortcuts (keystroke interceptor), except a few
//! Ctrl+Shift / Alt+digit ones; plain text arrives through the input handler so IME works.
use crate::theme::{self, c};
use alacritty_terminal::event::{Event, EventListener, Notify, OnResize, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Notifier};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point as TPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::{Color as AColor, NamedColor, Rgb};
use futures::StreamExt;
use gpui::{
    App, Bounds, ClipboardItem, Context, Element, ElementId, ElementInputHandler, Entity, EntityInputHandler, FocusHandle,
    Focusable, Global, GlobalElementId, Hsla, KeystrokeEvent, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels,
    ScrollWheelEvent, SharedString, Style, TextRun, UTF16Selection, WeakEntity, Window, div, fill, point, prelude::*, px,
    relative, size,
};
use std::borrow::Cow;
use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

#[derive(Clone)]
struct Listener(futures::channel::mpsc::UnboundedSender<Event>);

impl EventListener for Listener {
    fn send_event(&self, e: Event) {
        let _ = self.0.unbounded_send(e);
    }
}

struct Size {
    cols: u16,
    rows: u16,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows as usize
    }
    fn screen_lines(&self) -> usize {
        self.rows as usize
    }
    fn columns(&self) -> usize {
        self.cols as usize
    }
}

pub struct TerminalView {
    term: Arc<FairMutex<Term<Listener>>>,
    notifier: Notifier,
    focus: FocusHandle,
    pub title: String,
    pub exited: Option<i32>,
    cols: u16,
    rows: u16,
    /// cell size + origin of the last paint, for mouse → grid
    cell: (Pixels, Pixels),
    origin: gpui::Point<Pixels>,
}

/// All live terminals, so the keystroke interceptor can find the focused one.
#[derive(Default)]
pub struct Terminals(Vec<WeakEntity<TerminalView>>);
impl Global for Terminals {}

/// A started shell: terminal state, the PTY writer and the event stream.
pub struct Pty {
    term: Arc<FairMutex<Term<Listener>>>,
    notifier: Notifier,
    rx: futures::channel::mpsc::UnboundedReceiver<Event>,
    cols: u16,
    rows: u16,
}

impl Pty {
    /// Starts the user's shell ($SHELL) in `cwd`.
    pub fn spawn(cwd: &Path) -> anyhow::Result<Self> {
        Self::spawn_with(cwd, None)
    }

    pub fn spawn_with(cwd: &Path, shell: Option<tty::Shell>) -> anyhow::Result<Self> {
        let (tx, rx) = futures::channel::mpsc::unbounded();
        let listener = Listener(tx);
        let (cols, rows) = (100u16, 24u16);
        let config = Config { scrolling_history: 10_000, ..Config::default() };
        let term = Arc::new(FairMutex::new(Term::new(config, &Size { cols, rows }, listener.clone())));
        let mut env = HashMap::new();
        env.insert("TERM".to_string(), "xterm-256color".to_string());
        env.insert("COLORTERM".to_string(), "truecolor".to_string());
        env.insert("CHODILAMIDE".to_string(), "1".to_string());
        let options = tty::Options { shell, working_directory: Some(cwd.to_path_buf()), drain_on_exit: true, env, ..Default::default() };
        let pty = tty::new(&options, WindowSize { num_lines: rows, num_cols: cols, cell_width: 8, cell_height: 16 }, 0)?;
        let event_loop = EventLoop::new(term.clone(), listener, pty, true, false)?;
        let notifier = Notifier(event_loop.channel());
        event_loop.spawn();
        Ok(Self { term, notifier, rx, cols, rows })
    }
}

impl TerminalView {
    pub fn new(pty: Pty, cx: &mut Context<Self>) -> Self {
        let Pty { term, notifier, mut rx, cols, rows } = pty;
        cx.spawn(async move |this, cx| {
            while let Some(ev) = rx.next().await {
                let alive = this.update(cx, |t, cx| {
                    match ev {
                        Event::Title(s) => t.title = s,
                        Event::ResetTitle => t.title = String::new(),
                        Event::PtyWrite(s) => t.notifier.notify(s.into_bytes()),
                        Event::ClipboardStore(_, s) => cx.write_to_clipboard(ClipboardItem::new_string(s)),
                        Event::ChildExit(status) => t.exited = Some(status.code().unwrap_or(-1)),
                        Event::Exit => t.exited = t.exited.or(Some(0)),
                        _ => {}
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();

        let focus = cx.focus_handle();
        let weak = cx.entity().downgrade();
        cx.default_global::<Terminals>().0.push(weak);
        Self { term, notifier, focus, title: String::new(), exited: None, cols, rows, cell: (px(8.), px(16.)), origin: point(px(0.), px(0.)) }
    }

    pub fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        self.term.lock().scroll_display(Scroll::Bottom);
        self.notifier.notify(bytes);
    }

    fn paste(&self, text: &str) {
        let bracketed = self.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
        let text = text.replace("\r\n", "\r").replace('\n', "\r");
        if bracketed {
            self.write(format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', "")).into_bytes());
        } else {
            self.write(text.into_bytes());
        }
    }

    /// Handles a keystroke meant for the shell. Returns false for keys the IDE keeps
    /// (and for plain text, which comes through the input handler instead).
    pub fn handle_key(&mut self, ks: &gpui::Keystroke, cx: &mut Context<Self>) -> bool {
        let m = &ks.modifiers;
        let key = ks.key.as_str();
        // IDE shortcuts that stay global even in the terminal
        if m.control && m.shift && !matches!(key, "c" | "v") {
            return false;
        }
        if m.alt && key.len() == 1 && key.chars().all(|c| c.is_ascii_digit()) {
            return false;
        }
        if m.alt && key == "f12" {
            return false;
        }
        if m.control && m.shift && key == "c" {
            let text = self.term.lock().selection_to_string();
            if let Some(t) = text.filter(|t| !t.is_empty()) {
                cx.write_to_clipboard(ClipboardItem::new_string(t));
            }
            return true;
        }
        if m.control && m.shift && key == "v" {
            if let Some(t) = cx.read_from_clipboard().and_then(|i| i.text()) {
                self.paste(&t);
            }
            return true;
        }
        if m.shift && matches!(key, "pageup" | "pagedown") {
            let rows = self.rows as i32;
            self.term.lock().scroll_display(Scroll::Delta(if key == "pageup" { rows - 1 } else { 1 - rows }));
            cx.notify();
            return true;
        }
        let app_cursor = self.term.lock().mode().contains(TermMode::APP_CURSOR);
        match key_bytes(ks, app_cursor) {
            Some(b) => {
                self.term.lock().selection = None;
                self.write(b);
                true
            }
            None => false,
        }
    }

    fn grid_point(&self, p: gpui::Point<Pixels>) -> TPoint {
        let term = self.term.lock();
        let offset = term.grid().display_offset() as i32;
        let col = (((p.x - self.origin.x) / self.cell.0).floor().max(0.) as usize).min(self.cols.saturating_sub(1) as usize);
        let row = ((p.y - self.origin.y) / self.cell.1).floor().max(0.) as i32;
        TPoint::new(Line(row.min(self.rows as i32 - 1) - offset), Column(col))
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        let p = self.grid_point(ev.position);
        let ty = match ev.click_count {
            2 => SelectionType::Semantic,
            3 => SelectionType::Lines,
            _ => SelectionType::Simple,
        };
        self.term.lock().selection = Some(Selection::new(ty, p, Side::Left));
        cx.notify();
    }

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if ev.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let p = self.grid_point(ev.position);
        if let Some(sel) = self.term.lock().selection.as_mut() {
            sel.update(p, Side::Right);
        }
        cx.notify();
    }

    fn on_scroll(&mut self, ev: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let dy = ev.delta.pixel_delta(self.cell.1).y;
        let lines = (dy / self.cell.1).round() as i32;
        if lines != 0 {
            self.term.lock().scroll_display(Scroll::Delta(lines));
            cx.notify();
        }
    }

    fn resize(&mut self, cols: u16, rows: u16) {
        if cols == self.cols && rows == self.rows || cols < 2 || rows < 1 {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        self.term.lock().resize(Size { cols, rows });
        let (w, h) = (f32::from(self.cell.0) as u16, f32::from(self.cell.1) as u16);
        self.notifier.on_resize(WindowSize { num_lines: rows, num_cols: cols, cell_width: w, cell_height: h });
    }
}

impl Drop for TerminalView {
    fn drop(&mut self) {
        let _ = self.notifier.0.send(alacritty_terminal::event_loop::Msg::Shutdown);
    }
}

/// Bytes a VT100/xterm expects for special keys and Ctrl/Alt combos; None = plain text.
pub fn key_bytes(ks: &gpui::Keystroke, app_cursor: bool) -> Option<Vec<u8>> {
    let m = &ks.modifiers;
    let key = ks.key.as_str();
    let arrow = |c: char| if app_cursor && !m.shift && !m.control && !m.alt { format!("\x1bO{c}") } else if m.control { format!("\x1b[1;5{c}") } else { format!("\x1b[{c}") };
    let s: String = match key {
        "enter" => if m.alt { "\x1b\r".into() } else { "\r".into() },
        "backspace" => if m.control || m.alt { "\x17".into() } else { "\x7f".into() },
        "tab" => if m.shift { "\x1b[Z".into() } else { "\t".into() },
        "escape" => "\x1b".into(),
        "up" => arrow('A'),
        "down" => arrow('B'),
        "right" => arrow('C'),
        "left" => arrow('D'),
        "home" => if app_cursor { "\x1bOH".into() } else { "\x1b[H".into() },
        "end" => if app_cursor { "\x1bOF".into() } else { "\x1b[F".into() },
        "pageup" => "\x1b[5~".into(),
        "pagedown" => "\x1b[6~".into(),
        "delete" => "\x1b[3~".into(),
        "insert" => "\x1b[2~".into(),
        "f1" => "\x1bOP".into(),
        "f2" => "\x1bOQ".into(),
        "f3" => "\x1bOR".into(),
        "f4" => "\x1bOS".into(),
        "f5" => "\x1b[15~".into(),
        "f6" => "\x1b[17~".into(),
        "f7" => "\x1b[18~".into(),
        "f8" => "\x1b[19~".into(),
        "f9" => "\x1b[20~".into(),
        "f10" => "\x1b[21~".into(),
        "f11" => "\x1b[23~".into(),
        "f12" => "\x1b[24~".into(),
        "space" if m.control => "\0".into(),
        k if m.control && k.len() == 1 => {
            let ch = k.chars().next()?.to_ascii_lowercase();
            let code = match ch {
                'a'..='z' => ch as u8 - b'a' + 1,
                '[' => 0x1b,
                '\\' => 0x1c,
                ']' => 0x1d,
                '/' | '_' => 0x1f,
                '@' | '2' => 0,
                _ => return None,
            };
            let mut v = if m.alt { vec![0x1b] } else { vec![] };
            v.push(code);
            return Some(v);
        }
        k if m.alt && k.chars().count() == 1 => {
            let ch = ks.key_char.clone().unwrap_or_else(|| k.to_string());
            format!("\x1b{ch}")
        }
        _ => return None,
    };
    Some(s.into_bytes())
}

/// Registers the global interceptor that routes keys to the focused terminal first.
pub fn install_key_interceptor(cx: &mut App) {
    cx.intercept_keystrokes(|ev: &KeystrokeEvent, window, cx| {
        if cx.try_global::<crate::vim_mode::OverlayOpen>().is_some_and(|o| o.0) {
            return;
        }
        if !ev.context_stack.iter().any(|c| c.contains("Terminal")) {
            return;
        }
        let terms: Vec<Entity<TerminalView>> = cx.default_global::<Terminals>().0.iter().filter_map(|w| w.upgrade()).collect();
        for t in terms {
            if t.read(cx).focus.is_focused(window) {
                if t.update(cx, |v, cx| v.handle_key(&ev.keystroke, cx)) {
                    cx.stop_propagation();
                }
                return;
            }
        }
    })
    .detach();
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EntityInputHandler for TerminalView {
    fn text_for_range(&mut self, _: Range<usize>, _: &mut Option<Range<usize>>, _: &mut Window, _: &mut Context<Self>) -> Option<String> {
        None
    }
    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        Some(UTF16Selection { range: 0..0, reversed: false })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        None
    }
    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {}
    /// Committed text (typing, IME) goes straight to the shell.
    fn replace_text_in_range(&mut self, _: Option<Range<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        if !text.is_empty() {
            self.term.lock().selection = None;
            self.write(text.to_string().into_bytes());
            cx.notify();
        }
    }
    /// Preedit is not shown inside the grid yet; the IME's own popup shows it.
    fn replace_and_mark_text_in_range(&mut self, _: Option<Range<usize>>, _: &str, _: Option<Range<usize>>, _: &mut Window, _: &mut Context<Self>) {}
    fn bounds_for_range(&mut self, _: Range<usize>, _: Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        // put the IME candidate window at the cursor
        let t = self.term.lock();
        let cur = t.grid().cursor.point;
        let x = self.origin.x + self.cell.0 * cur.column.0 as f32;
        let y = self.origin.y + self.cell.1 * (cur.line.0 + t.grid().display_offset() as i32) as f32;
        Some(Bounds::new(point(x, y), size(self.cell.0, self.cell.1)))
    }
    fn character_index_for_point(&mut self, _: gpui::Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        None
    }
}

impl Render for TerminalView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("terminal")
            .key_context("Terminal")
            .track_focus(&self.focus)
            .size_full()
            .bg(c(theme::BG))
            .font_family("JetBrains Mono")
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .child(TerminalElement { view: cx.entity() })
    }
}

struct TerminalElement {
    view: Entity<TerminalView>,
}

impl IntoElement for TerminalElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

struct Painted {
    lines: Vec<(gpui::ShapedLine, gpui::Point<Pixels>)>,
    quads: Vec<gpui::PaintQuad>,
    cursor: Option<gpui::PaintQuad>,
}

fn named(n: NamedColor, fg: bool) -> Hsla {
    let hex = match n {
        NamedColor::Black => 0x1e1f22,
        NamedColor::Red => 0xf75464,
        NamedColor::Green => 0x6aab73,
        NamedColor::Yellow => 0xe0b050,
        NamedColor::Blue => 0x56a8f5,
        NamedColor::Magenta => 0xc77dbb,
        NamedColor::Cyan => 0x2aacb8,
        NamedColor::White => 0xbcbec4,
        NamedColor::BrightBlack => 0x6f737a,
        NamedColor::BrightRed => 0xff7b85,
        NamedColor::BrightGreen => 0x8fd694,
        NamedColor::BrightYellow => 0xf2d27a,
        NamedColor::BrightBlue => 0x7fbcff,
        NamedColor::BrightMagenta => 0xe0a3d8,
        NamedColor::BrightCyan => 0x6fd6de,
        NamedColor::BrightWhite => 0xffffff,
        NamedColor::Background => theme::BG,
        NamedColor::Foreground | NamedColor::BrightForeground => theme::FG,
        NamedColor::DimForeground => theme::FG_DIM,
        NamedColor::Cursor => theme::CURSOR,
        _ => if fg { theme::FG } else { theme::BG },
    };
    c(hex)
}

fn rgb(r: Rgb) -> Hsla {
    c(((r.r as u32) << 16) | ((r.g as u32) << 8) | r.b as u32)
}

/// xterm 256-color palette.
fn indexed(i: u8) -> Hsla {
    match i {
        0..=15 => {
            let n = [
                NamedColor::Black, NamedColor::Red, NamedColor::Green, NamedColor::Yellow, NamedColor::Blue, NamedColor::Magenta,
                NamedColor::Cyan, NamedColor::White, NamedColor::BrightBlack, NamedColor::BrightRed, NamedColor::BrightGreen,
                NamedColor::BrightYellow, NamedColor::BrightBlue, NamedColor::BrightMagenta, NamedColor::BrightCyan, NamedColor::BrightWhite,
            ][i as usize];
            named(n, true)
        }
        16..=231 => {
            let v = i - 16;
            let comp = |x: u8| if x == 0 { 0 } else { 55 + x * 40 };
            rgb(Rgb { r: comp(v / 36), g: comp((v / 6) % 6), b: comp(v % 6) })
        }
        _ => {
            let g = 8 + (i - 232) * 10;
            rgb(Rgb { r: g, g, b: g })
        }
    }
}

fn color(col: AColor, colors: &alacritty_terminal::term::color::Colors, fg: bool) -> Hsla {
    match col {
        AColor::Spec(r) => rgb(r),
        AColor::Indexed(i) => colors[i as usize].map(rgb).unwrap_or_else(|| indexed(i)),
        AColor::Named(n) => colors[n].map(rgb).unwrap_or_else(|| named(n, fg)),
    }
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = Painted;

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), window: &mut Window, cx: &mut App) -> Painted {
        let style = window.text_style();
        let font = style.font();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let lh = window.line_height();
        let probe = window.text_system().shape_line("M".into(), font_size, &[TextRun { len: 1, font: font.clone(), color: c(theme::FG), background_color: None, underline: None, strikethrough: None }], None);
        let cw = probe.width;
        let origin = point(bounds.left() + px(4.), bounds.top() + px(2.));
        let cols = (((bounds.size.width - px(8.)) / cw).floor().max(2.)) as u16;
        let rows = (((bounds.size.height - px(4.)) / lh).floor().max(1.)) as u16;
        let focused = self.view.read(cx).focus.is_focused(window);

        self.view.update(cx, |v, _| {
            v.cell = (cw, lh);
            v.origin = origin;
            v.resize(cols, rows);
            let term = v.term.lock();
            let content = term.renderable_content();
            let offset = content.display_offset as i32;
            let sel = content.selection;
            let colors = content.colors;
            let mut rows_text: Vec<Vec<(char, Hsla, Hsla, bool)>> = vec![Vec::new(); v.rows as usize];
            for cell in content.display_iter {
                let row = (cell.point.line.0 + offset) as usize;
                if row >= rows_text.len() || cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                    continue;
                }
                let (mut fg, mut bg) = (color(cell.fg, colors, true), color(cell.bg, colors, false));
                if cell.flags.contains(Flags::INVERSE) {
                    std::mem::swap(&mut fg, &mut bg);
                }
                if cell.flags.contains(Flags::DIM) {
                    fg.a *= 0.7;
                }
                let selected = sel.is_some_and(|s| s.contains(cell.point));
                let mut ch = cell.c;
                if ch == '\0' || cell.flags.contains(Flags::HIDDEN) {
                    ch = ' ';
                }
                rows_text[row].push((ch, fg, bg, selected));
                // combining marks (Vietnamese diacritics) ride on the base character
                if let Some(extra) = cell.zerowidth() {
                    for z in extra {
                        rows_text[row].push((*z, fg, bg, selected));
                    }
                }
            }
            let cursor_point = content.cursor.point;
            let cursor_visible = content.mode.contains(TermMode::SHOW_CURSOR) && offset == 0;
            drop(term);

            let mut lines = Vec::new();
            let mut quads = Vec::new();
            let bg_default = c(theme::BG);
            for (r, cells) in rows_text.iter().enumerate() {
                let y = origin.y + lh * r as f32;
                let mut text = String::new();
                let mut runs: Vec<TextRun> = Vec::new();
                let mut col = 0usize;
                for (ch, fg, bg, selected) in cells {
                    let is_mark = unicode_segmentation::UnicodeSegmentation::graphemes(format!("a{ch}").as_str(), true).count() == 1;
                    if !is_mark {
                        let bgc = if *selected { c(theme::SELECTION) } else { *bg };
                        if bgc != bg_default {
                            quads.push(fill(Bounds::new(point(origin.x + cw * col as f32, y), size(cw, lh)), bgc));
                        }
                        col += 1;
                    }
                    let len = ch.len_utf8();
                    match runs.last_mut() {
                        Some(last) if last.color == *fg => last.len += len,
                        _ => runs.push(TextRun { len, font: font.clone(), color: *fg, background_color: None, underline: None, strikethrough: None }),
                    }
                    text.push(*ch);
                }
                let trimmed = text.trim_end().len();
                if trimmed == 0 {
                    continue;
                }
                // drop trailing blanks from the runs too
                let mut keep = trimmed;
                let mut out_runs = Vec::new();
                for run in runs {
                    if keep == 0 {
                        break;
                    }
                    let l = run.len.min(keep);
                    keep -= l;
                    out_runs.push(TextRun { len: l, ..run });
                }
                text.truncate(trimmed);
                let shaped = window.text_system().shape_line(SharedString::from(text), font_size, &out_runs, None);
                lines.push((shaped, point(origin.x, y)));
            }
            let cursor = cursor_visible.then(|| {
                let x = origin.x + cw * cursor_point.column.0 as f32;
                let y = origin.y + lh * cursor_point.line.0 as f32;
                if focused {
                    fill(Bounds::new(point(x, y), size(cw, lh)), Hsla { a: 0.55, ..c(theme::CURSOR) })
                } else {
                    gpui::outline(Bounds::new(point(x, y), size(cw, lh)), c(theme::FG_DIM), gpui::BorderStyle::Solid)
                }
            });
            Painted { lines, quads, cursor }
        })
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), p: &mut Painted, window: &mut Window, cx: &mut App) {
        let focus = self.view.read(cx).focus.clone();
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.view.clone()), cx);
        let lh = window.line_height();
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            for q in p.quads.drain(..) {
                window.paint_quad(q);
            }
            if let Some(cur) = p.cursor.take() {
                window.paint_quad(cur);
            }
            for (line, at) in p.lines.drain(..) {
                let _ = line.paint(at, lh, window, cx);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::key_bytes;
    use gpui::Keystroke;

    fn ks(s: &str) -> Keystroke {
        Keystroke::parse(s).unwrap()
    }

    /// A real shell in a PTY: input goes in, Vietnamese output lands in the grid.
    #[test]
    fn shell_roundtrip_in_pty() {
        use alacritty_terminal::event::Notify;
        use alacritty_terminal::grid::Dimensions;
        let dir = std::env::temp_dir();
        let pty = super::Pty::spawn_with(&dir, Some(alacritty_terminal::tty::Shell::new("/bin/sh".into(), vec![]))).unwrap();
        pty.notifier.notify(b"echo \"xin ch\xc3\xa0o $((6*7))\"\r".to_vec());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let text: String = {
                let term = pty.term.lock();
                let grid = term.grid();
                (0..grid.screen_lines())
                    .map(|l| {
                        let row = &grid[alacritty_terminal::index::Line(l as i32)];
                        (0..grid.columns()).map(|c| row[alacritty_terminal::index::Column(c)].c).collect::<String>()
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            if text.contains("xin chào 42") {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "không thấy output trong terminal:\n{text}");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    #[test]
    fn keys_to_vt_sequences() {
        assert_eq!(key_bytes(&ks("ctrl-c"), false), Some(vec![3]));
        assert_eq!(key_bytes(&ks("enter"), false), Some(b"\r".to_vec()));
        assert_eq!(key_bytes(&ks("up"), false), Some(b"\x1b[A".to_vec()));
        assert_eq!(key_bytes(&ks("up"), true), Some(b"\x1bOA".to_vec()));
        assert_eq!(key_bytes(&ks("ctrl-left"), false), Some(b"\x1b[1;5D".to_vec()));
        assert_eq!(key_bytes(&ks("alt-b"), false), Some(b"\x1bb".to_vec()));
        assert_eq!(key_bytes(&ks("backspace"), false), Some(vec![0x7f]));
        assert_eq!(key_bytes(&ks("a"), false), None); // plain text: input handler
    }
}
