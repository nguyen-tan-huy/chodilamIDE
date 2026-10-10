//! Editor state (buffer + selection + IME composition) and the command set.
//! UI-free: the app maps key presses to `Command` via `keymap` and renders the state.
use crate::buffer::Buffer;
use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Left,
    Right,
    Up,
    Down,
    WordLeft,
    WordRight,
    LineStart,
    LineEnd,
    DocStart,
    DocEnd,
    PageUp,
    PageDown,
    SelectLeft,
    SelectRight,
    SelectUp,
    SelectDown,
    SelectWordLeft,
    SelectWordRight,
    SelectLineStart,
    SelectLineEnd,
    SelectAll,
    Backspace,
    Delete,
    DeleteWordLeft,
    Newline,
    Tab,
    DuplicateLine,
    DeleteLine,
    Undo,
    Redo,
    Save,
    Copy,
    Cut,
    Paste,
    Find,
    FindNext,
    /// Ctrl+/ : toggle line comments on the selected lines
    CommentLine,
    /// Ctrl+Shift+/ : wrap / unwrap the selection in a block comment
    CommentBlock,
    MoveLineUp,
    MoveLineDown,
    /// Shift+Tab (Tab with a multi-line selection indents)
    Unindent,
    /// Ctrl+Shift+J
    JoinLines,
    /// Ctrl+Shift+U
    ToggleCase,
    /// Ctrl+W: word → quoted/bracketed text → line → … (IntelliJ Extend Selection)
    ExtendSelection,
    /// Ctrl+Shift+W
    ShrinkSelection,
    /// Ctrl+- / Ctrl+= : collapse / expand the region at the caret
    FoldCollapse,
    FoldExpand,
    /// Ctrl+Shift+- / Ctrl+Shift+= : every region
    FoldAll,
    UnfoldAll,
    /// Alt+J: select the word, then add the next occurrence as another caret/selection
    AddNextOccurrence,
    /// Alt+Shift+J: drop the last added occurrence
    RemoveLastOccurrence,
    /// Ctrl+Alt+Shift+J: every occurrence of the selection
    SelectAllOccurrences,
    /// Esc with several carets: back to one
    SingleCaret,
}

pub struct Editor {
    pub buffer: Buffer,
    /// Where the selection started; equal to `head` when nothing is selected.
    pub anchor: usize,
    pub head: usize,
    /// Display column remembered across Up/Down through short lines.
    preferred_col: Option<usize>,
    /// IME preedit currently shown inline (byte range in the buffer).
    pub marked: Option<Range<usize>>,
    pub page_lines: usize,
    pub indent: String,
    /// comment syntax of the file's language: `//` and `/* */` for Java; None where the
    /// language has no line comment (XML) - Ctrl+/ then uses the block comment per line
    pub line_comment: Option<String>,
    pub block_comment: Option<(String, String)>,
    /// selections Ctrl+W grew from, for Ctrl+Shift+W
    selection_stack: Vec<Range<usize>>,
    /// other carets / selections (Alt+J); typing, Backspace, Delete and paste edit them all.
    /// Each is anchor..head like the primary one (head = caret).
    pub extra: Vec<Range<usize>>,
}

/// What the UI must do after a command the core can't do itself.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    None,
    CopyToClipboard(String),
    RequestPaste,
    RequestSave,
    OpenFind,
    FindNext,
}

impl Editor {
    pub fn new(buffer: Buffer) -> Self {
        Self {
            buffer,
            anchor: 0,
            head: 0,
            preferred_col: None,
            marked: None,
            page_lines: 40,
            indent: "    ".into(),
            line_comment: Some("//".into()),
            block_comment: Some(("/*".into(), "*/".into())),
            selection_stack: Vec::new(),
            extra: Vec::new(),
        }
    }

    pub fn selection(&self) -> Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }

    pub fn has_selection(&self) -> bool {
        self.anchor != self.head
    }

    pub fn cursor_line_col(&self) -> (usize, usize) {
        (self.buffer.line_of(self.head), self.buffer.display_col(self.head))
    }

    pub fn set_cursor(&mut self, byte: usize, extend: bool) {
        let byte = byte.min(self.buffer.len());
        self.head = byte;
        if !extend {
            self.anchor = byte;
        }
        self.buffer.break_group();
        // jumping into a collapsed region (goto, search) opens it
        self.reveal_cursor();
    }

    fn move_to(&mut self, byte: usize, extend: bool, keep_col: bool) {
        self.head = byte.min(self.buffer.len());
        if !extend {
            self.anchor = self.head;
        }
        if !keep_col {
            self.preferred_col = None;
        }
        self.buffer.break_group();
    }

    /// Up/Down by display rows, so collapsed regions are stepped over.
    fn vertical(&mut self, delta: isize, extend: bool) {
        let (line, col) = self.cursor_line_col();
        let col = *self.preferred_col.get_or_insert(col);
        let b = &self.buffer;
        let last = b.visible_line_count().saturating_sub(1) as isize;
        let target = b.row_of_line(line) as isize + delta;
        let byte = if target < 0 {
            0
        } else if target > last {
            b.len()
        } else {
            b.byte_at_display_col(b.line_of_row(target as usize), col)
        };
        self.move_to(byte, extend, true);
    }

    /// The caret never stays inside a collapsed region: open the folds hiding it.
    fn reveal_cursor(&mut self) {
        let line = self.buffer.line_of(self.head);
        if self.buffer.is_hidden(line) {
            self.buffer.folds.retain(|(s, e)| !(line > *s && line <= *e));
        }
    }

    /// Innermost foldable region (header, last hidden line) containing `line`.
    pub fn fold_region_at(&self, line: usize) -> Option<(usize, usize)> {
        let regions = fold_regions(&self.buffer);
        regions.into_iter().filter(|(s, e)| *s <= line && line <= *e + 1).min_by_key(|(s, e)| e - s)
    }

    fn fold_collapse(&mut self) {
        let line = self.buffer.line_of(self.head);
        // innermost region at the caret that is still open
        let mut regions: Vec<(usize, usize)> = fold_regions(&self.buffer).into_iter().filter(|(s, e)| *s <= line && line <= *e + 1).collect();
        regions.sort_by_key(|(s, e)| e - s);
        if let Some(r) = regions.into_iter().find(|r| !self.buffer.folds.contains(r)) {
            self.buffer.folds.push(r);
            // the caret goes to the header, like IntelliJ
            if line > r.0 {
                let at = self.buffer.line_start(r.0) + self.buffer.line_text(r.0).trim_end().len();
                self.move_to(at, false, false);
            }
        }
    }

    fn fold_expand(&mut self) {
        let line = self.buffer.line_of(self.head);
        if let Some(i) = self.buffer.folds.iter().rposition(|(s, _)| *s == line) {
            self.buffer.folds.remove(i);
        } else if let Some(i) = self.buffer.folds.iter().rposition(|(s, e)| *s <= line && line <= *e) {
            self.buffer.folds.remove(i);
        }
    }

    pub fn run(&mut self, cmd: Command) -> Effect {
        let effect = self.run_inner(cmd);
        if !matches!(cmd, Command::FoldCollapse | Command::FoldAll) {
            self.reveal_cursor();
        }
        effect
    }

    /// Every caret: the primary one last. Ranges are anchor..head (possibly reversed).
    fn all_carets(&self) -> Vec<Range<usize>> {
        let mut v = self.extra.clone();
        v.push(self.anchor..self.head);
        v
    }

    /// Applies one edit per caret (`edit(anchor, head)` → range to replace + new text), as one
    /// undo step; carets end after their inserted text.
    pub(crate) fn edit_all(&mut self, edit: impl Fn(&Buffer, usize, usize) -> (Range<usize>, String)) {
        let carets = self.all_carets();
        let n = carets.len();
        let mut edits: Vec<(usize, Range<usize>, String)> = carets.iter().enumerate().map(|(i, c)| {
            let (r, t) = edit(&self.buffer, c.start, c.end);
            (i, r, t)
        }).collect();
        // overlapping ranges (two carets in one word) would corrupt the text: keep the first
        edits.sort_by_key(|e| e.1.start);
        edits.dedup_by(|b, a| b.1.start < a.1.end);
        let before = self.anchor..self.head;
        // apply from the end so earlier offsets stay valid; one undo step for all
        self.buffer.begin_compound();
        for (_, r, t) in edits.iter().rev() {
            let end = r.start + t.len();
            self.buffer.edit(r.clone(), t, before.clone(), end..end);
        }
        self.buffer.end_compound();
        // new caret positions (ascending order): shift by the growth of the edits before
        let mut shift: isize = 0;
        let mut new_pos = vec![None; n];
        for (i, r, t) in &edits {
            let at = (r.start as isize + shift) as usize + t.len();
            new_pos[*i] = Some(at);
            shift += t.len() as isize - r.len() as isize;
        }
        let mut positions: Vec<usize> = new_pos.into_iter().flatten().collect();
        let primary = positions.pop().unwrap_or(self.head);
        self.extra = positions.into_iter().map(|p| p..p).collect();
        self.anchor = primary;
        self.head = primary;
        self.preferred_col = None;
    }

    /// Typed / pasted text at every caret (replacing each selection).
    pub fn insert_all(&mut self, text: &str) {
        if self.extra.is_empty() {
            return self.insert(text);
        }
        self.edit_all(|_, a, h| (a.min(h)..a.max(h), text.to_string()));
    }

    fn select_word_at_caret(&mut self) {
        let b = &self.buffer;
        let s = b.word_left(b.next_grapheme(self.head).min(b.len()));
        let e = b.word_right(s);
        let word = b.slice(s..e);
        if word.trim().is_empty() || !word.chars().any(|c| c.is_alphanumeric() || c == '_') {
            return;
        }
        // word_right may include trailing spaces: trim them
        let e = s + word.trim_end().len();
        self.anchor = s;
        self.head = e;
    }

    fn add_next_occurrence(&mut self) {
        if !self.has_selection() {
            return self.select_word_at_caret();
        }
        let needle = self.buffer.slice(self.selection());
        let taken: Vec<Range<usize>> = self.all_carets().into_iter().map(|c| c.start.min(c.end)..c.start.max(c.end)).collect();
        let mut from = self.selection().end;
        for _ in 0..taken.len() + 1 {
            let Some(r) = self.buffer.find(&needle, from, true) else { return };
            if !taken.contains(&r) {
                self.extra.push(self.anchor..self.head);
                self.anchor = r.start;
                self.head = r.end;
                return;
            }
            from = r.end;
        }
    }

    fn select_all_occurrences(&mut self) {
        if !self.has_selection() {
            self.select_word_at_caret();
        }
        if !self.has_selection() {
            return;
        }
        let needle = self.buffer.slice(self.selection());
        let text = self.buffer.text();
        let primary = self.selection();
        self.extra = text.match_indices(&needle).map(|(i, _)| i..i + needle.len()).filter(|r| *r != primary).collect();
    }

    /// Replaces the selection (or `range`) with `text`, leaving the cursor after it.
    pub fn insert(&mut self, text: &str) {
        let range = self.selection();
        let before = self.anchor..self.head;
        let end = range.start + text.len();
        self.buffer.edit(range, text, before, end..end);
        self.anchor = end;
        self.head = end;
        self.preferred_col = None;
    }

    fn delete_range(&mut self, range: Range<usize>) {
        if range.is_empty() {
            return;
        }
        let before = self.anchor..self.head;
        let at = range.start;
        self.buffer.edit(range, "", before, at..at);
        self.anchor = at;
        self.head = at;
        self.preferred_col = None;
    }

    fn current_indent(&self) -> String {
        let line = self.buffer.line_text(self.buffer.line_of(self.head));
        line.chars().take_while(|c| *c == ' ' || *c == '\t').collect()
    }

    /// First and last line touched by the selection (a selection ending at column 0 of a line
    /// doesn't include that line).
    fn selected_lines(&self) -> (usize, usize) {
        let b = &self.buffer;
        let r = self.selection();
        let first = b.line_of(r.start);
        let mut last = b.line_of(r.end);
        if last > first && r.end == b.line_start(last) {
            last -= 1;
        }
        (first, last)
    }

    /// Replaces whole lines first..=last with `new` (no trailing newline), one undo step,
    /// and selects the result (or keeps a caret at `caret` when given).
    fn replace_lines(&mut self, first: usize, last: usize, new: &str, keep_selection: bool) {
        let b = &self.buffer;
        let (start, end) = (b.line_start(first), b.line_end(last));
        let before = self.anchor..self.head;
        let had_selection = self.has_selection();
        // keep the caret's column offset from the line end when there was no selection
        let from_end = end - self.head.min(end);
        self.buffer.break_group();
        let new_end = start + new.len();
        self.buffer.edit(start..end, new, before, start..new_end);
        self.buffer.break_group();
        if keep_selection && had_selection {
            self.anchor = start;
            self.head = new_end;
        } else {
            let at = new_end.saturating_sub(from_end).max(start);
            self.anchor = at;
            self.head = at;
        }
        self.preferred_col = None;
    }

    fn toggle_line_comment(&mut self) {
        let (first, last) = self.selected_lines();
        let lines: Vec<String> = (first..=last).map(|l| self.buffer.line_text(l)).collect();
        let (open, close) = match (&self.line_comment, &self.block_comment) {
            (Some(lc), _) => (lc.clone(), String::new()),
            (None, Some((o, c))) => (o.clone(), c.clone()),
            (None, None) => return,
        };
        let is_commented = |l: &str| {
            let t = l.trim_start();
            t.starts_with(&open) && (close.is_empty() || t.trim_end().ends_with(&close))
        };
        let non_blank: Vec<&String> = lines.iter().filter(|l| !l.trim().is_empty()).collect();
        let uncomment = !non_blank.is_empty() && non_blank.iter().all(|l| is_commented(l));
        let indent = non_blank.iter().map(|l| l.len() - l.trim_start().len()).min().unwrap_or(0);
        let out: Vec<String> = lines
            .iter()
            .map(|l| {
                if l.trim().is_empty() {
                    return l.clone();
                }
                if uncomment {
                    let lead = l.len() - l.trim_start().len();
                    let mut rest = &l[lead + open.len()..];
                    if !close.is_empty() {
                        rest = rest.trim_end().strip_suffix(close.as_str()).unwrap_or(rest).trim_end();
                        rest = rest.strip_prefix(' ').unwrap_or(rest);
                    } else {
                        rest = rest.strip_prefix(' ').unwrap_or(rest);
                    }
                    format!("{}{rest}", &l[..lead])
                } else if close.is_empty() {
                    format!("{}{open} {}", &l[..indent], &l[indent..])
                } else {
                    format!("{}{open} {} {close}", &l[..indent], &l[indent..])
                }
            })
            .collect();
        let single = first == last && !self.has_selection();
        self.replace_lines(first, last, &out.join("\n"), true);
        // IntelliJ moves the caret to the next line after commenting a single line
        if single {
            let next = self.buffer.line_start((last + 1).min(self.buffer.line_count().saturating_sub(1)));
            if last + 1 < self.buffer.line_count() {
                self.anchor = next;
                self.head = next;
            }
        }
    }

    fn toggle_block_comment(&mut self) {
        let Some((open, close)) = self.block_comment.clone() else { return self.toggle_line_comment() };
        let r = self.selection();
        let text = self.buffer.slice(r.clone());
        let t = text.trim();
        if r.is_empty() {
            // nothing selected: an empty comment at the caret, caret inside
            let at = self.head;
            self.buffer.break_group();
            self.insert(&format!("{open}  {close}"));
            self.buffer.break_group();
            self.set_cursor(at + open.len() + 1, false);
            return;
        }
        self.buffer.break_group();
        if t.starts_with(&open) && t.ends_with(&close) && t.len() >= open.len() + close.len() {
            let lead = text.len() - text.trim_start().len();
            let inner = &t[open.len()..t.len() - close.len()];
            let inner = inner.strip_prefix(' ').unwrap_or(inner);
            let inner = inner.strip_suffix(' ').unwrap_or(inner);
            let new = format!("{}{inner}{}", &text[..lead], &text[lead + t.len()..]);
            let start = r.start;
            self.insert(&new);
            self.anchor = start;
        } else {
            let start = r.start;
            self.insert(&format!("{open}{text}{close}"));
            self.anchor = start;
        }
        self.buffer.break_group();
    }

    fn move_lines(&mut self, down: bool) {
        let (first, last) = self.selected_lines();
        let count = self.buffer.line_count();
        if (!down && first == 0) || (down && last + 1 >= count) {
            return;
        }
        let (a, b_) = if down { (first, last + 1) } else { (first - 1, last) };
        let lines: Vec<String> = (a..=b_).map(|l| self.buffer.line_text(l)).collect();
        let mut moved = lines.clone();
        if down {
            let x = moved.pop().unwrap();
            moved.insert(0, x);
        } else {
            let x = moved.remove(0);
            moved.push(x);
        }
        // selection / caret move with the lines
        let (sa, sh) = (self.anchor, self.head);
        let shift: isize = if down { lines.last().unwrap().len() as isize + 1 } else { -(lines[0].len() as isize + 1) };
        let before = sa..sh;
        let (start, end) = (self.buffer.line_start(a), self.buffer.line_end(b_));
        let new = moved.join("\n");
        let na = (sa as isize + shift) as usize;
        let nh = (sh as isize + shift) as usize;
        self.buffer.break_group();
        self.buffer.edit(start..end, &new, before, na.min(nh)..na.max(nh));
        self.buffer.break_group();
        self.anchor = na;
        self.head = nh;
    }

    fn indent_lines(&mut self, out: bool) {
        let (first, last) = self.selected_lines();
        let unit = self.indent.clone();
        let lines: Vec<String> = (first..=last)
            .map(|l| {
                let s = self.buffer.line_text(l);
                if out {
                    if let Some(r) = s.strip_prefix(unit.as_str()) {
                        r.to_string()
                    } else if s.starts_with('\t') {
                        s[1..].to_string()
                    } else {
                        // indented by less than one unit: drop what spaces there are
                        let lead = s.len() - s.trim_start_matches(' ').len();
                        s[lead.min(unit.len())..].to_string()
                    }
                } else if s.is_empty() {
                    s
                } else {
                    format!("{unit}{s}")
                }
            })
            .collect();
        self.replace_lines(first, last, &lines.join("\n"), true);
    }

    fn join_lines(&mut self) {
        let (first, mut last) = self.selected_lines();
        if first == last {
            last = first + 1;
        }
        if last >= self.buffer.line_count() {
            return;
        }
        let mut out = self.buffer.line_text(first).trim_end().to_string();
        for l in first + 1..=last {
            let next = self.buffer.line_text(l);
            let next = next.trim_start();
            // a comment continuation line loses its "//" / "*"
            let next = next.strip_prefix("// ").or_else(|| next.strip_prefix("* ")).filter(|_| out.trim_start().starts_with("//") || out.trim_start().starts_with('*')).unwrap_or(next);
            if !out.is_empty() && !next.is_empty() && !out.ends_with('(') && !next.starts_with([')', '.', ';', ',']) {
                out.push(' ');
            }
            out.push_str(next);
        }
        let caret = self.buffer.line_text(first).trim_end().len();
        let start = self.buffer.line_start(first);
        self.replace_lines(first, last, &out, false);
        self.set_cursor(start + caret, false);
    }

    fn toggle_case(&mut self) {
        if !self.has_selection() {
            let (s, e) = (self.buffer.word_left(self.head.min(self.buffer.word_right(self.head))), self.buffer.word_right(self.head));
            self.anchor = s;
            self.head = e;
        }
        let r = self.selection();
        let text = self.buffer.slice(r.clone());
        let new = if text.chars().any(|c| c.is_lowercase()) { text.to_uppercase() } else { text.to_lowercase() };
        self.buffer.break_group();
        self.insert(&new);
        self.anchor = r.start;
        self.buffer.break_group();
    }

    /// Next larger selection around the current one: word, then the inside of the enclosing
    /// quotes/brackets, then including them, then the line, then everything.
    fn extend_selection(&mut self) {
        let text = self.buffer.text();
        let cur = self.selection();
        let cands = extend_candidates(&text, cur.clone());
        if let Some(next) = cands.into_iter().find(|c| c.start <= cur.start && c.end >= cur.end && c.len() > cur.len()) {
            self.selection_stack.push(cur);
            self.anchor = next.start;
            self.head = next.end;
        }
    }

    fn shrink_selection(&mut self) {
        if let Some(prev) = self.selection_stack.pop() {
            self.anchor = prev.start;
            self.head = prev.end;
        }
    }

    fn run_inner(&mut self, cmd: Command) -> Effect {
        use Command::*;
        if !matches!(cmd, ExtendSelection | ShrinkSelection) {
            self.selection_stack.clear();
        }
        // several carets: editing commands act on all of them, anything else drops the extras
        if !self.extra.is_empty() {
            match cmd {
                Backspace => {
                    self.edit_all(|b, a, h| if a != h { (a.min(h)..a.max(h), String::new()) } else { (b.prev_grapheme(h)..h, String::new()) });
                    return Effect::None;
                }
                Delete => {
                    self.edit_all(|b, a, h| if a != h { (a.min(h)..a.max(h), String::new()) } else { (h..b.next_grapheme(h), String::new()) });
                    return Effect::None;
                }
                Newline => {
                    self.insert_all("\n");
                    return Effect::None;
                }
                AddNextOccurrence | RemoveLastOccurrence | SelectAllOccurrences | Copy | Paste | Save | Undo | Redo | Find | FindNext => {}
                _ => self.extra.clear(),
            }
        }
        let b = &self.buffer;
        match cmd {
            Left => {
                let t = if self.has_selection() { self.selection().start } else { b.prev_grapheme(self.head) };
                self.move_to(t, false, false)
            }
            Right => {
                let t = if self.has_selection() { self.selection().end } else { b.next_grapheme(self.head) };
                self.move_to(t, false, false)
            }
            SelectLeft => self.move_to(b.prev_grapheme(self.head), true, false),
            SelectRight => self.move_to(b.next_grapheme(self.head), true, false),
            WordLeft => self.move_to(b.word_left(self.head), false, false),
            WordRight => self.move_to(b.word_right(self.head), false, false),
            SelectWordLeft => self.move_to(b.word_left(self.head), true, false),
            SelectWordRight => self.move_to(b.word_right(self.head), true, false),
            Up => self.vertical(-1, false),
            Down => self.vertical(1, false),
            SelectUp => self.vertical(-1, true),
            SelectDown => self.vertical(1, true),
            PageUp => self.vertical(-(self.page_lines as isize), false),
            PageDown => self.vertical(self.page_lines as isize, false),
            LineStart | SelectLineStart => {
                // IntelliJ Home: first non-blank, then column 0
                let line = b.line_of(self.head);
                let start = b.line_start(line);
                let first = start + self.current_indent().len();
                let t = if self.head == first { start } else { first };
                self.move_to(t, cmd == SelectLineStart, false)
            }
            LineEnd | SelectLineEnd => {
                let t = b.line_end(b.line_of(self.head));
                self.move_to(t, cmd == SelectLineEnd, false)
            }
            DocStart => self.move_to(0, false, false),
            DocEnd => self.move_to(b.len(), false, false),
            SelectAll => {
                self.anchor = 0;
                self.head = self.buffer.len();
            }
            Backspace => {
                let r = if self.has_selection() { self.selection() } else { b.prev_grapheme(self.head)..self.head };
                self.delete_range(r)
            }
            Delete => {
                let r = if self.has_selection() { self.selection() } else { self.head..b.next_grapheme(self.head) };
                self.delete_range(r)
            }
            DeleteWordLeft => {
                let r = if self.has_selection() { self.selection() } else { b.word_left(self.head)..self.head };
                self.delete_range(r)
            }
            Newline => self.smart_newline(),
            Tab => {
                let (first, last) = self.selected_lines();
                if self.has_selection() && last > first {
                    self.indent_lines(false)
                } else {
                    let indent = self.indent.clone();
                    self.insert(&indent)
                }
            }
            Unindent => self.indent_lines(true),
            CommentLine => self.toggle_line_comment(),
            CommentBlock => self.toggle_block_comment(),
            MoveLineUp => self.move_lines(false),
            MoveLineDown => self.move_lines(true),
            JoinLines => self.join_lines(),
            ToggleCase => self.toggle_case(),
            ExtendSelection => self.extend_selection(),
            ShrinkSelection => self.shrink_selection(),
            AddNextOccurrence => self.add_next_occurrence(),
            RemoveLastOccurrence => {
                if let Some(r) = self.extra.pop() {
                    self.anchor = r.start;
                    self.head = r.end;
                }
            }
            SelectAllOccurrences => self.select_all_occurrences(),
            SingleCaret => self.extra.clear(),
            FoldCollapse => self.fold_collapse(),
            FoldExpand => self.fold_expand(),
            FoldAll => {
                let mut regions = fold_regions(&self.buffer);
                // imports and top-level class bodies stay readable: fold what's inside classes
                regions.retain(|(s, _)| self.buffer.line_text(*s).starts_with(char::is_whitespace) || self.buffer.line_text(*s).starts_with("import "));
                self.buffer.folds = regions;
                let line = self.buffer.line_of(self.head);
                if self.buffer.is_hidden(line) {
                    let row = self.buffer.row_of_line(line);
                    let l = self.buffer.line_of_row(row);
                    let at = self.buffer.line_start(l);
                    self.move_to(at, false, false);
                }
            }
            UnfoldAll => self.buffer.folds.clear(),
            DuplicateLine => {
                let line = b.line_of(self.head);
                let text = b.line_text(line);
                let end = b.line_end(line);
                let col = self.head - b.line_start(line);
                self.buffer.break_group();
                self.anchor = end;
                self.head = end;
                self.insert(&format!("\n{text}"));
                let new_start = end + 1;
                self.move_to(new_start + col, false, false);
            }
            DeleteLine => {
                let line = b.line_of(self.head);
                let start = b.line_start(line);
                let next = b.line_start(line + 1);
                let r = if next > start { start..next } else { b.line_end(line.saturating_sub(1))..b.len() };
                self.buffer.break_group();
                self.delete_range(r)
            }
            Undo => {
                self.extra.clear();
                if let Some(sel) = self.buffer.undo() {
                    self.anchor = sel.start;
                    self.head = sel.end;
                    self.marked = None;
                }
            }
            Redo => {
                if let Some(sel) = self.buffer.redo() {
                    self.anchor = sel.start;
                    self.head = sel.end;
                }
            }
            Copy | Cut => {
                // IntelliJ: with no selection, copy/cut the whole line
                let line = b.line_of(self.head);
                let r = if self.has_selection() { self.selection() } else { b.line_start(line)..b.line_start(line + 1) };
                let mut text = b.slice(r.clone());
                if !self.has_selection() && !text.ends_with('\n') {
                    text.push('\n');
                }
                if cmd == Cut {
                    self.buffer.break_group();
                    self.delete_range(r);
                }
                return Effect::CopyToClipboard(text);
            }
            Paste => return Effect::RequestPaste,
            Save => return Effect::RequestSave,
            Find => return Effect::OpenFind,
            FindNext => return Effect::FindNext,
        }
        Effect::None
    }

    pub fn paste(&mut self, text: &str) {
        self.buffer.break_group();
        self.insert_all(text);
        self.buffer.break_group();
    }

    // ---- IME composition --------------------------------------------------------

    /// Shows/updates preedit text; replaces the previous preedit (or the selection).
    pub fn set_marked_text(&mut self, text: &str) {
        // Vietnamese IME preedit works on one caret: drop the others
        self.extra.clear();
        let range = self.marked.clone().unwrap_or_else(|| self.selection());
        let before = self.anchor..self.head;
        let end = range.start + text.len();
        self.buffer.edit(range.clone(), text, before, end..end);
        self.marked = if text.is_empty() { None } else { Some(range.start..end) };
        self.anchor = end;
        self.head = end;
    }

    /// Commits `text`, replacing any preedit.
    pub fn commit_text(&mut self, text: &str) {
        if let Some(m) = self.marked.take() {
            self.anchor = m.start;
            self.head = m.end;
            // a composition (IME) only ever edits the primary caret
            self.extra.clear();
        }
        if matches!(text, "}" | ")" | "]") && self.extra.is_empty() && !self.has_selection() {
            self.outdent_for_closer(text.chars().next().unwrap());
        }
        self.insert_all(text);
    }

    /// Enter keeps the line's indent, one level deeper after an opening bracket; between a
    /// bracket pair the closer goes to its own line (`{|}` → three lines, caret in the middle).
    fn smart_newline(&mut self) {
        let indent = self.current_indent();
        let (pre, post) = if self.has_selection() {
            (String::new(), String::new())
        } else {
            let b = &self.buffer;
            let start = b.line_start(b.line_of(self.head));
            let end = b.line_end(b.line_of(self.head));
            (b.slice(start..self.head), b.slice(self.head..end))
        };
        let opener = pre.trim_end().chars().last().filter(|c| matches!(c, '{' | '(' | '['));
        let closer = post.trim_start().chars().next();
        let pair = matches!((opener, closer), (Some('{'), Some('}')) | (Some('('), Some(')')) | (Some('['), Some(']')));
        let unit = self.indent.clone();
        self.buffer.break_group();
        if pair {
            // drop the spaces between caret and closer
            let ws = post.len() - post.trim_start().len();
            self.head += ws;
            self.insert(&format!("\n{indent}{unit}\n{indent}"));
            let caret = self.head - 1 - indent.len();
            self.anchor = caret;
            self.head = caret;
        } else if opener.is_some() {
            self.insert(&format!("\n{indent}{unit}"));
        } else {
            self.insert(&format!("\n{indent}"));
        }
    }

    /// Typing a closing bracket as the first thing on a line lines it up with its opener's line.
    fn outdent_for_closer(&mut self, close: char) {
        let b = &self.buffer;
        let line = b.line_of(self.head);
        let start = b.line_start(line);
        let pre = b.slice(start..self.head);
        if pre.is_empty() || !pre.chars().all(|c| c == ' ' || c == '\t') {
            return;
        }
        let open = match close {
            '}' => '{',
            ')' => '(',
            _ => '[',
        };
        // the unmatched opener before this line
        let before = b.slice(0..start);
        let mut depth = 0i32;
        let mut at = None;
        for (i, ch) in before.char_indices().rev() {
            if ch == close {
                depth += 1;
            } else if ch == open {
                if depth == 0 {
                    at = Some(i);
                    break;
                }
                depth -= 1;
            }
        }
        let Some(at) = at else { return };
        let ol = b.line_text(b.line_of(at));
        let want: String = ol.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
        if want == pre {
            return;
        }
        self.anchor = start;
        self.insert(&want);
    }

    pub fn unmark(&mut self) {
        self.marked = None;
    }
}

/// Foldable regions (header line, last line to hide), from indentation: a line followed by
/// more-indented lines folds them (a closing `}` / `</tag>` at the header's indent stays
/// visible, like IntelliJ's "{...}"); consecutive `import` lines fold into the first one.
pub fn fold_regions(b: &crate::Buffer) -> Vec<(usize, usize)> {
    let n = b.line_count();
    let lines: Vec<String> = (0..n).map(|l| b.line_text(l)).collect();
    let indent = |s: &str| -> Option<usize> {
        if s.trim().is_empty() {
            None
        } else {
            Some(s.chars().take_while(|c| *c == ' ' || *c == '\t').map(|c| if c == '\t' { 4 } else { 1 }).sum())
        }
    };
    let mut out = Vec::new();
    for i in 0..n {
        let Some(ind) = indent(&lines[i]) else { continue };
        // next non-blank line must be deeper
        let Some(next) = (i + 1..n).find(|&j| indent(&lines[j]).is_some()) else { continue };
        if indent(&lines[next]).unwrap_or(0) <= ind {
            continue;
        }
        // last line (non-blank) deeper than the header
        let mut last = next;
        for (j, l) in lines.iter().enumerate().skip(next) {
            match indent(l) {
                Some(d) if d > ind => last = j,
                Some(_) => break,
                None => {}
            }
        }
        if last > i {
            out.push((i, last));
        }
    }
    // import block
    let imports: Vec<usize> = (0..n).filter(|&i| lines[i].starts_with("import ")).collect();
    if imports.len() >= 2 {
        let (first, last) = (imports[0], *imports.last().unwrap());
        out.push((first, last));
    }
    out
}

/// Ranges Ctrl+W may grow to, smallest first: the word, the inside and outside of each
/// enclosing pair ("…", '…', (…), […], {…}), the trimmed line, the full line, the text.
fn extend_candidates(text: &str, cur: Range<usize>) -> Vec<Range<usize>> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let is_w = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80;
    let (mut ws, mut we) = (cur.start, cur.end);
    while ws > 0 && is_w(b[ws - 1]) {
        ws -= 1;
    }
    while we < b.len() && is_w(b[we]) {
        we += 1;
    }
    out.push(ws..we);
    // enclosing pairs: scan outwards with a depth count per bracket kind
    let pairs = [(b'(', b')'), (b'[', b']'), (b'{', b'}')];
    for (o, c) in pairs {
        let mut depth = 0;
        let mut i = cur.start;
        let mut open = None;
        while i > 0 {
            i -= 1;
            if b[i] == c {
                depth += 1;
            } else if b[i] == o {
                if depth == 0 {
                    open = Some(i);
                    break;
                }
                depth -= 1;
            }
        }
        let Some(open) = open else { continue };
        let mut depth = 0;
        let mut j = open + 1;
        while j < b.len() {
            if b[j] == o {
                depth += 1;
            } else if b[j] == c {
                if depth == 0 {
                    break;
                }
                depth -= 1;
            }
            j += 1;
        }
        if j < b.len() && j >= cur.end {
            out.push(open + 1..j);
            out.push(open..j + 1);
        }
    }
    // quotes on the same line
    let ls = text[..cur.start].rfind('\n').map_or(0, |i| i + 1);
    let le = text[cur.end..].find('\n').map_or(text.len(), |i| cur.end + i);
    for q in [b'"', b'\''] {
        if let (Some(s), Some(e)) = (text[ls..cur.start].rfind(q as char).map(|i| ls + i), text[cur.end..le].find(q as char).map(|i| cur.end + i)) {
            out.push(s + 1..e);
            out.push(s..e + 1);
        }
    }
    let line = &text[ls..le];
    let lead = line.len() - line.trim_start().len();
    out.push(ls + lead..ls + line.trim_end().len());
    out.push(ls..le);
    out.push(0..text.len());
    out.sort_by_key(|r| (r.len(), r.start));
    out
}

/// IntelliJ-style default keymap. Keys use gpui's keystroke syntax ("ctrl-shift-z").
pub fn intellij_keymap() -> Vec<(&'static str, Command)> {
    use Command::*;
    vec![
        ("left", Left),
        ("right", Right),
        ("up", Up),
        ("down", Down),
        ("ctrl-left", WordLeft),
        ("ctrl-right", WordRight),
        ("home", LineStart),
        ("end", LineEnd),
        ("ctrl-home", DocStart),
        ("ctrl-end", DocEnd),
        ("pageup", PageUp),
        ("pagedown", PageDown),
        ("shift-left", SelectLeft),
        ("shift-right", SelectRight),
        ("shift-up", SelectUp),
        ("shift-down", SelectDown),
        ("ctrl-shift-left", SelectWordLeft),
        ("ctrl-shift-right", SelectWordRight),
        ("shift-home", SelectLineStart),
        ("shift-end", SelectLineEnd),
        ("ctrl-a", SelectAll),
        ("backspace", Backspace),
        ("delete", Delete),
        ("ctrl-backspace", DeleteWordLeft),
        ("enter", Newline),
        ("tab", Tab),
        ("ctrl-d", DuplicateLine),
        ("ctrl-y", DeleteLine),
        ("ctrl-z", Undo),
        ("ctrl-shift-z", Redo),
        ("ctrl-s", Save),
        ("ctrl-c", Copy),
        ("ctrl-x", Cut),
        ("ctrl-v", Paste),
        ("ctrl-f", Find),
        ("f3", FindNext),
        ("ctrl-/", CommentLine),
        ("ctrl-shift-/", CommentBlock),
        ("ctrl-shift-up", MoveLineUp),
        ("ctrl-shift-down", MoveLineDown),
        ("alt-shift-up", MoveLineUp),
        ("alt-shift-down", MoveLineDown),
        ("shift-tab", Unindent),
        ("ctrl-shift-j", JoinLines),
        ("ctrl-shift-u", ToggleCase),
        ("ctrl-shift-w", ShrinkSelection),
        ("ctrl--", FoldCollapse),
        ("ctrl-=", FoldExpand),
        ("ctrl-+", FoldExpand),
        ("ctrl-shift--", FoldAll),
        ("ctrl-_", FoldAll),
        ("ctrl-shift-=", UnfoldAll),
        ("ctrl-shift-+", UnfoldAll),
        ("alt-j", AddNextOccurrence),
        ("alt-shift-j", RemoveLastOccurrence),
        ("ctrl-alt-shift-j", SelectAllOccurrences),
    ]
}
