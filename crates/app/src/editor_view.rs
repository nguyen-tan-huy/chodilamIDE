//! One open file: `editor_core::Editor` + tree-sitter highlighting, drawn by a custom
//! element that only shapes the visible lines. Text input (incl. IME preedit) comes
//! through `EntityInputHandler`, which speaks UTF-16 offsets.
use crate::input::{InputEvent, TextInput};
use crate::theme;
use editor_core::{Buffer, Command, Editor, Effect};
use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, Focusable, GlobalElementId, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollWheelEvent, ShapedLine, SharedString, Style, TextRun,
    UTF16Selection, UnderlineStyle, Window, fill, point, prelude::*, px, relative, size,
};
use std::ops::Range;
use std::path::PathBuf;
use syntax::{Highlighter, Lang};

pub struct EditorView {
    pub editor: Editor,
    focus: FocusHandle,
    highlighter: Option<Highlighter>,
    pub(crate) scroll_y: Pixels,
    /// horizontal scroll of the text area (long lines)
    pub(crate) scroll_x: Pixels,
    /// keep the caret horizontally visible on the next paint
    follow_x: bool,
    layout: Option<Layout>,
    selecting: bool,
    pub(crate) tab_width: usize,
    pub(crate) last_find: Option<String>,
    find_input: Option<Entity<TextInput>>,
    /// Vim keymap (Settings → Kiểu phím: Vim); None = IntelliJ keys
    pub(crate) vim: Option<editor_core::vim::Vim>,
    /// Ctrl keys that go to Vim instead of the IDE
    pub(crate) vim_keys: Vec<String>,
    /// ':' command line
    pub(crate) vim_cmdline: Option<Entity<TextInput>>,
    /// the last search was '?' (n goes backwards)
    pub(crate) vim_search_back: bool,
    /// Ctrl+R: the "replace with" field under the find field
    pub(crate) replace_input: Option<Entity<TextInput>>,
    /// Message for the status bar (save errors, search misses...).
    pub message: Option<String>,
    /// Set when a file is saved, so the workspace can react (e.g. reload the model on pom.xml).
    pub just_saved: bool,
    /// Center the cursor line on the next paint (after goto from a picker).
    pending_center: bool,
    // ---- language server (see lsp_editor.rs) ----
    pub(crate) lsp: Option<crate::lsp_editor::LspDoc>,
    pub(crate) diagnostics: Vec<crate::lsp_editor::Diag>,
    pub(crate) completion: Option<crate::lsp_editor::Completion>,
    pub(crate) signature: Option<crate::lsp_editor::SignatureHelp>,
    /// id of the last JSF completion asked from the workspace
    pub(crate) jsf_request: i64,
    /// caret position of the latest JSF/Spring completion request (older answers are dropped)
    pub(crate) jsf_at: usize,
    pub(crate) hover: Option<String>,
    pub(crate) actions_menu: Option<crate::lsp_editor::ActionsMenu>,
    pub(crate) rename_input: Option<Entity<TextInput>>,
    /// the name being renamed (Shift+F6), for the JSF follow-up in .xhtml pages
    pub(crate) rename_old: String,
    /// Class files from jars (jdt://) are shown read-only.
    pub read_only: bool,
    pub title_override: Option<String>,
    // ---- run/debug (set by the workspace) ----
    /// 0-based lines with a breakpoint
    pub breakpoints: Vec<usize>,
    /// the subset of `breakpoints` that have a condition (drawn orange)
    pub conditional_breakpoints: Vec<usize>,
    /// 0-based line where the debugger is stopped in this file
    pub exec_line: Option<usize>,
    /// ▶ menu open for a `main`/test on this line
    gutter_menu: Option<usize>,
    /// debugger stopped: grey "name: value" hints after these (0-based) lines
    pub inline_values: Vec<(usize, String)>,
    /// "◇ N xhtml" after the members of a JSF bean (line, text)
    pub lens: Vec<(usize, String)>,
    /// the file at HEAD (git), None outside git / new file: no change bars
    pub git_base: Option<String>,
    /// change bars per line, recomputed when the buffer version changes
    pub git_marks: Vec<(usize, vcs::LineMark)>,
    pub git_marks_version: usize,
    /// Annotate: "date hash author" per line, drawn in the gutter
    pub annotations: Vec<String>,
    /// bookmarked lines (F11)
    pub bookmarks: Vec<usize>,
    /// Run with Coverage: (line, 0 missed / 1 partly / 2 covered)
    pub coverage: Vec<(usize, u8)>,
    /// semantic highlighting from jdtls (fields, statics, annotations, deprecated)
    pub(crate) semantic: Vec<(Range<usize>, crate::lsp_editor::Sem)>,
    pub(crate) sem_version: Option<usize>,
    pub(crate) sem_inflight: bool,
    /// debugger stopped: hovering an expression asks for its value
    pub debug_active: bool,
    hover_probe: Option<String>,
    hover_gen: usize,
}

pub enum EditorEvent {
    /// Jump to LSP locations (definition); `Value` = Location.
    Goto(Vec<serde_json::Value>),
    /// Find Usages on a Java file: jdtls' references plus, for JSF bean members, the .xhtml EL
    /// usages (the workspace adds those); `uri`/`line`/`col` = the cursor (utf-16).
    Usages { locs: Vec<serde_json::Value>, uri: String, line: usize, col: usize },
    /// .xhtml: Ctrl+B at byte offset (EL → Java, include/template → file, composite tag).
    JsfGoto(usize),
    /// .xhtml: completion at byte offset (the popup is already open, waiting for items).
    JsfComplete { at: usize, explicit: bool },
    /// .xhtml: hover (Ctrl+Q) at byte offset.
    JsfHover(usize),
    /// Spring: application.properties/yml or `${…}` in Java - Ctrl+B, completion, Ctrl+Q.
    SpringGoto(usize),
    SpringComplete { at: usize, explicit: bool },
    SpringHover(usize),
    /// Several LSP locations to choose from (implementations).
    Locations { title: String, locs: Vec<serde_json::Value> },
    /// Type / call hierarchy at a position (`params` = TextDocumentPositionParams).
    Hierarchy { kind: Hierarchy, params: serde_json::Value },
    /// A WorkspaceEdit to apply across files (rename).
    ApplyEdit(serde_json::Value),
    /// A rename was applied in Java: `old` → `new` (the workspace updates JSF pages).
    Renamed { old: String, new: String },
    /// A code action / command chosen from the Alt+Enter menu.
    RunAction(serde_json::Value),
    /// Flattened document symbols (name, detail, line, utf16 col, depth) for Ctrl+F12.
    Structure(Vec<(String, String, usize, usize, usize)>),
    /// Gutter click / Ctrl+F8 on a 0-based line.
    ToggleBreakpoint(usize),
    /// Right-click in the gutter / Ctrl+Shift+F8: edit the breakpoint's condition.
    EditBreakpoint(usize),
    /// ▶ next to `main`: run or debug that class.
    RunMain { main_class: String, debug: bool },
    /// Mouse rested on an expression while the debugger is stopped.
    DebugHover { expr: String, at: gpui::Point<Pixels> },
    /// Click / typing / scrolling: close transient popups (value popup).
    DismissPopups,
    /// Right-click in the text: show the editor context menu.
    ContextMenu { at: gpui::Point<Pixels>, line: usize },
    /// Vim `:e path`: open a file (relative to this file's folder or the project).
    OpenPath(String),
    /// ▶ next to a request of an .http file (or Ctrl+Enter in it).
    RunHttp(usize),
    /// ▶ next to a test method / test class.
    RunTest { class: String, method: Option<String>, debug: bool },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Hierarchy {
    /// Ctrl+H: supertypes and subtypes of the class
    Types,
    /// Ctrl+Alt+H: callers of the method (incoming, 3 levels)
    Calls,
    /// Ctrl+U: the super method / super class
    Super,
}

impl gpui::EventEmitter<EditorEvent> for EditorView {}

/// What the last paint laid out - used for mouse hit-testing and IME candidate placement.
struct Layout {
    bounds: Bounds<Pixels>,
    /// x of the text's column 0 (already shifted by scroll_x; may be left of gutter_end)
    text_x: Pixels,
    /// where the gutter ends and the (clipped) text area begins
    gutter_end: Pixels,
    /// x offsets (from bounds.left) of the ▶ column: [start, end)
    run_col: (Pixels, Pixels),
    line_height: Pixels,
    first_line: usize,
    y_offset: Pixels,
    lines: Vec<VisibleLine>,
}

struct VisibleLine {
    /// buffer line (rows skip collapsed regions)
    line: usize,
    start: usize,
    map: TabMap,
    shaped: ShapedLine,
}

/// Tabs are drawn as spaces; maps byte offsets within a line to display-string offsets.
#[derive(Clone)]
struct TabMap {
    tabs: Vec<usize>,
    width: usize,
}

impl TabMap {
    fn new(line: &str, width: usize) -> (Self, String) {
        let tabs: Vec<usize> = line.match_indices('\t').map(|(i, _)| i).collect();
        let display = if tabs.is_empty() { line.to_string() } else { line.replace('\t', &" ".repeat(width)) };
        (Self { tabs, width }, display)
    }

    fn to_display(&self, byte: usize) -> usize {
        byte + self.tabs.iter().take_while(|&&t| t < byte).count() * (self.width - 1)
    }

    fn to_byte(&self, disp: usize) -> usize {
        let mut extra = 0;
        for &t in &self.tabs {
            let t_disp = t + extra;
            if disp <= t_disp {
                break;
            }
            if disp < t_disp + self.width {
                // inside the tab's spaces: snap to the nearer side
                return if disp - t_disp < self.width / 2 { t } else { t + 1 };
            }
            extra += self.width - 1;
        }
        disp - extra
    }
}

impl EditorView {
    pub fn new(buffer: Buffer, tab_width: usize, cx: &mut Context<Self>) -> Self {
        let lang = buffer.path.as_deref().and_then(Lang::from_path);
        Self::with_lang(buffer, tab_width, lang, cx)
    }

    /// For buffers without a path (class files from jars).
    pub fn new_with_lang(buffer: Buffer, tab_width: usize, lang: Lang, cx: &mut Context<Self>) -> Self {
        Self::with_lang(buffer, tab_width, Some(lang), cx)
    }

    fn with_lang(buffer: Buffer, tab_width: usize, lang: Option<Lang>, cx: &mut Context<Self>) -> Self {
        let highlighter = lang.and_then(Highlighter::new);
        let mut editor = Editor::new(buffer);
        // keep the file's own indentation style for Tab
        if editor.buffer.text().lines().take(200).any(|l| l.starts_with('\t')) {
            editor.indent = "\t".into();
        }
        // comment syntax for Ctrl+/ and Ctrl+Shift+/
        let ext = editor.buffer.path.as_deref().and_then(|p| p.extension()).and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        match ext.as_str() {
            "xml" | "xhtml" | "html" | "htm" | "jsp" | "jsf" | "pom" => {
                editor.line_comment = None;
                editor.block_comment = Some(("<!--".into(), "-->".into()));
            }
            "properties" | "yml" | "yaml" | "sh" | "toml" | "py" | "conf" | "env" => {
                editor.line_comment = Some("#".into());
                editor.block_comment = None;
            }
            "sql" => editor.line_comment = Some("--".into()),
            _ => {}
        }
        Self {
            editor,
            focus: cx.focus_handle(),
            highlighter,
            scroll_y: px(0.),
            scroll_x: px(0.),
            follow_x: false,
            layout: None,
            selecting: false,
            tab_width,
            last_find: None,
            find_input: None,
            replace_input: None,
            vim: None,
            vim_keys: Vec::new(),
            vim_cmdline: None,
            vim_search_back: false,
            message: None,
            just_saved: false,
            pending_center: false,
            lsp: None,
            diagnostics: Vec::new(),
            completion: None,
            signature: None,
            jsf_request: 0,
            jsf_at: 0,
            hover: None,
            actions_menu: None,
            rename_input: None,
            rename_old: String::new(),
            read_only: false,
            title_override: None,
            breakpoints: Vec::new(),
            conditional_breakpoints: Vec::new(),
            exec_line: None,
            gutter_menu: None,
            inline_values: Vec::new(),
            lens: Vec::new(),
            git_base: None,
            git_marks: Vec::new(),
            git_marks_version: usize::MAX,
            annotations: Vec::new(),
            bookmarks: Vec::new(),
            coverage: Vec::new(),
            semantic: Vec::new(),
            sem_version: None,
            sem_inflight: false,
            debug_active: false,
            hover_probe: None,
            hover_gen: 0,
        }
    }

    pub fn path(&self) -> Option<&PathBuf> {
        self.editor.buffer.path.as_ref()
    }

    pub fn title(&self) -> String {
        if let Some(t) = &self.title_override {
            return t.clone();
        }
        self.path().map(|p| crate::tree::file_name(p)).unwrap_or_else(|| "untitled".into())
    }

    pub fn is_dirty(&self) -> bool {
        self.editor.buffer.is_dirty()
    }

    // ---- commands ------------------------------------------------------------

    pub fn run(&mut self, cmd: Command, window: &mut Window, cx: &mut Context<Self>) {
        use Command::*;
        self.message = None;
        cx.emit(EditorEvent::DismissPopups);
        self.hover = None;
        if self.read_only && matches!(cmd, Backspace | Delete | DeleteWordLeft | Newline | Tab | DuplicateLine | DeleteLine | Undo | Redo | Cut | Paste | Save) {
            self.message = Some("File chỉ đọc (class trong thư viện)".into());
            cx.notify();
            return;
        }
        // moving the cursor away closes the completion popup; deleting refilters it
        if self.completion.is_some() {
            if matches!(cmd, Backspace | DeleteWordLeft) {
                self.editor.run(cmd);
                self.refilter_completion(cx);
                self.scroll_to_cursor();
                cx.notify();
                return;
            }
            self.completion = None;
        }
        // leaving the call being typed closes parameter info
        if matches!(cmd, Up | Down | PageUp | PageDown | DocStart | DocEnd | Newline | Undo | Redo) {
            self.signature = None;
        }
        match self.editor.run(cmd) {
            Effect::None => {}
            Effect::CopyToClipboard(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
            Effect::RequestPaste => {
                if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) {
                    self.editor.paste(&text);
                }
            }
            Effect::RequestSave => self.save_formatted(cx),
            Effect::OpenFind => self.open_find(window, cx),
            Effect::FindNext => self.find_next(),
        }
        self.scroll_to_cursor();
        cx.notify();
    }

    fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sel = self.editor.selection();
        let initial = if !sel.is_empty() && !self.editor.buffer.slice(sel.clone()).contains('\n') {
            Some(self.editor.buffer.slice(sel))
        } else {
            self.last_find.clone()
        };
        let input = match &self.find_input {
            Some(i) => i.clone(),
            None => {
                let i = cx.new(|cx| TextInput::new("Tìm trong file…", cx));
                cx.subscribe(&i, |this, _, ev: &InputEvent, cx| {
                    let InputEvent::Changed(q) = ev;
                    this.last_find = (!q.is_empty()).then(|| q.clone());
                    // incremental: search again from where the current match starts
                    let from = this.editor.selection().start;
                    this.find_from(from);
                    this.scroll_to_cursor();
                    cx.notify();
                })
                .detach();
                self.find_input = Some(i.clone());
                i
            }
        };
        if let Some(q) = initial {
            input.update(cx, |i, cx| i.set_text(&q, cx));
        }
        window.focus(&input.read(cx).focus_handle(cx));
    }

    /// Ctrl+R: find bar + replace field (focus goes to the find field like IntelliJ).
    pub(crate) fn open_replace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        self.open_find(window, cx);
        if self.replace_input.is_none() {
            self.replace_input = Some(cx.new(|cx| TextInput::new("Thay bằng…", cx)));
        }
        cx.notify();
    }

    fn replacement(&self, cx: &Context<Self>) -> String {
        self.replace_input.as_ref().map(|i| i.read(cx).content.clone()).unwrap_or_default()
    }

    /// Replaces the current match (when the selection is one) and moves to the next.
    pub(crate) fn replace_next(&mut self, cx: &mut Context<Self>) {
        let Some(q) = self.last_find.clone().filter(|q| !q.is_empty()) else { return };
        let sel = self.editor.selection();
        let is_match = !sel.is_empty() && self.editor.buffer.slice(sel.clone()).to_lowercase() == q.to_lowercase();
        if is_match {
            let rep = self.replacement(cx);
            self.editor.buffer.break_group();
            self.editor.insert(&rep);
            self.editor.buffer.break_group();
        }
        self.find_next();
        self.scroll_to_cursor();
        cx.notify();
    }

    /// Replaces every match in the file as one undo step.
    pub(crate) fn replace_all(&mut self, cx: &mut Context<Self>) {
        let Some(q) = self.last_find.clone().filter(|q| !q.is_empty()) else { return };
        let rep = self.replacement(cx);
        let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
        let mut from = 0;
        while let Some(r) = self.editor.buffer.find(&q, from, false) {
            if r.start < from {
                break; // wrapped around
            }
            from = r.end.max(r.start + 1);
            ranges.push(r);
        }
        self.editor.buffer.break_group();
        for r in ranges.iter().rev() {
            self.editor.anchor = r.start;
            self.editor.head = r.end;
            self.editor.insert(&rep);
        }
        self.editor.buffer.break_group();
        self.message = Some(format!("Đã thay {} chỗ", ranges.len()));
        cx.notify();
    }

    pub(crate) fn scroll_to_cursor_pub(&mut self) {
        self.scroll_to_cursor();
    }

    pub(crate) fn open_find_pub(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_find(window, cx);
    }

    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find_input = None;
        self.replace_input = None;
        window.focus(&self.focus);
        cx.notify();
    }

    /// Jumps to a 0-based line/byte column (from Find in Files, later LSP locations).
    pub fn goto(&mut self, line: usize, col: usize) {
        let b = &self.editor.buffer;
        let line = line.min(b.line_count().saturating_sub(1));
        let start = b.line_start(line);
        let at = (start + col).min(b.line_end(line));
        self.editor.set_cursor(at, false);
        self.pending_center = true;
    }

    pub(crate) fn request_center(&mut self) {
        self.pending_center = true;
    }

    fn find_next(&mut self) {
        self.find_from(self.editor.selection().end);
    }

    fn find_from(&mut self, from: usize) {
        let Some(q) = self.last_find.clone() else { return };
        match self.editor.buffer.find(&q, from, false) {
            Some(r) => {
                self.editor.anchor = r.start;
                self.editor.head = r.end;
            }
            None => self.message = Some(format!("Không tìm thấy \"{q}\"")),
        }
    }

    pub fn save(&mut self) {
        match self.editor.buffer.save() {
            Ok(()) => {
                self.message = Some("Đã lưu".into());
                self.just_saved = true;
            }
            Err(e) => self.message = Some(format!("Lưu lỗi: {e}")),
        }
    }

    fn scroll_to_cursor(&mut self) {
        self.follow_x = true;
        let Some(l) = &self.layout else { return };
        let lh = l.line_height;
        // rows, not lines: collapsed regions take no space
        let line = self.editor.buffer.row_of_line(self.editor.buffer.line_of(self.editor.head));
        let visible = ((l.bounds.size.height / lh).floor() as usize).max(1);
        self.editor.page_lines = visible.saturating_sub(2).max(1);
        let top = (self.scroll_y / lh).ceil() as usize;
        let margin = 2.min(visible / 2);
        if line < top + margin {
            self.scroll_y = lh * line.saturating_sub(margin) as f32;
        } else if line + margin + 1 > top + visible {
            self.scroll_y = lh * (line + margin + 1).saturating_sub(visible) as f32;
        }
    }

    /// Window position just below the cursor (for popups), from the last paint.
    pub(crate) fn cursor_window_point(&self, at: usize) -> Option<Point<Pixels>> {
        let l = self.layout.as_ref()?;
        let line = self.editor.buffer.line_of(at);
        let idx = l.lines.iter().position(|v| v.line == line)?;
        let vl = l.lines.get(idx)?;
        let x = vl.shaped.x_for_index(vl.map.to_display(at - vl.start));
        Some(point(l.bounds.left() + l.text_x + x, l.bounds.top() - l.y_offset + l.line_height * (idx + 1) as f32))
    }

    /// ▶ target of a line: `Some(None)` = the test class, `Some(Some(m))` = test method m.
    pub(crate) fn test_at_line(&self, line: usize) -> Option<Option<String>> {
        let b = &self.editor.buffer;
        let t = b.line_text(line);
        let trimmed = t.trim_start();
        if ["@Test", "@ParameterizedTest", "@RepeatedTest", "@TestFactory"].iter().any(|a| trimmed.starts_with(a) && !trimmed[a.len()..].starts_with(|c: char| c.is_alphanumeric())) {
            // the method declared below the annotation(s)
            for l in line + 1..(line + 6).min(b.line_count()) {
                let decl = b.line_text(l);
                if decl.trim_start().starts_with('@') {
                    continue;
                }
                let name = decl.split('(').next()?.split_whitespace().last()?.to_string();
                return Some(Some(name));
            }
            return None;
        }
        let is_class_decl = trimmed.contains("class ") && !trimmed.starts_with("//") && !trimmed.starts_with('*');
        if is_class_decl && self.has_tests() {
            return Some(None);
        }
        None
    }

    fn has_tests(&self) -> bool {
        let b = &self.editor.buffer;
        (0..b.line_count().min(3000)).any(|i| b.line_text(i).trim_start().starts_with("@Test"))
    }

    fn is_main_line(&self, line: usize) -> bool {
        let t = self.editor.buffer.line_text(line);
        t.contains("static void main(") || t.contains("static void main (")
    }

    /// `package a.b;` + file stem → "a.b.Stem".
    pub fn class_name(&self) -> Option<String> {
        let stem = self.path()?.file_stem()?.to_string_lossy().to_string();
        let b = &self.editor.buffer;
        for i in 0..b.line_count().min(200) {
            let l = b.line_text(i);
            if let Some(rest) = l.trim_start().strip_prefix("package ") {
                return Some(format!("{}.{stem}", rest.trim().trim_end_matches(';').trim()));
            }
        }
        Some(stem)
    }

    /// Debug hover: after the mouse rests 350 ms on the same expression, ask for its value.
    fn probe_hover(&mut self, p: gpui::Point<Pixels>, cx: &mut Context<Self>) {
        let expr = self.expr_under(p);
        if expr == self.hover_probe {
            return;
        }
        self.hover_probe = expr.clone();
        self.hover_gen += 1;
        let Some(expr) = expr else { return };
        let generation = self.hover_gen;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(350)).await;
            let _ = this.update(cx, |v, cx| {
                if v.hover_gen == generation && v.debug_active {
                    cx.emit(EditorEvent::DebugHover { expr, at: p });
                }
            });
        })
        .detach();
    }

    /// The expression under a window point, only when the point is over actual text.
    fn expr_under(&self, p: gpui::Point<Pixels>) -> Option<String> {
        let l = self.layout.as_ref()?;
        let rel_y = p.y - l.bounds.top() + l.y_offset;
        let idx = (rel_y / l.line_height).floor();
        if idx < 0. {
            return None;
        }
        let vl = l.lines.get(idx as usize)?;
        let x = p.x - l.bounds.left() - l.text_x;
        if x < px(0.) || x > vl.shaped.width {
            return None;
        }
        let disp = vl.shaped.closest_index_for_x(x);
        let col = vl.map.to_byte(disp);
        let line = self.editor.buffer.line_of(vl.start);
        crate::debug_values::expr_at(&self.editor.buffer.line_text(line), col)
    }

    fn on_right_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(l) = &self.layout else { return };
        let at = self.offset_for_point(ev.position);
        let line = self.editor.buffer.line_of(at);
        if ev.position.x - l.bounds.left() < l.gutter_end - px(2.) {
            cx.emit(EditorEvent::EditBreakpoint(line));
            return;
        }
        // like IntelliJ: right-click outside the selection moves the caret there first
        window.focus(&self.focus);
        if !self.editor.selection().contains(&at) {
            self.editor.set_cursor(at, false);
        }
        self.completion = None;
        self.hover = None;
        cx.emit(EditorEvent::ContextMenu { at: ev.position, line });
        cx.notify();
    }

    pub(crate) fn edit_breakpoint_at_cursor(&mut self, cx: &mut Context<Self>) {
        let line = self.editor.buffer.line_of(self.editor.head);
        cx.emit(EditorEvent::EditBreakpoint(line));
    }

    pub(crate) fn toggle_breakpoint_at_cursor(&mut self, cx: &mut Context<Self>) {
        let line = self.editor.buffer.line_of(self.editor.head);
        cx.emit(EditorEvent::ToggleBreakpoint(line));
    }

    pub(crate) fn render_gutter_menu(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        use gpui::{anchored, deferred, div};
        let line = self.gutter_menu?;
        let l = self.layout.as_ref()?;
        let idx = l.lines.iter().position(|v| v.line == line)?;
        let at = point(l.bounds.left() + px(4.), l.bounds.top() - l.y_offset + l.line_height * (idx + 1) as f32);
        let main_class = self.class_name()?;
        let test = self.test_at_line(line);
        let item = |label: String, key: &'static str, debug: bool, main_class: String| {
            let test = test.clone();
            div()
                .id(SharedString::from(label.clone()))
                .flex()
                .justify_between()
                .gap_4()
                .px_2()
                .hover(|s| s.bg(theme::ed(theme::ROW_HOVER)))
                .child(label)
                .child(gpui::div().text_color(theme::ed(theme::FG_DIM)).child(key))
                .on_mouse_down(MouseButton::Left, cx.listener(move |v, _, _, cx| {
                    cx.stop_propagation();
                    v.gutter_menu = None;
                    match test.clone() {
                        Some(method) => cx.emit(EditorEvent::RunTest { class: main_class.clone(), method, debug }),
                        None => cx.emit(EditorEvent::RunMain { main_class: main_class.clone(), debug }),
                    }
                    cx.notify();
                }))
        };
        let what = match &test {
            Some(Some(m)) => format!(" '{m}'"),
            Some(None) => " tất cả test".to_string(),
            None => String::new(),
        };
        Some(
            deferred(anchored().position(at).child(
                div()
                    .min_w(px(220.))
                    .py_1()
                    .bg(theme::ed(theme::POPUP_BG))
                    .border_1()
                    .border_color(theme::ed(theme::BORDER))
                    .shadow_lg().occlude().rounded_lg()
                    .text_color(theme::ed(theme::FG))
                    .child(item(format!("▶ Run{what}"), "", false, main_class.clone()))
                    .child(item(format!("🐞 Debug{what}"), "", true, main_class)),
            ))
            .into_any_element(),
        )
    }

    /// Buffer lines of the last paint (tests: what folding shows).
    #[cfg(test)]
    pub(crate) fn laid_out_lines(&self) -> Vec<usize> {
        self.layout.as_ref().map(|l| l.lines.iter().map(|v| v.line).collect()).unwrap_or_default()
    }

    pub(crate) fn line_height(&self) -> Option<Pixels> {
        self.layout.as_ref().map(|l| l.line_height)
    }

    // ---- mouse ---------------------------------------------------------------

    fn offset_for_point(&self, p: Point<Pixels>) -> usize {
        let Some(l) = &self.layout else { return self.editor.head };
        let b = &self.editor.buffer;
        let rel_y = p.y - l.bounds.top() + l.y_offset;
        if rel_y < px(0.) {
            return b.line_start(l.first_line);
        }
        let idx = (rel_y / l.line_height).floor() as usize;
        match l.lines.get(idx) {
            Some(vl) => {
                let disp = vl.shaped.closest_index_for_x(p.x - l.bounds.left() - l.text_x);
                vl.start + vl.map.to_byte(disp)
            }
            None => b.len(),
        }
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        cx.emit(EditorEvent::DismissPopups);
        let at = self.offset_for_point(ev.position);
        // gutter: ▶ on a main line opens the Run/Debug menu, anywhere else toggles a breakpoint
        if let Some(l) = &self.layout {
            let x = ev.position.x - l.bounds.left();
            if x < l.gutter_end - px(2.) {
                let line = self.editor.buffer.line_of(at);
                let in_run_col = x >= l.run_col.0 && x < l.run_col.1;
                let http = self.path().is_some_and(|p| crate::http_client::is_http_file(p)) && crate::http_client::is_request_line(&self.editor.buffer.line_text(line));
                if in_run_col && http {
                    self.gutter_menu = None;
                    cx.emit(EditorEvent::RunHttp(line));
                } else if in_run_col && (self.is_main_line(line) || self.test_at_line(line).is_some()) {
                    self.gutter_menu = Some(line);
                } else {
                    self.gutter_menu = None;
                    cx.emit(EditorEvent::ToggleBreakpoint(line));
                }
                cx.notify();
                return;
            }
        }
        self.gutter_menu = None;
        self.editor.unmark();
        match ev.click_count {
            2 => {
                let s = self.editor.buffer.word_left(self.editor.buffer.next_grapheme(at).min(self.editor.buffer.len()));
                let e = self.editor.buffer.word_right(at);
                self.editor.set_cursor(s, false);
                self.editor.set_cursor(e, true);
            }
            3 => {
                let line = self.editor.buffer.line_of(at);
                self.editor.set_cursor(self.editor.buffer.line_start(line), false);
                self.editor.set_cursor(self.editor.buffer.line_start(line + 1), true);
            }
            _ if ev.modifiers.control => {
                self.editor.set_cursor(at, false);
                self.goto_definition(cx);
            }
            // Alt+Click: one more caret (IntelliJ)
            _ if ev.modifiers.alt && !ev.modifiers.shift => {
                let cur = self.editor.anchor..self.editor.head;
                if !self.editor.extra.contains(&cur) {
                    self.editor.extra.push(cur);
                }
                self.editor.anchor = at;
                self.editor.head = at;
            }
            _ => {
                // click on the "⋯" after a collapsed header: expand it
                let line = self.editor.buffer.line_of(at);
                let past_end = self.layout.as_ref().and_then(|l| l.lines.iter().find(|v| v.line == line).map(|v| ev.position.x - l.bounds.left() - l.text_x > v.shaped.width + px(4.)));
                if past_end == Some(true) && self.editor.buffer.folds.iter().any(|(s, _)| *s == line) {
                    self.editor.buffer.folds.retain(|(s, _)| *s != line);
                    cx.notify();
                    return;
                }
                self.selecting = true;
                self.editor.set_cursor(at, ev.modifiers.shift);
            }
        }
        self.completion = None;
        self.signature = None;
        self.hover = None;
        cx.notify();
    }

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selecting && self.debug_active && ev.pressed_button.is_none() {
            self.probe_hover(ev.position, cx);
        }
        if self.selecting {
            let at = self.offset_for_point(ev.position);
            self.editor.set_cursor(at, true);
            cx.notify();
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.selecting = false;
    }

    fn on_scroll(&mut self, ev: &ScrollWheelEvent, window: &mut Window, cx: &mut Context<Self>) {
        cx.emit(EditorEvent::DismissPopups);
        self.hover_probe = None;
        let lh = self.layout.as_ref().map(|l| l.line_height).unwrap_or(window.line_height());
        let mut delta = ev.delta.pixel_delta(lh);
        // Shift+wheel scrolls sideways (mice without a horizontal wheel)
        if ev.modifiers.shift && delta.x == px(0.) {
            delta = point(delta.y, px(0.));
        }
        let max = lh * self.editor.buffer.visible_line_count().saturating_sub(3) as f32;
        self.scroll_y = (self.scroll_y - delta.y).clamp(px(0.), max.max(px(0.)));
        if delta.x != px(0.) {
            // as far as the widest visible line, plus a little room
            let (widest, area) = self
                .layout
                .as_ref()
                .map(|l| (l.lines.iter().map(|v| v.shaped.width).fold(px(0.), |a, b| a.max(b)), l.bounds.size.width - l.gutter_end))
                .unwrap_or((px(0.), px(0.)));
            let max_x = (widest - area + lh * 4.).max(px(0.));
            self.scroll_x = (self.scroll_x - delta.x).clamp(px(0.), max_x);
        }
        cx.notify();
    }

    // ---- UTF-16 helpers for the input handler ----------------------------------

    fn range_from_utf16(&self, r: &Range<usize>) -> Range<usize> {
        let b = &self.editor.buffer;
        b.utf16_to_byte(r.start)..b.utf16_to_byte(r.end)
    }

    fn range_to_utf16(&self, r: &Range<usize>) -> Range<usize> {
        let b = &self.editor.buffer;
        b.byte_to_utf16(r.start)..b.byte_to_utf16(r.end)
    }
}

impl Focusable for EditorView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EntityInputHandler for EditorView {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let r = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&r));
        Some(self.editor.buffer.slice(r))
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.editor.selection()),
            reversed: self.editor.head < self.editor.anchor,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.editor.marked.as_ref().map(|r| self.range_to_utf16(r))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.editor.unmark();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        if let Some(r) = range_utf16 {
            let r = self.range_from_utf16(&r);
            if r != self.editor.selection() {
                self.editor.extra.clear();
            }
            self.editor.marked = None;
            self.editor.anchor = r.start;
            self.editor.head = r.end;
        }
        self.editor.commit_text(text);
        self.vim_inserted(text);
        self.message = None;
        self.hover = None;
        self.after_typed(text, cx);
        self.scroll_to_cursor();
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        new_selected_utf16: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        if let Some(r) = range_utf16 {
            self.editor.marked = Some(self.range_from_utf16(&r));
        }
        self.editor.set_marked_text(text);
        if let (Some(sel), Some(m)) = (new_selected_utf16, self.editor.marked.clone()) {
            // selection is relative to the marked text, in UTF-16 units
            let to_byte = |u: usize| {
                let mut units = 0;
                for (i, ch) in text.char_indices() {
                    if units >= u {
                        return i;
                    }
                    units += ch.len_utf16();
                }
                text.len()
            };
            self.editor.anchor = m.start + to_byte(sel.start);
            self.editor.head = m.start + to_byte(sel.end);
        }
        self.scroll_to_cursor();
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let l = self.layout.as_ref()?;
        let r = self.range_from_utf16(&range_utf16);
        let line = self.editor.buffer.line_of(r.start);
        let idx = l.lines.iter().position(|v| v.line == line)?;
        let vl = l.lines.get(idx)?;
        let x0 = vl.shaped.x_for_index(vl.map.to_display(r.start - vl.start));
        let end_in_line = r.end.min(vl.start + self.editor.buffer.line_text(line).len());
        let x1 = vl.shaped.x_for_index(vl.map.to_display(end_in_line - vl.start));
        let top = l.bounds.top() - l.y_offset + l.line_height * idx as f32;
        let left = l.bounds.left() + l.text_x;
        Some(Bounds::from_corners(point(left + x0, top), point(left + x1.max(x0 + px(1.)), top + l.line_height)))
    }

    fn character_index_for_point(&mut self, p: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        let at = self.offset_for_point(p);
        Some(self.editor.buffer.byte_to_utf16(at))
    }
}

impl Render for EditorView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.lsp_sync();
        let mut key_context = gpui::KeyContext::default();
        key_context.add("Editor");
        if self.completion.as_ref().is_some_and(|c| !c.visible.is_empty()) {
            key_context.add("completing");
        }
        if self.actions_menu.is_some() {
            key_context.add("menu");
        }
        if self.signature.is_some() {
            key_context.add("sighelp");
        }
        if !self.editor.extra.is_empty() {
            key_context.add("multicaret");
        }
        if let Some(v) = &self.vim {
            key_context.add("vim");
            key_context.add(match v.mode {
                editor_core::vim::Mode::Insert => "vim_insert",
                editor_core::vim::Mode::Normal => "vim_normal",
                _ => "vim_visual",
            });
        }
        let mut root = gpui::div()
            .id("editor")
            .key_context(key_context)
            .track_focus(&self.focus)
            .size_full()
            .cursor(CursorStyle::IBeam)
            .bg(theme::ed(theme::BG))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_right_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_scroll_wheel(cx.listener(Self::on_scroll));
        root = crate::actions::register_editor_actions(root, cx);
        root = crate::lsp_editor::register_lsp_actions(root, cx);
        root = root
            .flex()
            .flex_col()
            .on_action(cx.listener(|this, _: &crate::actions::CloseFind, window, cx| this.close_find(window, cx)))
            .on_action(cx.listener(|this, _: &crate::actions::FindNextInBar, _, cx| {
                this.find_next();
                this.scroll_to_cursor();
                cx.notify()
            }))
            .on_action(cx.listener(|this, _: &crate::actions::OpenReplace, window, cx| this.open_replace(window, cx)))
            .on_action(cx.listener(|this, _: &crate::actions::VimCmdRun, window, cx| this.run_vim_cmdline(window, cx)))
            .on_action(cx.listener(|this, _: &crate::actions::VimCmdCancel, window, cx| this.close_vim_cmdline(window, cx)))
            .on_action(cx.listener(|this, _: &crate::actions::ReplaceNext, _, cx| this.replace_next(cx)))
            .on_action(cx.listener(|this, _: &crate::actions::ReplaceAll, _, cx| this.replace_all(cx)));
        if let Some(input) = &self.find_input {
            let q = self.last_find.clone().unwrap_or_default();
            let count = if q.is_empty() { String::new() } else { format!("{} kết quả", self.editor.buffer.count(&q, false)) };
            root = root.child(
                gpui::div()
                    .key_context("FindBar")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .bg(theme::ed(theme::PANEL_BG))
                    .border_b_1()
                    .border_color(theme::ed(theme::BORDER))
                    .child(gpui::div().w(px(360.)).child(input.clone()))
                    .child(gpui::div().text_color(theme::ed(theme::FG_DIM)).child(count))
                    .child(gpui::div().text_color(theme::ed(theme::FG_DIM)).child("Enter/F3: tiếp · Esc: đóng")),
            );
            if let Some(rep) = &self.replace_input {
                root = root.child(
                    gpui::div()
                        .key_context("ReplaceBar")
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .bg(theme::ed(theme::PANEL_BG))
                        .border_b_1()
                        .border_color(theme::ed(theme::BORDER))
                        .child(gpui::div().w(px(360.)).child(rep.clone()))
                        .child(gpui::div().text_color(theme::ed(theme::FG_DIM)).child("Enter: thay + tiếp · Alt+A: thay tất cả · Esc: đóng")),
                );
            }
        }
        if let Some(input) = &self.vim_cmdline {
            root = root.child(
                gpui::div()
                    .key_context("VimCmd")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .bg(theme::ed(theme::PANEL_BG))
                    .border_b_1()
                    .border_color(theme::ed(theme::BORDER))
                    .child(":")
                    .child(gpui::div().w(px(420.)).child(input.clone()))
                    .child(gpui::div().text_color(theme::ed(theme::FG_DIM)).child("w q wq e <file> 42 s/a/b/g %s/a/b/g noh · Enter · Esc")),
            );
        }
        let gutter_menu = self.render_gutter_menu(cx);
        root.child(EditorElement { view: cx.entity() }).children(self.render_popups(cx)).children(gutter_menu)
    }
}

struct EditorElement {
    view: Entity<EditorView>,
}

impl IntoElement for EditorElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

struct Prepaint {
    gutter: Vec<(ShapedLine, Point<Pixels>)>,
    /// current-line / breakpoint / ▶ backgrounds: full width, not clipped at the gutter
    gutter_quads: Vec<gpui::PaintQuad>,
    /// debugger inline values (text area)
    hints: Vec<(ShapedLine, Point<Pixels>)>,
    quads: Vec<gpui::PaintQuad>,
    cursor: Option<gpui::PaintQuad>,
}

impl Element for EditorElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
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
    ) -> Prepaint {
        let text_style = window.text_style();
        let font = text_style.font();
        let font_size = text_style.font_size.to_pixels(window.rem_size());
        let lh = window.line_height();
        let fg = theme::ed(theme::FG);

        self.view.update(cx, |view, _| {
            let b = &view.editor.buffer;
            let version = b.version();
            if let Some(h) = view.highlighter.as_mut() {
                h.update(version, || b.text());
            }
            if view.git_marks_version != version {
                view.git_marks_version = version;
                view.git_marks = match &view.git_base {
                    Some(base) => vcs::line_marks(&vcs::diff_lines(base, &b.text())),
                    None => Vec::new(),
                };
            }
            let b = &view.editor.buffer;
            let line_count = b.line_count();
            // scrolling is in display rows: collapsed regions take no rows
            let row_count = b.visible_line_count();
            if std::mem::take(&mut view.pending_center) {
                let row = b.row_of_line(b.line_of(view.editor.head));
                let half = (bounds.size.height / lh / 2.).floor() as usize;
                view.scroll_y = lh * row.saturating_sub(half) as f32;
            }
            let first_row = ((view.scroll_y / lh).floor() as usize).min(row_count.saturating_sub(1));
            let y_offset = view.scroll_y - lh * first_row as f32;
            let visible = ((bounds.size.height + y_offset) / lh).ceil() as usize + 1;
            let mut shown: Vec<usize> = Vec::with_capacity(visible);
            let mut next = b.line_of_row(first_row);
            while shown.len() < visible && next < line_count {
                shown.push(next);
                next = b.next_visible_line(next);
            }
            let first_line = shown.first().copied().unwrap_or(0);
            let last_line = shown.last().map_or(first_line, |l| l + 1);

            // gutter width from the widest line number
            let digits = line_count.to_string().len().max(3);
            let digit_run = |len, color| TextRun { len, font: font.clone(), color, background_color: None, underline: None, strikethrough: None };
            let probe = window.text_system().shape_line("0".repeat(digits).into(), font_size, &[digit_run(digits, fg)], None);
            // IntelliJ-like gutter: [numbers][breakpoint column][▶ column] | text
            let col_w = (lh * 0.95).floor();
            // Annotate: a "2026-09-30 1a2b3c4d author" column before the numbers
            let annot_w = if view.annotations.is_empty() { px(0.) } else { probe.width / digits as f32 * 32. };
            let numbers_end = px(10.) + annot_w + probe.width;
            let bp_col = (numbers_end + px(6.), numbers_end + px(6.) + col_w);
            let run_col = (bp_col.1, bp_col.1 + col_w);
            let gutter_w = run_col.1;
            let gutter_end = gutter_w + px(6.);
            // caret moved by keyboard: scroll sideways so it stays in view
            if std::mem::take(&mut view.follow_x) {
                let line = b.line_of(view.editor.head);
                let text = b.line_text(line);
                let (map, display) = TabMap::new(&text, view.tab_width);
                let run = TextRun { len: display.len(), font: font.clone(), color: fg, background_color: None, underline: None, strikethrough: None };
                let shaped = window.text_system().shape_line(SharedString::from(display), font_size, &[run], None);
                let x = shaped.x_for_index(map.to_display(view.editor.head - b.line_start(line)));
                let area = bounds.size.width - gutter_end;
                let margin = lh * 3.;
                if x < view.scroll_x + margin {
                    view.scroll_x = (x - margin).max(px(0.));
                } else if x > view.scroll_x + area - margin {
                    view.scroll_x = x - area + margin;
                }
            }
            let text_x = gutter_end - view.scroll_x;
            let char_w = probe.width / digits as f32;
            let unit = if view.editor.indent == "\t" { view.tab_width } else { view.editor.indent.len().max(1) };
            let guides: Vec<usize> = shown.iter().map(|&l| indent_levels(b, l, l + 1, view.tab_width)[0]).collect();

            let (cur_line, _) = view.editor.cursor_line_col();
            let is_http = b.path.as_deref().is_some_and(crate::http_client::is_http_file);
            let sel = view.editor.selection();
            let marked = view.editor.marked.clone();
            let range_start = b.line_start(first_line);
            let range_end = if last_line >= line_count { b.len() } else { b.line_start(last_line) };
            let spans = view.highlighter.as_ref().map(|h| h.spans(range_start..range_end)).unwrap_or_default();
            let diags: Vec<(Range<usize>, u8)> = view
                .diagnostics
                .iter()
                .filter(|d| d.range.end >= range_start && d.range.start <= range_end && d.severity <= 4)
                .map(|d| (d.range.clone(), d.severity))
                .collect();

            let mut lines = Vec::new();
            let mut gutter = Vec::new();
            let mut gutter_quads = Vec::new();
            let mut hints = Vec::new();
            let mut quads = Vec::new();
            let mut cursor = None;
            for (i, &line) in shown.iter().enumerate() {
                let start = b.line_start(line);
                let text = b.line_text(line);
                let end = start + text.len();
                let top = bounds.top() - y_offset + lh * i as f32;

                if view.exec_line == Some(line) {
                    gutter_quads.push(fill(Bounds::new(point(bounds.left(), top), size(bounds.size.width, lh)), theme::ed(theme::EXEC_LINE)));
                } else if line == cur_line && !view.editor.has_selection() {
                    gutter_quads.push(fill(Bounds::new(point(bounds.left(), top), size(bounds.size.width, lh)), theme::ed(theme::CURRENT_LINE)));
                }
                // coverage stripe at the right edge of the breakpoint column
                if let Some((_, k)) = view.coverage.iter().find(|(l, _)| *l == line) {
                    let color = match k {
                        2 => theme::GIT_ADDED,
                        1 => theme::WARNING,
                        _ => theme::ERROR,
                    };
                    gutter_quads.push(fill(Bounds::new(point(bounds.left() + bp_col.1 - px(3.), top), size(px(3.), lh)), theme::ed(color)));
                }
                // bookmark: a small tab at the gutter's left edge
                if view.bookmarks.contains(&line) {
                    gutter_quads.push(fill(Bounds::new(point(bounds.left() + px(1.), top + px(2.)), size(px(4.), lh - px(4.))), theme::ed(theme::ACCENT)));
                }
                // git change bar between the numbers and the breakpoint column
                if let Some((_, mark)) = view.git_marks.iter().find(|(l, _)| *l == line) {
                    let x0 = bounds.left() + numbers_end + px(2.);
                    let q = match mark {
                        vcs::LineMark::Added => fill(Bounds::new(point(x0, top), size(px(3.), lh)), theme::ed(theme::GIT_ADDED)),
                        vcs::LineMark::Modified => fill(Bounds::new(point(x0, top), size(px(3.), lh)), theme::ed(theme::GIT_MODIFIED)),
                        // removed lines: a small mark on the line's top edge
                        vcs::LineMark::Deleted => fill(Bounds::new(point(x0, top - px(2.)), size(px(6.), px(4.))), theme::ed(theme::FG_DIM)),
                    };
                    gutter_quads.push(q);
                }
                if let Some(a) = view.annotations.get(line) {
                    let run = TextRun { len: a.len(), font: font.clone(), color: theme::ed(theme::GUTTER_FG), background_color: None, underline: None, strikethrough: None };
                    let s = window.text_system().shape_line(SharedString::from(a.clone()), font_size * 0.9, &[run], None);
                    gutter.push((s, point(bounds.left() + px(6.), top)));
                }
                // breakpoint dot in its own column (never over the line number)
                if view.breakpoints.contains(&line) {
                    let d = (lh * 0.55).floor();
                    let x0 = bounds.left() + bp_col.0 + (col_w - d) / 2.;
                    let color = if view.conditional_breakpoints.contains(&line) { theme::BREAKPOINT_COND } else { theme::BREAKPOINT };
                    gutter_quads.push(fill(Bounds::new(point(x0, top + (lh - d) / 2.), size(d, d)), theme::ed(color)).corner_radii(d / 2.));
                }
                let trimmed = text.trim_start();
                if text.contains("static void main(") || trimmed.starts_with("@Test") || trimmed.starts_with("@ParameterizedTest") || (is_http && crate::http_client::is_request_line(&text)) {
                    // ▶ marker in the run column; clicking it opens Run/Debug
                    let d = (lh * 0.5).floor();
                    let x0 = bounds.left() + run_col.0 + (col_w - d * 0.8) / 2.;
                    gutter_quads.push(fill(Bounds::new(point(x0, top + (lh - d) / 2.), size(d * 0.8, d)), theme::ed(theme::RUN_GREEN)).corner_radii(px(2.)));
                }
                // indent guides: a thin line at each enclosing indent level (IntelliJ style)
                let level = guides[i];
                let mut col = 0;
                while col + unit <= level {
                    let x = bounds.left() + text_x + char_w * col as f32 + char_w * 0.5;
                    quads.push(fill(Bounds::new(point(x, top), size(px(1.), lh)), theme::ed(theme::INDENT_GUIDE)));
                    col += unit;
                }

                let (map, display) = TabMap::new(&text, view.tab_width);
                let sem: Vec<(Range<usize>, crate::lsp_editor::Sem)> =
                    view.semantic.iter().filter(|(r, _)| r.end > start && r.start < end).cloned().collect();
                let runs = line_runs(&text, start, &spans, &sem, marked.as_ref(), &diags, &map, &font, fg);
                let shaped = window.text_system().shape_line(SharedString::from(display), font_size, &runs, None);

                // find matches on this line
                if let Some(q) = view.last_find.as_deref().filter(|q| !q.is_empty() && view.find_input.is_some()) {
                    let lower = text.to_lowercase();
                    if lower.len() == text.len() {
                        let ql = q.to_lowercase();
                        for (i, _) in lower.match_indices(&ql) {
                            let x0 = shaped.x_for_index(map.to_display(i));
                            let x1 = shaped.x_for_index(map.to_display(i + ql.len()));
                            quads.push(fill(
                                Bounds::from_corners(point(bounds.left() + text_x + x0, top), point(bounds.left() + text_x + x1, top + lh)),
                                theme::ed(theme::FIND_MATCH),
                            ));
                        }
                    }
                }
                // selection
                if sel.start <= end && sel.end >= start && !sel.is_empty() {
                    let s = sel.start.max(start) - start;
                    let e = sel.end.min(end) - start;
                    let x0 = shaped.x_for_index(map.to_display(s));
                    let mut x1 = shaped.x_for_index(map.to_display(e));
                    if sel.end > end {
                        x1 += px(6.); // selection runs past the line break
                    }
                    if x1 > x0 {
                        quads.push(fill(
                            Bounds::from_corners(point(bounds.left() + text_x + x0, top), point(bounds.left() + text_x + x1, top + lh)),
                            theme::ed(theme::SELECTION),
                        ));
                    }
                }
                // the other carets (Alt+J): their selections and carets
                for r in &view.editor.extra {
                    let (rs, re) = (r.start.min(r.end), r.start.max(r.end));
                    if rs <= end && re >= start {
                        if re > rs {
                            let x0 = shaped.x_for_index(map.to_display(rs.max(start) - start));
                            let x1 = shaped.x_for_index(map.to_display(re.min(end) - start));
                            quads.push(fill(Bounds::from_corners(point(bounds.left() + text_x + x0, top), point(bounds.left() + text_x + x1, top + lh)), theme::ed(theme::SELECTION)));
                        }
                        if r.end >= start && r.end <= end {
                            let x = shaped.x_for_index(map.to_display(r.end - start));
                            quads.push(fill(Bounds::new(point(bounds.left() + text_x + x, top + px(1.)), size(px(2.), lh - px(2.))), theme::ed(theme::CURSOR)));
                        }
                    }
                }
                // cursor
                let head = view.editor.head;
                if head >= start && head <= end && line == cur_line {
                    let x = shaped.x_for_index(map.to_display(head - start));
                    let block = view.vim_block_cursor();
                    cursor = Some(if block {
                        // Vim Normal/Visual: a block over the character
                        let next = b.next_grapheme(head).min(end.max(head));
                        let w = if next > head { shaped.x_for_index(map.to_display(next - start)) - x } else { char_w };
                        let mut c = theme::ed(theme::CURSOR);
                        c.a = 0.45;
                        fill(Bounds::new(point(bounds.left() + text_x + x, top + px(1.)), size(w.max(px(2.)), lh - px(2.))), c)
                    } else {
                        fill(Bounds::new(point(bounds.left() + text_x + x, top + px(1.)), size(px(2.), lh - px(2.))), theme::ed(theme::CURSOR))
                    });
                }
                // line number, right-aligned
                let num = (line + 1).to_string();
                let line_sev = diags.iter().filter(|(r, _)| r.start <= end && r.end >= start && !(r.start == end && r.end == end && end > start)).map(|(_, s)| *s).min();
                let color = match line_sev {
                    Some(1) => theme::ed(theme::ERROR),
                    Some(_) => theme::ed(theme::WARNING),
                    None if line == cur_line => theme::ed(theme::GUTTER_FG_ACTIVE),
                    None => theme::ed(theme::GUTTER_FG),
                };
                let n = window.text_system().shape_line(num.clone().into(), font_size, &[digit_run(num.len(), color)], None);
                let nx = bounds.left() + numbers_end - n.width;
                gutter.push((n, point(nx, top)));
                // debugger inline values: grey italic after the end of the line
                // collapsed region: "⋯" after the header, like IntelliJ's {...}
                let folded = b.folds.iter().any(|(s, _)| *s == line);
                let mut after = shaped.width + char_w;
                if folded {
                    let dots = " ⋯ ".to_string();
                    let run = TextRun { len: dots.len(), font: font.clone(), color: theme::ed(theme::FG_DIM), background_color: Some(theme::ed(theme::FIND_MATCH)), underline: None, strikethrough: None };
                    let s = window.text_system().shape_line(SharedString::from(dots), font_size, &[run], None);
                    let w = s.width;
                    hints.push((s, point(bounds.left() + text_x + after, top)));
                    after += w;
                }
                let inline = view.inline_values.iter().find(|(l, _)| *l == line).map(|(_, h)| (h, true));
                let lens = view.lens.iter().find(|(l, _)| *l == line).map(|(_, h)| (h, false));
                if let Some((hint, italic)) = inline.or(lens) {
                    let mut f = font.clone();
                    if italic {
                        f.style = gpui::FontStyle::Italic;
                    }
                    let color = if italic { theme::ed(theme::INLINE_VALUE) } else { theme::ed(theme::GUTTER_FG) };
                    let run = TextRun { len: hint.len(), font: f, color, background_color: None, underline: None, strikethrough: None };
                    let shaped_hint = window.text_system().shape_line(SharedString::from(hint.clone()), font_size, &[run], None);
                    let hx = bounds.left() + text_x + after + char_w * 2.;
                    hints.push((shaped_hint, point(hx, top)));
                }

                lines.push(VisibleLine { line, start, map, shaped });
            }
            view.layout = Some(Layout { bounds, text_x, gutter_end, run_col, line_height: lh, first_line, y_offset, lines });
            Prepaint { gutter, gutter_quads, hints, quads, cursor }
        })
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        pre: &mut Prepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.view.read(cx).focus.clone();
        let (vim, block) = self.view.read_with(cx, |v, _| (v.vim.is_some(), v.vim_block_cursor()));
        // Vim Normal/Visual: no text input (fcitx must not compose "dd" into "đ")
        if !block {
            window.handle_input(&focus, ElementInputHandler::new(bounds, self.view.clone()), cx);
        }
        if vim && focus.is_focused(window) {
            crate::vim_mode::set_focus(self.view.downgrade(), cx);
        }
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            for q in pre.gutter_quads.drain(..) {
                window.paint_quad(q);
            }
            let lh = window.line_height();
            let view = self.view.read(cx);
            let Some(layout) = view.layout.as_ref() else { return };
            let origin_x = bounds.left() + layout.text_x;
            let tops: Vec<Pixels> =
                (0..layout.lines.len()).map(|i| bounds.top() - layout.y_offset + lh * i as f32).collect();
            let shaped: Vec<ShapedLine> = layout.lines.iter().map(|l| l.shaped.clone()).collect();
            // the text area is clipped at the gutter, so scrolled text slides under it
            let text_area = Bounds::from_corners(point(bounds.left() + layout.gutter_end, bounds.top()), bounds.bottom_right());
            let show_cursor = focus.is_focused(window);
            window.with_content_mask(Some(gpui::ContentMask { bounds: text_area }), |window| {
                for q in pre.quads.drain(..) {
                    window.paint_quad(q);
                }
                for (line, top) in shaped.iter().zip(tops) {
                    let _ = line.paint(point(origin_x, top), lh, window, cx);
                }
                for (n, at) in pre.hints.drain(..) {
                    let _ = n.paint(at, lh, window, cx);
                }
                if show_cursor {
                    if let Some(cur) = pre.cursor.take() {
                        window.paint_quad(cur);
                    }
                }
            });
            for (n, at) in pre.gutter.drain(..) {
                let _ = n.paint(at, lh, window, cx);
            }
        });
    }
}

/// Indent width (columns) per visible line; blank lines take the smaller of the
/// surrounding non-blank lines, so guides run through empty lines inside a block.
fn indent_levels(b: &editor_core::Buffer, first: usize, last: usize, tab: usize) -> Vec<usize> {
    let indent = |t: &str| -> Option<usize> {
        if t.trim().is_empty() {
            return None;
        }
        Some(t.chars().take_while(|c| *c == ' ' || *c == '\t').map(|c| if c == '\t' { tab } else { 1 }).sum())
    };
    let count = b.line_count();
    let own: Vec<Option<usize>> = (first..last).map(|l| indent(&b.line_text(l))).collect();
    let near = |mut l: isize, step: isize| -> usize {
        // nearest non-blank line in one direction (bounded look-around)
        for _ in 0..200 {
            l += step;
            if l < 0 || l as usize >= count {
                return 0;
            }
            if let Some(i) = indent(&b.line_text(l as usize)) {
                return i;
            }
        }
        0
    };
    own.iter()
        .enumerate()
        .map(|(i, v)| match v {
            Some(v) => *v,
            None => {
                let l = (first + i) as isize;
                near(l, -1).min(near(l, 1))
            }
        })
        .collect()
}

/// Text runs for one line: syntax colors, with the IME preedit underlined.
fn line_runs(
    text: &str,
    line_start: usize,
    spans: &[(Range<usize>, syntax::Style)],
    sem: &[(Range<usize>, crate::lsp_editor::Sem)],
    marked: Option<&Range<usize>>,
    diags: &[(Range<usize>, u8)],
    map: &TabMap,
    font: &gpui::Font,
    fg: gpui::Hsla,
) -> Vec<TextRun> {
    let line_end = line_start + text.len();
    let mut cuts = vec![0, text.len()];
    for (r, _) in spans {
        if r.end > line_start && r.start < line_end {
            cuts.push(r.start.max(line_start) - line_start);
            cuts.push(r.end.min(line_end) - line_start);
        }
    }
    for (r, _) in sem {
        if r.end > line_start && r.start < line_end {
            cuts.push(r.start.max(line_start) - line_start);
            cuts.push(r.end.min(line_end) - line_start);
        }
    }
    for (r, _) in diags {
        if r.end > line_start && r.start < line_end {
            cuts.push(r.start.max(line_start) - line_start);
            cuts.push(r.end.min(line_end) - line_start);
        }
    }
    if let Some(m) = marked {
        if m.end > line_start && m.start < line_end {
            cuts.push(m.start.max(line_start) - line_start);
            cuts.push(m.end.min(line_end) - line_start);
        }
    }
    cuts.retain(|&i| text.is_char_boundary(i));
    cuts.sort_unstable();
    cuts.dedup();

    let mut runs = Vec::new();
    for w in cuts.windows(2) {
        let (s, e) = (w[0], w[1]);
        if s == e {
            continue;
        }
        let abs = line_start + s;
        let mut color = spans.iter().find(|(r, _)| r.contains(&abs)).map(|(_, st)| theme::syntax(*st)).unwrap_or(fg);
        let mut run_font = font.clone();
        let mut strike = None;
        if let Some((_, s)) = sem.iter().find(|(r, _)| r.contains(&abs)) {
            if s.field {
                color = theme::syntax(syntax::Style::Property);
            }
            if s.annotation {
                color = theme::syntax(syntax::Style::Attribute);
            }
            if s.is_static {
                run_font.style = gpui::FontStyle::Italic;
            }
            if s.deprecated {
                strike = Some(gpui::StrikethroughStyle { color: Some(color), thickness: px(1.) });
            }
        }
        let underline = marked
            .filter(|m| m.contains(&abs))
            .map(|_| UnderlineStyle { color: Some(color), thickness: px(1.), wavy: false })
            .or_else(|| {
                // most severe diagnostic covering this run: wavy underline
                diags.iter().filter(|(r, _)| r.contains(&abs)).map(|(_, s)| *s).min().map(|sev| UnderlineStyle {
                    color: Some(theme::ed(match sev {
                        1 => theme::ERROR,
                        2 => theme::WARNING,
                        // hints (JSF "@all", c:forEach…): a quiet grey like IntelliJ's weak warnings
                        _ => theme::FG_DIM,
                    })),
                    thickness: px(1.),
                    wavy: true,
                })
            });
        // run lengths are in display bytes (tabs expanded)
        let len = map.to_display(e) - map.to_display(s);
        runs.push(TextRun { len, font: run_font, color, background_color: None, underline, strikethrough: strike });
    }
    if runs.is_empty() {
        runs.push(TextRun { len: 0, font: font.clone(), color: fg, background_color: None, underline: None, strikethrough: None });
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::TabMap;

    #[test]
    fn tab_map_roundtrip() {
        let (m, d) = TabMap::new("\tint x;\t// c", 4);
        assert_eq!(d, "    int x;    // c");
        assert_eq!(m.to_display(0), 0);
        assert_eq!(m.to_display(1), 4);
        assert_eq!(m.to_byte(4), 1);
        assert_eq!(m.to_byte(1), 0);
        assert_eq!(m.to_byte(3), 1);
        assert_eq!(m.to_display(8), 14); // second tab at byte 7
        assert_eq!(m.to_byte(14), 8);
    }
}
