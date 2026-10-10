//! Text buffer: a rope plus grouped undo/redo. All positions are byte offsets that
//! always sit on a char boundary; column math goes through graphemes (Vietnamese
//! text may carry combining marks, so one char is not one column).
use ropey::Rope;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, PartialEq)]
struct Edit {
    at: usize,
    old: String,
    new: String,
}

/// One edit in LSP terms (positions before the edit, UTF-16 columns), for incremental didChange.
#[derive(Debug, Clone, PartialEq)]
pub struct TextChange {
    pub start: (usize, usize),
    pub end: (usize, usize),
    pub text: String,
}

#[derive(Debug, Clone)]
struct Group {
    edits: Vec<Edit>,
    sel_before: Range<usize>,
    sel_after: Range<usize>,
    at: Instant,
}

/// Typing within this window, contiguous with the previous edit, undoes as one step.
const GROUP_WINDOW: Duration = Duration::from_millis(800);

pub struct Buffer {
    rope: Rope,
    pub path: Option<PathBuf>,
    undo: Vec<Group>,
    redo: Vec<Group>,
    open_group: bool,
    saved_version: usize,
    version: usize,
    track_changes: bool,
    changes: Vec<TextChange>,
    /// collapsed regions: (header line, last hidden line), lines header+1..=last are hidden.
    /// Kept in step with edits by `apply` (shifted, or dropped when an edit touches them).
    pub folds: Vec<(usize, usize)>,
    /// Some(false) = compound edit started, no edit yet; Some(true) = its group exists
    compound: Option<bool>,
}

impl Buffer {
    pub fn new() -> Self {
        Self::from_text("")
    }

    pub fn from_text(text: &str) -> Self {
        Self {
            rope: Rope::from_str(text),
            path: None,
            undo: Vec::new(),
            redo: Vec::new(),
            open_group: false,
            saved_version: 0,
            version: 0,
            track_changes: false,
            changes: Vec::new(),
            folds: Vec::new(),
            compound: None,
        }
    }

    pub fn open(path: &Path) -> std::io::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let mut b = Self::from_text(&text);
        b.path = Some(path.to_path_buf());
        Ok(b)
    }

    /// Writes through a temp file + rename so a crash never leaves half a file.
    pub fn save(&mut self) -> std::io::Result<()> {
        let path = self.path.clone().ok_or_else(|| std::io::Error::other("buffer has no path"))?;
        let tmp = path.with_file_name(format!(
            ".{}.chodilamide-tmp",
            path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()
        ));
        {
            let mut f = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
            self.rope.write_to(&mut f)?;
        }
        if let Ok(meta) = std::fs::metadata(&path) {
            let _ = std::fs::set_permissions(&tmp, meta.permissions());
        }
        std::fs::rename(&tmp, &path)?;
        self.saved_version = self.version;
        self.open_group = false;
        Ok(())
    }

    /// Replaces the whole text with what is on disk now (file changed outside the IDE) and
    /// counts as saved. Recorded as one edit so a language server stays in sync.
    pub fn reload_text(&mut self, text: &str) {
        let all = 0..self.len();
        self.break_group();
        self.edit(all, text, 0..0, 0..0);
        self.break_group();
        self.saved_version = self.version;
    }

    pub fn is_dirty(&self) -> bool {
        self.version != self.saved_version
    }

    pub fn version(&self) -> usize {
        self.version
    }

    pub fn rope(&self) -> &Rope {
        &self.rope
    }

    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    pub fn len(&self) -> usize {
        self.rope.len_bytes()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn slice(&self, r: Range<usize>) -> String {
        self.rope.byte_slice(r).to_string()
    }

    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    pub fn line_of(&self, byte: usize) -> usize {
        self.rope.byte_to_line(byte.min(self.len()))
    }

    pub fn line_start(&self, line: usize) -> usize {
        self.rope.line_to_byte(line.min(self.line_count()))
    }

    /// Byte offset just before the line break (or at EOF).
    pub fn line_end(&self, line: usize) -> usize {
        if line + 1 >= self.line_count() {
            return self.len();
        }
        let next = self.rope.line_to_byte(line + 1);
        let mut end = next;
        let s = self.slice(self.line_start(line)..next);
        if s.ends_with("\r\n") {
            end -= 2;
        } else if s.ends_with('\n') {
            end -= 1;
        }
        end
    }

    /// Line text without its line break.
    pub fn line_text(&self, line: usize) -> String {
        self.slice(self.line_start(line)..self.line_end(line))
    }

    // ---- grapheme navigation -------------------------------------------------

    pub fn prev_grapheme(&self, byte: usize) -> usize {
        if byte == 0 {
            return 0;
        }
        let line = self.line_of(byte);
        let start = self.line_start(line);
        if byte == start {
            // step over the line break of the previous line
            return self.line_end(line - 1);
        }
        let text = self.slice(start..byte);
        let last = text.grapheme_indices(true).next_back().map(|(i, _)| i).unwrap_or(0);
        start + last
    }

    pub fn next_grapheme(&self, byte: usize) -> usize {
        if byte >= self.len() {
            return self.len();
        }
        let line = self.line_of(byte);
        let end = self.line_end(line);
        if byte >= end {
            return self.line_start(line + 1).max(byte + 1).min(self.len());
        }
        let text = self.slice(byte..end);
        byte + text.graphemes(true).next().map(str::len).unwrap_or(1)
    }

    /// Display column (terminal-style widths) of `byte` within its line.
    pub fn display_col(&self, byte: usize) -> usize {
        let start = self.line_start(self.line_of(byte));
        self.slice(start..byte).graphemes(true).map(grapheme_width).sum()
    }

    /// Byte offset on `line` closest to display column `col` (never mid-grapheme).
    pub fn byte_at_display_col(&self, line: usize, col: usize) -> usize {
        let start = self.line_start(line);
        let text = self.line_text(line);
        let mut w = 0;
        for (i, g) in text.grapheme_indices(true) {
            let gw = grapheme_width(g);
            if w + gw > col {
                return start + i;
            }
            w += gw;
        }
        start + text.len()
    }

    pub fn word_left(&self, byte: usize) -> usize {
        let line = self.line_of(byte);
        let start = self.line_start(line);
        if byte == start {
            return self.prev_grapheme(byte);
        }
        let text = self.slice(start..byte);
        let mut last = 0;
        for (i, w) in text.split_word_bound_indices() {
            if !w.trim().is_empty() {
                last = i;
            }
        }
        start + last
    }

    pub fn word_right(&self, byte: usize) -> usize {
        let line = self.line_of(byte);
        let end = self.line_end(line);
        if byte >= end {
            return self.next_grapheme(byte);
        }
        let text = self.slice(byte..end);
        let mut seen_word = false;
        for (i, w) in text.split_word_bound_indices() {
            if !w.trim().is_empty() {
                if seen_word {
                    return byte + i;
                }
                seen_word = true;
            }
        }
        end
    }

    // ---- UTF-16 (the IME/platform input handler speaks UTF-16 offsets) --------

    pub fn byte_to_utf16(&self, byte: usize) -> usize {
        let c = self.rope.byte_to_char(byte.min(self.len()));
        self.rope.char_to_utf16_cu(c)
    }

    pub fn utf16_to_byte(&self, u: usize) -> usize {
        let c = self.rope.utf16_cu_to_char(u.min(self.rope.len_utf16_cu()));
        self.rope.char_to_byte(c)
    }

    // ---- editing ---------------------------------------------------------------

    /// Replaces `range` with `text`. `sel_before`/`sel_after` are restored by undo/redo.
    pub fn edit(&mut self, range: Range<usize>, text: &str, sel_before: Range<usize>, sel_after: Range<usize>) {
        let range = range.start.min(self.len())..range.end.min(self.len());
        let old = self.slice(range.clone());
        if old.is_empty() && text.is_empty() {
            return;
        }
        self.apply(range.start, &old, text);
        let e = Edit { at: range.start, old, new: text.to_string() };
        self.redo.clear();
        self.version += 1;

        let now = Instant::now();
        // several edits that must undo together (multi-caret typing)
        if let Some(started) = self.compound {
            if started {
                let g = self.undo.last_mut().unwrap();
                g.edits.push(e);
                g.sel_after = sel_after;
            } else {
                self.undo.push(Group { edits: vec![e], sel_before, sel_after, at: now });
                self.compound = Some(true);
            }
            self.open_group = false;
            return;
        }
        let mergeable = self.open_group
            && !e.new.contains('\n')
            && self.undo.last().is_some_and(|g| {
                now.duration_since(g.at) < GROUP_WINDOW
                    && g.edits.last().is_some_and(|p| contiguous(p, &e))
            });
        if mergeable {
            let g = self.undo.last_mut().unwrap();
            g.edits.push(e);
            g.sel_after = sel_after;
            g.at = now;
        } else {
            let breaks = e.new.contains('\n');
            self.undo.push(Group { edits: vec![e], sel_before, sel_after, at: now });
            // a line break closes its own undo step
            self.open_group = !breaks;
            return;
        }
        self.open_group = true;
    }

    /// Edits until `end_compound` form one undo step whatever their positions.
    pub fn begin_compound(&mut self) {
        self.open_group = false;
        self.compound = Some(false);
    }

    pub fn end_compound(&mut self) {
        self.compound = None;
        self.open_group = false;
    }

    /// Next edit starts a new undo step (call on cursor jumps, paste, save...).
    pub fn break_group(&mut self) {
        self.open_group = false;
    }

    /// Start recording edits as `TextChange`s (only while a language server is attached).
    pub fn set_track_changes(&mut self, on: bool) {
        self.track_changes = on;
        self.changes.clear();
    }

    pub fn take_changes(&mut self) -> Vec<TextChange> {
        std::mem::take(&mut self.changes)
    }

    /// (line, UTF-16 column) of a byte offset.
    pub fn lsp_position(&self, byte: usize) -> (usize, usize) {
        let byte = byte.min(self.len());
        let line = self.rope.byte_to_line(byte);
        let c = self.rope.byte_to_char(byte);
        let lc = self.rope.line_to_char(line);
        (line, self.rope.char_to_utf16_cu(c) - self.rope.char_to_utf16_cu(lc))
    }

    /// Byte offset of an LSP (line, UTF-16 column), clamped to the line's end.
    pub fn byte_of_lsp_position(&self, line: usize, col16: usize) -> usize {
        if line >= self.line_count() {
            return self.len();
        }
        let lc = self.rope.line_to_char(line);
        let base16 = self.rope.char_to_utf16_cu(lc);
        let end = self.line_end(line);
        let end16 = self.rope.char_to_utf16_cu(self.rope.byte_to_char(end));
        let target = (base16 + col16).min(end16);
        self.rope.char_to_byte(self.rope.utf16_cu_to_char(target))
    }

    fn apply(&mut self, at: usize, old: &str, new: &str) {
        if !self.folds.is_empty() {
            let first = self.line_of(at);
            let old_lines = old.matches('\n').count();
            let delta = new.matches('\n').count() as isize - old_lines as isize;
            let last = first + old_lines;
            self.folds.retain_mut(|(s, e)| {
                if last < *s {
                    // entirely above the fold: it moves with the lines
                    *s = (*s as isize + delta) as usize;
                    *e = (*e as isize + delta) as usize;
                    true
                } else if first > *e {
                    true
                } else {
                    // on the header without changing lines, the fold stays; anything else unfolds
                    first == *s && last == *s && delta == 0
                }
            });
        }
        if self.track_changes {
            let start = self.lsp_position(at);
            let end = self.lsp_position(at + old.len());
            self.changes.push(TextChange { start, end, text: new.to_string() });
        }
        let c0 = self.rope.byte_to_char(at);
        let c1 = self.rope.byte_to_char(at + old.len());
        self.rope.remove(c0..c1);
        self.rope.insert(c0, new);
    }

    // ---- folding ---------------------------------------------------------------

    /// Hidden line intervals (inclusive), merged and sorted.
    fn hidden(&self) -> Vec<(usize, usize)> {
        let mut v: Vec<(usize, usize)> = self.folds.iter().filter(|(s, e)| e > s).map(|(s, e)| (s + 1, *e)).collect();
        v.sort();
        let mut out: Vec<(usize, usize)> = Vec::new();
        for (a, b) in v {
            match out.last_mut() {
                Some(l) if a <= l.1 + 1 => l.1 = l.1.max(b),
                _ => out.push((a, b)),
            }
        }
        out
    }

    pub fn is_hidden(&self, line: usize) -> bool {
        self.folds.iter().any(|(s, e)| line > *s && line <= *e)
    }

    /// Display row of `line` (a hidden line maps to its fold's header row).
    pub fn row_of_line(&self, line: usize) -> usize {
        let mut row = line;
        for (a, b) in self.hidden() {
            if line > b {
                row -= b - a + 1;
            } else if line >= a {
                row -= line - a + 1;
            }
        }
        row
    }

    /// Buffer line shown at display row `row` (clamped to the last visible line).
    pub fn line_of_row(&self, row: usize) -> usize {
        let mut line = row;
        for (a, b) in self.hidden() {
            if line >= a {
                line += b - a + 1;
            } else {
                break;
            }
        }
        line.min(self.line_count().saturating_sub(1))
    }

    pub fn visible_line_count(&self) -> usize {
        let hidden: usize = self.hidden().iter().map(|(a, b)| b - a + 1).sum();
        self.line_count().saturating_sub(hidden)
    }

    /// First visible line after `line`.
    pub fn next_visible_line(&self, line: usize) -> usize {
        let mut l = line + 1;
        for (a, b) in self.hidden() {
            if l >= a && l <= b {
                l = b + 1;
            }
        }
        l
    }

    /// Returns the selection to restore, or None if nothing to undo.
    pub fn undo(&mut self) -> Option<Range<usize>> {
        let g = self.undo.pop()?;
        for e in g.edits.iter().rev() {
            self.apply(e.at, &e.new, &e.old);
        }
        self.version += 1;
        self.open_group = false;
        let sel = g.sel_before.clone();
        self.redo.push(g);
        Some(sel)
    }

    pub fn redo(&mut self) -> Option<Range<usize>> {
        let g = self.redo.pop()?;
        for e in &g.edits {
            self.apply(e.at, &e.old, &e.new);
        }
        self.version += 1;
        self.open_group = false;
        let sel = g.sel_after.clone();
        self.undo.push(g);
        Some(sel)
    }

    // ---- search ----------------------------------------------------------------

    /// Number of (non-overlapping) occurrences of `needle`.
    pub fn count(&self, needle: &str, case_sensitive: bool) -> usize {
        if needle.is_empty() {
            return 0;
        }
        let hay = self.text();
        if case_sensitive {
            hay.matches(needle).count()
        } else {
            hay.to_lowercase().matches(&needle.to_lowercase()).count()
        }
    }

    /// Next match of `needle` at or after `from`, wrapping around.
    pub fn find(&self, needle: &str, from: usize, case_sensitive: bool) -> Option<Range<usize>> {
        if needle.is_empty() {
            return None;
        }
        let hay = self.text();
        let (hay, needle) = if case_sensitive {
            (hay, needle.to_string())
        } else {
            (hay.to_lowercase(), needle.to_lowercase())
        };
        // lowercasing can change byte lengths for some scripts; fall back to exact search then
        if hay.len() != self.len() {
            return self.find(&needle, from, true);
        }
        let from = from.min(hay.len());
        hay[from..]
            .find(&needle)
            .map(|i| from + i)
            .or_else(|| hay[..from].find(&needle))
            .map(|i| i..i + needle.len())
    }
}

impl Default for Buffer {
    fn default() -> Self {
        Self::new()
    }
}

fn contiguous(prev: &Edit, next: &Edit) -> bool {
    // typing forward, or backspacing backward, or IME replacing its own preedit
    next.at == prev.at + prev.new.len()
        || (next.new.is_empty() && next.at + next.old.len() == prev.at + prev.new.len())
        || next.at == prev.at
}

pub fn grapheme_width(g: &str) -> usize {
    if g == "\t" {
        4
    } else {
        g.width().max(if g.is_empty() { 0 } else { 1 })
    }
}
