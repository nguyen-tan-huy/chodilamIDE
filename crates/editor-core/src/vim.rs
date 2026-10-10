//! Vim emulation (like IdeaVim) on top of `Editor`: Normal / Insert / Visual / Visual Line
//! modes, counts, operators with motions and text objects, registers, `.` repeat, search
//! keys. Keys come in as Vim notation: "d", "A", "$", "<Esc>", "<CR>", "<BS>", "<C-r>".
//! IDE-level things (gd, :w, /) are returned as `VimOut` for the app to run.
use crate::{Command, Editor, Effect};
use std::collections::HashMap;
use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Visual,
    VisualLine,
    VisualBlock,
}

#[derive(Debug, Clone, PartialEq)]
pub enum VimOut {
    /// consumed (maybe waiting for more keys)
    Handled,
    /// not a Vim key in this mode: let the app handle it (typing in Insert mode…)
    Pass,
    /// something only the UI can do (save, clipboard, open find bar)
    Effect(Effect),
    /// an IDE action by name, e.g. "lsp::GotoDefinition" (gd), "workspace::FindAction"
    Action(String),
    /// ':' pressed: open the command line
    CommandLine,
    /// '/' or '?': open search (forward?)
    Search(bool),
    /// n / N: repeat the last search (forward?)
    SearchNext(bool),
    /// text to insert (an Insert-mode mapping that did not complete, e.g. "j" of "jk")
    Insert(String),
}

/// One mapping of a vimrc: `nnoremap <leader>f <Action>(workspace::SearchEverywhere)`.
#[derive(Debug, Clone, PartialEq)]
pub struct Mapping {
    /// 'n', 'v', 'i' (a plain `map` = n + v)
    pub modes: String,
    pub lhs: Vec<String>,
    pub rhs: Vec<String>,
}

/// Splits Vim key notation into keys: "<leader>ff" → ["<leader>", "f", "f"].
pub fn split_keys(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '<' {
            let rest: String = chars.clone().collect();
            if let Some(end) = rest.find('>') {
                let name = &rest[..end];
                if !name.is_empty() && !name.contains(' ') && name.len() < 30 {
                    // <Action>(name) stays one key
                    let mut key = format!("<{name}>");
                    for _ in 0..=end {
                        chars.next();
                    }
                    if name == "Action" && chars.peek() == Some(&'(') {
                        let rest: String = chars.clone().collect();
                        if let Some(close) = rest.find(')') {
                            key.push_str(&rest[..=close]);
                            for _ in 0..=close {
                                chars.next();
                            }
                        }
                    }
                    out.push(normalize_key(&key));
                    continue;
                }
            }
        }
        out.push(c.to_string());
    }
    out
}

/// Vim spellings of the same key → the form `Vim::key` receives.
fn normalize_key(k: &str) -> String {
    let lower = k.to_lowercase();
    match lower.as_str() {
        "<esc>" => "<Esc>".into(),
        "<cr>" | "<enter>" | "<return>" => "<CR>".into(),
        "<bs>" | "<backspace>" => "<BS>".into(),
        "<space>" => " ".into(),
        "<tab>" => "<Tab>".into(),
        "<leader>" => "<leader>".into(),
        "<lt>" => "<".into(),
        _ if lower.starts_with("<c-") && lower.len() == 5 => format!("<C-{}>", &lower[3..4]),
        _ => k.to_string(),
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct VimConfig {
    pub leader: String,
    pub maps: Vec<Mapping>,
    /// `set clipboard=unnamedplus` (default on) / `set clipboard=`
    pub clipboard: Option<bool>,
}

/// Reads a vimrc / .ideavimrc subset: let mapleader, (n|v|x|i)(nore)map, map/noremap, set clipboard.
pub fn parse_vimrc(text: &str) -> VimConfig {
    let mut cfg = VimConfig { leader: "\\".into(), ..Default::default() };
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('"') {
            continue;
        }
        if let Some(v) = l.strip_prefix("let mapleader") {
            let v = v.trim_start_matches([' ', '=']).trim();
            let v = v.trim_matches(['"', '\'']);
            cfg.leader = if v.eq_ignore_ascii_case("<space>") { " ".into() } else { v.to_string() };
            continue;
        }
        if let Some(v) = l.strip_prefix("set clipboard") {
            cfg.clipboard = Some(v.contains("unnamed"));
            continue;
        }
        let mut parts = l.splitn(3, char::is_whitespace);
        let (Some(cmd), Some(lhs), Some(rhs)) = (parts.next(), parts.next(), parts.next()) else { continue };
        let modes = match cmd.trim_end_matches("noremap").trim_end_matches("map") {
            "n" => "n",
            "v" | "x" => "v",
            "i" => "i",
            "" => "nv",
            _ => continue,
        };
        if !(cmd.ends_with("map")) {
            continue;
        }
        let rhs = rhs.trim();
        // IdeaVim's ":action Name<CR>" = <Action>(Name)
        let rhs = match rhs.strip_prefix(":action ").map(|r| r.trim_end_matches("<CR>").trim_end_matches("<cr>").trim()) {
            Some(name) => vec![format!("<Action>({name})")],
            None => split_keys(rhs),
        };
        cfg.maps.push(Mapping { modes: modes.into(), lhs: split_keys(lhs), rhs });
    }
    cfg
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Motion {
    Left,
    Right,
    Up,
    Down,
    WordFwd(bool),
    WordBack(bool),
    WordEnd(bool),
    LineStart,
    FirstNonBlank,
    LineEnd,
    FileStart,
    FileEnd,
    ParaFwd,
    ParaBack,
    MatchPair,
    Find(char, bool, bool), // char, forward, till
    HalfDown,
    HalfUp,
}

impl Motion {
    /// Line-wise when used with an operator (dj, dG…).
    fn linewise(self) -> bool {
        matches!(self, Motion::Up | Motion::Down | Motion::FileStart | Motion::FileEnd | Motion::HalfDown | Motion::HalfUp)
    }
    /// The character under the end position is included (de, df, d$, d%).
    fn inclusive(self) -> bool {
        matches!(self, Motion::WordEnd(_) | Motion::LineEnd | Motion::MatchPair | Motion::Find(_, true, _))
    }
}

#[derive(Debug, Clone, Default)]
pub struct Register {
    pub text: String,
    pub linewise: bool,
}

pub struct Vim {
    pub mode: Mode,
    pending: Vec<String>,
    registers: HashMap<char, Register>,
    last_find: Option<(char, bool, bool)>,
    /// keys of the last change, replayed by `.`
    last_change: Vec<String>,
    recording: Option<Vec<String>>,
    /// Visual mode anchor and the caret (the selection shown is one character wider)
    visual_start: usize,
    visual_caret: usize,
    /// yanks/deletes also go to the system clipboard (clipboard=unnamedplus)
    pub use_clipboard: bool,
    marks: HashMap<char, usize>,
    /// vimrc mappings (lhs with <leader> resolved) and the keys typed towards one
    maps: Vec<Mapping>,
    map_pending: Vec<String>,
    /// q{reg}: recording keys (and inserted text as "\0text")
    macro_rec: Option<(char, Vec<String>)>,
    macros: HashMap<char, Vec<String>>,
    macro_await: Option<char>,
    last_macro: Option<char>,
    macro_depth: usize,
}

impl Default for Vim {
    fn default() -> Self {
        Self::new()
    }
}

fn class(c: char, big: bool) -> u8 {
    if c.is_whitespace() {
        0
    } else if big || c.is_alphanumeric() || c == '_' {
        1
    } else {
        2
    }
}

impl Vim {
    pub fn new() -> Self {
        Self {
            mode: Mode::Normal,
            pending: Vec::new(),
            registers: HashMap::new(),
            last_find: None,
            last_change: Vec::new(),
            recording: None,
            visual_start: 0,
            visual_caret: 0,
            use_clipboard: true,
            marks: HashMap::new(),
            maps: Vec::new(),
            map_pending: Vec::new(),
            macro_rec: None,
            macros: HashMap::new(),
            macro_await: None,
            last_macro: None,
            macro_depth: 0,
        }
    }

    /// Keys typed so far of an unfinished command (shown in the status bar).
    pub fn pending(&self) -> String {
        let rec = self.macro_rec.as_ref().map(|(c, _)| format!("recording @{c} ")).unwrap_or_default();
        format!("{rec}{}{}", self.map_pending.concat(), self.pending.concat())
    }

    pub fn configure(&mut self, cfg: &VimConfig) {
        let leader = split_keys(&cfg.leader);
        self.maps = cfg
            .maps
            .iter()
            .map(|m| Mapping { modes: m.modes.clone(), lhs: m.lhs.iter().flat_map(|k| if k == "<leader>" { leader.clone() } else { vec![k.clone()] }).collect(), rhs: m.rhs.clone() })
            .collect();
        if let Some(c) = cfg.clipboard {
            self.use_clipboard = c;
        }
    }

    fn mode_char(&self) -> char {
        match self.mode {
            Mode::Normal => 'n',
            Mode::Insert => 'i',
            Mode::Visual | Mode::VisualLine | Mode::VisualBlock => 'v',
        }
    }

    /// Would `key` (after what is pending) continue a mapping of the current mode? (Insert
    /// mode: the app routes printable keys here only then.)
    pub fn wants_key(&self, key: &str) -> bool {
        let m = self.mode_char();
        let mut seq = self.map_pending.clone();
        seq.push(key.to_string());
        self.maps.iter().any(|x| x.modes.contains(m) && x.lhs.starts_with(&seq))
    }

    /// A key through the vimrc mappings, then Vim. Several results when a mapping expands.
    pub fn feed(&mut self, ed: &mut Editor, key: &str) -> Vec<VimOut> {
        // macros: q{reg} … q records, @{reg} / @@ replays (Normal mode, between commands)
        if self.mode == Mode::Normal && self.pending.is_empty() && self.map_pending.is_empty() {
            if let Some(kind) = self.macro_await.take() {
                let Some(c) = key.chars().next().filter(|_| key.chars().count() == 1) else { return vec![VimOut::Handled] };
                if kind == 'q' {
                    self.macro_rec = Some((c, Vec::new()));
                    return vec![VimOut::Handled];
                }
                let c = if c == '@' { match self.last_macro { Some(l) => l, None => return vec![VimOut::Handled] } } else { c };
                self.last_macro = Some(c);
                return self.play_macro(ed, c);
            }
            if key == "q" {
                match self.macro_rec.take() {
                    Some((c, keys)) => {
                        self.macros.insert(c, keys);
                    }
                    None => self.macro_await = Some('q'),
                }
                return vec![VimOut::Handled];
            }
            if key == "@" {
                self.macro_await = Some('@');
                return vec![VimOut::Handled];
            }
        }
        if let Some((_, rec)) = self.macro_rec.as_mut() {
            rec.push(key.to_string());
        }
        let m = self.mode_char();
        self.map_pending.push(key.to_string());
        let seq = self.map_pending.clone();
        let exact = self.maps.iter().find(|x| x.modes.contains(m) && x.lhs == seq).cloned();
        let longer = self.maps.iter().any(|x| x.modes.contains(m) && x.lhs.len() > seq.len() && x.lhs.starts_with(&seq));
        if longer && exact.is_none() {
            return vec![VimOut::Handled];
        }
        self.map_pending.clear();
        if let Some(map) = exact {
            let mut outs = Vec::new();
            for k in &map.rhs {
                if let Some(name) = k.strip_prefix("<Action>(").and_then(|r| r.strip_suffix(')')) {
                    outs.push(VimOut::Action(name.to_string()));
                } else {
                    match self.key(ed, k) {
                        // an Insert-mode rhs typing text
                        VimOut::Pass => outs.push(VimOut::Insert(k.clone())),
                        o => outs.push(o),
                    }
                }
            }
            return outs;
        }
        // no mapping: the keys as typed
        seq.iter()
            .map(|k| match self.key(ed, k) {
                VimOut::Pass if self.mode == Mode::Insert && seq.len() > 1 && k.chars().count() == 1 => VimOut::Insert(k.clone()),
                o => o,
            })
            .collect()
    }

    pub fn mode_label(&self) -> &'static str {
        match self.mode {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Visual => "VISUAL",
            Mode::VisualLine => "V-LINE",
            Mode::VisualBlock => "V-BLOCK",
        }
    }

    /// Text typed in Insert mode (goes through the app/IME), recorded for `.` and macros.
    pub fn inserted(&mut self, text: &str) {
        if let Some((_, r)) = self.macro_rec.as_mut() {
            r.push(format!("\u{0}{text}"));
        }
        self.inserted_for_repeat(text);
    }

    /// Text from `VimOut::Insert` (its keys are already in the macro): only `.` records it.
    pub fn inserted_for_repeat(&mut self, text: &str) {
        if let Some(r) = self.recording.as_mut() {
            r.push(format!("\u{0}{text}"));
        }
    }

    fn play_macro(&mut self, ed: &mut Editor, c: char) -> Vec<VimOut> {
        let Some(keys) = self.macros.get(&c).cloned() else { return vec![VimOut::Handled] };
        if self.macro_depth > 20 {
            return vec![VimOut::Handled];
        }
        self.macro_depth += 1;
        let mut outs = Vec::new();
        for k in keys {
            if let Some(t) = k.strip_prefix('\u{0}') {
                ed.insert(t);
                self.inserted_for_repeat(t);
                continue;
            }
            for o in self.feed(ed, &k) {
                match o {
                    VimOut::Insert(t) => {
                        ed.insert(&t);
                        self.inserted_for_repeat(&t);
                    }
                    VimOut::Handled | VimOut::Pass => {}
                    o => outs.push(o),
                }
            }
        }
        self.macro_depth -= 1;
        if outs.is_empty() {
            outs.push(VimOut::Handled);
        }
        outs
    }

    pub fn register(&self, r: char) -> Option<&Register> {
        self.registers.get(&r)
    }

    /// The clipboard's text as the unnamed register (after the app read it for `p`).
    pub fn set_register(&mut self, r: char, text: String, linewise: bool) {
        self.registers.insert(r, Register { text, linewise });
    }

    // ---- positions --------------------------------------------------------------

    fn text(ed: &Editor) -> String {
        ed.buffer.text()
    }

    fn line_bounds(ed: &Editor, line: usize) -> (usize, usize) {
        (ed.buffer.line_start(line), ed.buffer.line_end(line))
    }

    /// Normal mode: the caret sits ON a character, never after the last one of a line.
    pub fn clamp(&self, ed: &mut Editor) {
        if self.mode != Mode::Normal {
            return;
        }
        let line = ed.buffer.line_of(ed.head);
        let (s, e) = Self::line_bounds(ed, line);
        if ed.head >= e && e > s {
            let p = ed.buffer.prev_grapheme(e);
            ed.head = p.max(s);
            ed.anchor = ed.head;
        }
    }

    fn motion_target(&mut self, ed: &Editor, m: Motion, count: usize) -> Option<usize> {
        let b = &ed.buffer;
        let mut pos = ed.head;
        let text = Self::text(ed);
        let chars_at = |p: usize| text[p..].chars().next();
        for _ in 0..count.max(1) {
            pos = match m {
                Motion::Left => {
                    let s = b.line_start(b.line_of(pos));
                    if pos > s { b.prev_grapheme(pos) } else { pos }
                }
                Motion::Right => {
                    let e = b.line_end(b.line_of(pos));
                    let n = b.next_grapheme(pos);
                    if n < e || (self.mode != Mode::Normal && n <= e) { n } else { pos }
                }
                Motion::Up | Motion::Down | Motion::HalfDown | Motion::HalfUp => {
                    let line = b.line_of(pos);
                    let col = b.display_col(ed.head);
                    let step = match m {
                        Motion::HalfDown | Motion::HalfUp => (ed.page_lines / 2).max(1) as isize,
                        _ => 1,
                    };
                    let dir = if matches!(m, Motion::Up | Motion::HalfUp) { -step } else { step };
                    let row = b.row_of_line(line) as isize + dir;
                    let max = b.visible_line_count().saturating_sub(1) as isize;
                    let t = b.line_of_row(row.clamp(0, max) as usize);
                    b.byte_at_display_col(t, col)
                }
                Motion::WordFwd(big) => {
                    let mut it = text[pos..].char_indices().peekable();
                    let first = it.peek().map(|(_, c)| class(*c, big));
                    let mut p = text.len();
                    let mut seen_space = false;
                    for (i, c) in it {
                        let cl = class(c, big);
                        if cl == 0 {
                            seen_space = true;
                            continue;
                        }
                        if seen_space || Some(cl) != first {
                            p = pos + i;
                            break;
                        }
                    }
                    p
                }
                Motion::WordEnd(big) => {
                    let mut p = pos;
                    let mut it = text[pos..].char_indices().skip(1).peekable();
                    // skip spaces, then to the end of the next word
                    while let Some((_, c)) = it.peek() {
                        if class(*c, big) == 0 {
                            it.next();
                        } else {
                            break;
                        }
                    }
                    if let Some((i, c)) = it.next() {
                        let cl = class(c, big);
                        p = pos + i;
                        for (j, c2) in it {
                            if class(c2, big) != cl {
                                break;
                            }
                            p = pos + j;
                        }
                    }
                    p
                }
                Motion::WordBack(big) => {
                    let before: Vec<(usize, char)> = text[..pos].char_indices().collect();
                    let mut k = before.len();
                    while k > 0 && class(before[k - 1].1, big) == 0 {
                        k -= 1;
                    }
                    if k == 0 {
                        0
                    } else {
                        let cl = class(before[k - 1].1, big);
                        while k > 0 && class(before[k - 1].1, big) == cl {
                            k -= 1;
                        }
                        before.get(k).map_or(0, |(i, _)| *i)
                    }
                }
                Motion::LineStart => b.line_start(b.line_of(pos)),
                Motion::FirstNonBlank => {
                    let line = b.line_of(pos);
                    let t = b.line_text(line);
                    b.line_start(line) + (t.len() - t.trim_start().len())
                }
                Motion::LineEnd => {
                    let line = b.line_of(pos);
                    let (s, e) = Self::line_bounds(ed, line);
                    if e > s { b.prev_grapheme(e) } else { s }
                }
                Motion::FileStart => b.line_start(count.saturating_sub(1).min(b.line_count().saturating_sub(1)) * usize::from(count > 1)),
                Motion::FileEnd => {
                    let l = b.line_count().saturating_sub(1);
                    b.line_start(l)
                }
                Motion::ParaFwd => {
                    let mut l = b.line_of(pos) + 1;
                    while l < b.line_count() && !b.line_text(l).trim().is_empty() {
                        l += 1;
                    }
                    if l >= b.line_count() { b.len() } else { b.line_start(l) }
                }
                Motion::ParaBack => {
                    let mut l = b.line_of(pos).saturating_sub(1);
                    while l > 0 && !b.line_text(l).trim().is_empty() {
                        l -= 1;
                    }
                    b.line_start(l)
                }
                Motion::MatchPair => {
                    let line_end = b.line_end(b.line_of(pos));
                    // the first bracket at or after the caret on this line
                    let (i, c) = text[pos..line_end].char_indices().find(|(_, c)| "()[]{}".contains(*c))?;
                    let at = pos + i;
                    let (open, close, fwd) = match c {
                        '(' => ('(', ')', true),
                        ')' => ('(', ')', false),
                        '[' => ('[', ']', true),
                        ']' => ('[', ']', false),
                        '{' => ('{', '}', true),
                        _ => ('{', '}', false),
                    };
                    let mut depth = 0i32;
                    if fwd {
                        let mut found = None;
                        for (j, ch) in text[at..].char_indices() {
                            if ch == open {
                                depth += 1;
                            } else if ch == close {
                                depth -= 1;
                                if depth == 0 {
                                    found = Some(at + j);
                                    break;
                                }
                            }
                        }
                        found?
                    } else {
                        let mut found = None;
                        for (j, ch) in text[..=at].char_indices().rev() {
                            if ch == close {
                                depth += 1;
                            } else if ch == open {
                                depth -= 1;
                                if depth == 0 {
                                    found = Some(j);
                                    break;
                                }
                            }
                        }
                        found?
                    }
                }
                Motion::Find(ch, fwd, till) => {
                    let line = b.line_of(pos);
                    let (s, e) = Self::line_bounds(ed, line);
                    if fwd {
                        let from = b.next_grapheme(pos).min(e);
                        let from = if till && from < e { b.next_grapheme(from).min(e) } else { from };
                        let i = text[from..e].find(ch)? + from;
                        if till { b.prev_grapheme(i) } else { i }
                    } else {
                        let to = if till && pos > s { b.prev_grapheme(pos) } else { pos };
                        let i = text[s..to].rfind(ch)? + s;
                        if till { b.next_grapheme(i) } else { i }
                    }
                }
            };
            let _ = chars_at;
        }
        Some(pos)
    }

    /// Text object range (iw, a", i(, it…) around the caret.
    fn text_object(ed: &Editor, inner: bool, obj: char) -> Option<Range<usize>> {
        let text = Self::text(ed);
        let pos = ed.head.min(text.len());
        match obj {
            'w' | 'W' => {
                let big = obj == 'W';
                let c = text[pos..].chars().next()?;
                let cl = class(c, big);
                let mut s = pos;
                for (i, ch) in text[..pos].char_indices().rev() {
                    if class(ch, big) != cl {
                        break;
                    }
                    s = i;
                }
                let from = pos + c.len_utf8();
                let mut e = from + text[from..].char_indices().find(|(_, ch)| class(*ch, big) != cl).map_or(text.len() - from, |(i, _)| i);
                if !inner {
                    // aw: plus the following spaces (or the preceding ones at line end)
                    let after: usize = text[e..].chars().take_while(|c| *c == ' ' || *c == '\t').map(char::len_utf8).sum();
                    if after > 0 {
                        e += after;
                    } else {
                        let before: usize = text[..s].chars().rev().take_while(|c| *c == ' ' || *c == '\t').map(char::len_utf8).sum();
                        s -= before;
                    }
                }
                Some(s..e)
            }
            '"' | '\'' | '`' => {
                let line = ed.buffer.line_of(pos);
                let (ls, le) = Self::line_bounds(ed, line);
                let l = &text[ls..le];
                let rel = pos - ls;
                let quotes: Vec<usize> = l.match_indices(obj).map(|(i, _)| i).collect();
                // the pair around the caret, else the next pair on the line
                let pair = quotes.chunks(2).find(|p| p.len() == 2 && p[0] <= rel && rel <= p[1]).or_else(|| quotes.chunks(2).find(|p| p.len() == 2 && p[0] > rel))?;
                let (a, b) = (ls + pair[0], ls + pair[1]);
                Some(if inner { a + 1..b } else { a..b + 1 })
            }
            '(' | ')' | 'b' | '[' | ']' | '{' | '}' | 'B' | '<' | '>' => {
                let (open, close) = match obj {
                    '(' | ')' | 'b' => ('(', ')'),
                    '[' | ']' => ('[', ']'),
                    '<' | '>' => ('<', '>'),
                    _ => ('{', '}'),
                };
                let mut depth = 0;
                let mut start = None;
                for (i, ch) in text[..=pos.min(text.len().saturating_sub(1))].char_indices().rev() {
                    if ch == close && i != pos {
                        depth += 1;
                    } else if ch == open {
                        if depth == 0 {
                            start = Some(i);
                            break;
                        }
                        depth -= 1;
                    }
                }
                let s = start?;
                let mut depth = 0;
                let mut end = None;
                for (i, ch) in text[s..].char_indices() {
                    if ch == open {
                        depth += 1;
                    } else if ch == close {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(s + i);
                            break;
                        }
                    }
                }
                let e = end?;
                if inner {
                    // i{ spanning lines: the lines between the braces
                    let mut a = s + 1;
                    let mut z = e;
                    if text[a..z].starts_with('\n') {
                        a += 1;
                        if let Some(nl) = text[a..z].rfind('\n') {
                            z = a + nl + 1;
                        }
                    }
                    Some(a..z)
                } else {
                    Some(s..e + 1)
                }
            }
            't' => {
                // the innermost <tag>…</tag> around the caret
                let mut search = pos;
                loop {
                    let open = text[..search.min(text.len())].rfind('<')?;
                    let rest = &text[open + 1..];
                    if rest.starts_with('/') || rest.starts_with('!') || rest.starts_with('?') {
                        search = open;
                        continue;
                    }
                    let name: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == ':' || *c == '-' || *c == '_').collect();
                    let gt = open + 1 + rest.find('>')?;
                    if text[..gt].ends_with('/') {
                        search = open;
                        continue;
                    }
                    let close_tag = format!("</{name}>");
                    if let Some(c) = text[gt..].find(&close_tag) {
                        let c = gt + c;
                        if c + close_tag.len() > pos {
                            return Some(if inner { gt + 1..c } else { open..c + close_tag.len() });
                        }
                    }
                    if open == 0 {
                        return None;
                    }
                    search = open;
                }
            }
            'p' => {
                let b = &ed.buffer;
                let line = b.line_of(pos);
                let blank = |l: usize| b.line_text(l).trim().is_empty();
                let kind = blank(line);
                let mut s = line;
                while s > 0 && blank(s - 1) == kind {
                    s -= 1;
                }
                let mut e = line;
                while e + 1 < b.line_count() && blank(e + 1) == kind {
                    e += 1;
                }
                Some(b.line_start(s)..b.line_start(e + 1).max(b.line_end(e)))
            }
            _ => None,
        }
    }

    // ---- editing helpers ---------------------------------------------------------

    fn yank(&mut self, ed: &Editor, r: Range<usize>, linewise: bool, reg: Option<char>) -> Option<Effect> {
        let mut text = ed.buffer.slice(r);
        if linewise && !text.ends_with('\n') {
            text.push('\n');
        }
        let reg = reg.unwrap_or('"');
        self.registers.insert(reg, Register { text: text.clone(), linewise });
        if reg == '"' {
            self.registers.insert('0', Register { text: text.clone(), linewise });
        }
        (self.use_clipboard && (reg == '"' || reg == '+')).then_some(Effect::CopyToClipboard(text))
    }

    fn delete(&mut self, ed: &mut Editor, r: Range<usize>) {
        if r.is_empty() {
            return;
        }
        ed.buffer.break_group();
        ed.anchor = r.start;
        ed.head = r.end;
        ed.insert("");
        ed.buffer.break_group();
    }

    /// Whole-line range for lines first..=last (with the trailing newline, or the leading one
    /// for the last line of the file).
    fn lines_range(ed: &Editor, first: usize, last: usize) -> Range<usize> {
        let b = &ed.buffer;
        let s = b.line_start(first);
        let e = if last + 1 < b.line_count() { b.line_start(last + 1) } else { b.len() };
        if e == b.len() && s > 0 && !b.slice(s..e).ends_with('\n') && last + 1 >= b.line_count() {
            // deleting the last line: take the newline before it
            return s - 1..e;
        }
        s..e
    }

    fn apply_operator(&mut self, ed: &mut Editor, op: char, r: Range<usize>, linewise: bool, reg: Option<char>) -> VimOut {
        let mut out = VimOut::Handled;
        match op {
            'y' => {
                if let Some(e) = self.yank(ed, r.clone(), linewise, reg) {
                    out = VimOut::Effect(e);
                }
                ed.set_cursor(r.start, false);
            }
            'd' => {
                if let Some(e) = self.yank(ed, r.clone(), linewise, reg) {
                    out = VimOut::Effect(e);
                }
                self.delete(ed, r);
                if linewise {
                    let t = self.motion_target(ed, Motion::FirstNonBlank, 1).unwrap_or(ed.head);
                    ed.set_cursor(t, false);
                }
            }
            'c' => {
                let r = if linewise {
                    // cc keeps the line (and its indent) and its newline
                    let first = ed.buffer.line_of(r.start);
                    let t = ed.buffer.line_text(first);
                    let indent = t.len() - t.trim_start().len();
                    let end = if r.end > 0 && ed.buffer.slice(r.end - 1..r.end) == "\n" { r.end - 1 } else { r.end };
                    ed.buffer.line_start(first) + indent..end
                } else {
                    r
                };
                if let Some(e) = self.yank(ed, r.clone(), linewise, reg) {
                    out = VimOut::Effect(e);
                }
                self.delete(ed, r);
                self.enter_insert();
            }
            '>' | '<' => {
                let (f, l) = (ed.buffer.line_of(r.start), ed.buffer.line_of(r.end.saturating_sub(1).max(r.start)));
                ed.anchor = ed.buffer.line_start(f);
                ed.head = ed.buffer.line_end(l);
                if f != l || op == '<' {
                    ed.run(if op == '>' { Command::Tab } else { Command::Unindent });
                } else {
                    let unit = ed.indent.clone();
                    let at = ed.buffer.line_start(f);
                    ed.set_cursor(at, false);
                    ed.insert(&unit);
                }
                let t = self.motion_target(ed, Motion::FirstNonBlank, 1).unwrap_or(ed.head);
                ed.set_cursor(t, false);
            }
            '~' | 'u' | 'U' => {
                let s = ed.buffer.slice(r.clone());
                let new: String = match op {
                    'u' => s.to_lowercase(),
                    'U' => s.to_uppercase(),
                    _ => s.chars().map(|c| if c.is_uppercase() { c.to_lowercase().collect::<String>() } else { c.to_uppercase().collect() }).collect(),
                };
                ed.buffer.break_group();
                ed.anchor = r.start;
                ed.head = r.end;
                ed.insert(&new);
                ed.buffer.break_group();
                ed.set_cursor(r.start, false);
            }
            _ => {}
        }
        out
    }

    fn enter_insert(&mut self) {
        self.mode = Mode::Insert;
    }

    fn finish_change(&mut self) {
        if self.mode != Mode::Insert {
            if let Some(r) = self.recording.take() {
                self.last_change = r;
            }
        }
    }

    fn put(&mut self, ed: &mut Editor, after: bool, reg: Option<char>, count: usize) -> VimOut {
        let Some(r) = self.registers.get(&reg.unwrap_or('"')).cloned() else { return VimOut::Effect(Effect::RequestPaste) };
        let text = r.text.repeat(count.max(1));
        ed.buffer.break_group();
        if r.linewise {
            let line = ed.buffer.line_of(ed.head);
            let at = if after { ed.buffer.line_start(line + 1) } else { ed.buffer.line_start(line) };
            let at = if after && at == ed.buffer.len() && !ed.buffer.text().ends_with('\n') {
                ed.set_cursor(at, false);
                ed.insert("\n");
                ed.buffer.len()
            } else {
                at
            };
            ed.set_cursor(at, false);
            ed.insert(&text);
            ed.set_cursor(at, false);
            let t = self.motion_target(ed, Motion::FirstNonBlank, 1).unwrap_or(at);
            ed.set_cursor(t, false);
        } else {
            let line_end = ed.buffer.line_end(ed.buffer.line_of(ed.head));
            let at = if after && ed.head < line_end { ed.buffer.next_grapheme(ed.head) } else { ed.head };
            ed.set_cursor(at, false);
            ed.insert(&text);
            let end = ed.head;
            ed.set_cursor(ed.buffer.prev_grapheme(end).max(at), false);
        }
        ed.buffer.break_group();
        VimOut::Handled
    }

    // ---- key handling -------------------------------------------------------------

    /// One key. In Insert mode only <Esc> (and <C-o>, <C-w>…) are Vim's; the rest is `Pass`.
    pub fn key(&mut self, ed: &mut Editor, key: &str) -> VimOut {
        match self.mode {
            Mode::Insert => self.key_insert(ed, key),
            Mode::Normal | Mode::Visual | Mode::VisualLine | Mode::VisualBlock => {
                self.pending.push(key.to_string());
                let keys = self.pending.clone();
                match self.run_normal(ed, &keys) {
                    Some(out) => {
                        self.pending.clear();
                        self.clamp(ed);
                        out
                    }
                    None => VimOut::Handled,
                }
            }
        }
    }

    fn key_insert(&mut self, ed: &mut Editor, key: &str) -> VimOut {
        match key {
            "<Esc>" | "<C-[>" | "<C-c>" => {
                self.mode = Mode::Normal;
                // after a block insert (several carets) Vim returns to the top-left one
                if !ed.extra.is_empty() {
                    let top = ed.extra.iter().map(|r| r.end).chain([ed.head]).min().unwrap_or(ed.head);
                    ed.extra.clear();
                    ed.set_cursor(top, false);
                }
                if let Some(r) = self.recording.as_mut() {
                    r.push("<Esc>".into());
                }
                self.finish_change();
                // Esc steps back onto the last inserted character
                let s = ed.buffer.line_start(ed.buffer.line_of(ed.head));
                if ed.head > s {
                    let p = ed.buffer.prev_grapheme(ed.head);
                    ed.set_cursor(p, false);
                }
                ed.buffer.break_group();
                VimOut::Handled
            }
            "<C-w>" => {
                ed.run(Command::DeleteWordLeft);
                VimOut::Handled
            }
            "<C-u>" => {
                let s = ed.buffer.line_start(ed.buffer.line_of(ed.head));
                ed.anchor = s;
                ed.insert("");
                VimOut::Handled
            }
            _ => VimOut::Pass,
        }
    }

    /// None = incomplete (more keys needed).
    fn run_normal(&mut self, ed: &mut Editor, keys: &[String]) -> Option<VimOut> {
        let mut i = 0;
        let k = |i: usize| keys.get(i).map(String::as_str);
        // register "x
        let mut reg = None;
        if k(0) == Some("\"") {
            let r = k(1)?.chars().next()?;
            reg = Some(r);
            i = 2;
        }
        // count
        let mut count_s = String::new();
        while let Some(c) = k(i) {
            if c.len() == 1 && c.chars().all(|ch| ch.is_ascii_digit()) && !(c == "0" && count_s.is_empty()) {
                count_s.push_str(c);
                i += 1;
            } else {
                break;
            }
        }
        let count: usize = count_s.parse().unwrap_or(1);
        let has_count = !count_s.is_empty();
        let key = k(i)?;
        let visual = matches!(self.mode, Mode::Visual | Mode::VisualLine | Mode::VisualBlock);

        // motions (move, or extend in Visual)
        if let Some(res) = self.parse_motion(keys, i, count, has_count) {
            let (m, _) = res?;
            if visual {
                ed.head = self.visual_caret;
            }
            let target = self.motion_target(ed, m, count)?;
            if m == Motion::FileEnd && has_count {
                let t = ed.buffer.line_start(count.saturating_sub(1).min(ed.buffer.line_count().saturating_sub(1)));
                ed.head = t;
            } else {
                ed.head = target;
            }
            if !visual {
                ed.anchor = ed.head;
            }
            if m.linewise() || matches!(m, Motion::FileStart | Motion::FileEnd) {
                if !visual {
                    let t = self.motion_target(ed, Motion::FirstNonBlank, 1).unwrap_or(ed.head);
                    ed.head = t;
                    ed.anchor = t;
                }
            }
            ed.buffer.break_group();
            if visual {
                self.update_visual(ed);
            }
            return Some(VimOut::Handled);
        }

        if visual {
            return self.visual_key(ed, keys, i, reg);
        }

        let change = |s: &mut Self| {
            s.recording = Some(keys.to_vec());
        };
        let out = match key {
            "i" | "a" | "I" | "A" | "o" | "O" => {
                change(self);
                let b = &ed.buffer;
                let line = b.line_of(ed.head);
                match key {
                    "a" => {
                        let e = b.line_end(line);
                        if ed.head < e {
                            let n = b.next_grapheme(ed.head);
                            ed.set_cursor(n, false);
                        }
                    }
                    "I" => {
                        let t = self.motion_target(ed, Motion::FirstNonBlank, 1).unwrap_or(ed.head);
                        ed.set_cursor(t, false);
                    }
                    "A" => {
                        let e = b.line_end(line);
                        ed.set_cursor(e, false);
                    }
                    "o" => {
                        let e = b.line_end(line);
                        let t = b.line_text(line);
                        let indent: String = t.chars().take_while(|c| c.is_whitespace()).collect();
                        ed.set_cursor(e, false);
                        ed.buffer.break_group();
                        ed.insert(&format!("\n{indent}"));
                    }
                    "O" => {
                        let s = b.line_start(line);
                        let t = b.line_text(line);
                        let indent: String = t.chars().take_while(|c| c.is_whitespace()).collect();
                        ed.set_cursor(s, false);
                        ed.buffer.break_group();
                        ed.insert(&format!("{indent}\n"));
                        ed.set_cursor(s + indent.len(), false);
                    }
                    _ => {}
                }
                ed.buffer.break_group();
                self.enter_insert();
                VimOut::Handled
            }
            "v" => {
                self.mode = Mode::Visual;
                self.visual_start = ed.head;
                self.update_visual(ed);
                VimOut::Handled
            }
            "<C-v>" => {
                self.mode = Mode::VisualBlock;
                self.visual_start = ed.head;
                self.update_visual(ed);
                VimOut::Handled
            }
            "V" => {
                self.mode = Mode::VisualLine;
                self.visual_start = ed.head;
                self.update_visual(ed);
                VimOut::Handled
            }
            "x" | "X" | "s" => {
                change(self);
                let line = ed.buffer.line_of(ed.head);
                let (s, e) = Self::line_bounds(ed, line);
                let r = if key == "X" {
                    let mut a = ed.head;
                    for _ in 0..count {
                        if a > s {
                            a = ed.buffer.prev_grapheme(a);
                        }
                    }
                    a..ed.head
                } else {
                    let mut z = ed.head;
                    for _ in 0..count {
                        if z < e {
                            z = ed.buffer.next_grapheme(z);
                        }
                    }
                    ed.head..z
                };
                let out = self.apply_operator(ed, if key == "s" { 'c' } else { 'd' }, r, false, reg);
                self.finish_change();
                out
            }
            "D" | "C" => {
                change(self);
                let e = ed.buffer.line_end(ed.buffer.line_of(ed.head));
                let out = self.apply_operator(ed, if key == "D" { 'd' } else { 'c' }, ed.head..e, false, reg);
                self.finish_change();
                out
            }
            "S" => {
                change(self);
                let line = ed.buffer.line_of(ed.head);
                let r = Self::lines_range(ed, line, line);
                let out = self.apply_operator(ed, 'c', r, true, reg);
                self.finish_change();
                out
            }
            "Y" => {
                let line = ed.buffer.line_of(ed.head);
                let last = (line + count - 1).min(ed.buffer.line_count().saturating_sub(1));
                let r = Self::lines_range(ed, line, last);
                let at = ed.head;
                let out = self.apply_operator(ed, 'y', r, true, reg);
                ed.set_cursor(at, false);
                out
            }
            "p" | "P" => {
                change(self);
                let out = self.put(ed, key == "p", reg, count);
                self.finish_change();
                out
            }
            "J" => {
                change(self);
                for _ in 0..count.max(2) - 1 {
                    ed.run(Command::JoinLines);
                }
                self.finish_change();
                VimOut::Handled
            }
            "~" => {
                change(self);
                let e = ed.buffer.line_end(ed.buffer.line_of(ed.head));
                let mut z = ed.head;
                for _ in 0..count {
                    if z < e {
                        z = ed.buffer.next_grapheme(z);
                    }
                }
                let at = ed.head;
                self.apply_operator(ed, '~', at..z, false, None);
                ed.set_cursor(z.min(e), false);
                self.finish_change();
                VimOut::Handled
            }
            "r" => {
                let c = k(i + 1)?;
                if c == "<Esc>" {
                    return Some(VimOut::Handled);
                }
                change(self);
                let ch = if c == "<CR>" { "\n".to_string() } else { c.to_string() };
                let e = ed.buffer.line_end(ed.buffer.line_of(ed.head));
                let mut z = ed.head;
                for _ in 0..count {
                    if z < e {
                        z = ed.buffer.next_grapheme(z);
                    }
                }
                let at = ed.head;
                let n = ed.buffer.slice(at..z).chars().count();
                ed.buffer.break_group();
                ed.anchor = at;
                ed.head = z;
                ed.insert(&ch.repeat(n));
                ed.buffer.break_group();
                ed.set_cursor(at + ch.len() * n.saturating_sub(1), false);
                self.finish_change();
                VimOut::Handled
            }
            "u" => {
                for _ in 0..count {
                    ed.run(Command::Undo);
                }
                VimOut::Handled
            }
            "<C-r>" => {
                for _ in 0..count {
                    ed.run(Command::Redo);
                }
                VimOut::Handled
            }
            "." => {
                self.pending.clear();
                let keys = self.last_change.clone();
                for _ in 0..count {
                    self.replay(ed, &keys);
                }
                return Some(VimOut::Handled);
            }
            ":" => VimOut::CommandLine,
            "/" => VimOut::Search(true),
            "?" => VimOut::Search(false),
            "n" => VimOut::SearchNext(true),
            "N" => VimOut::SearchNext(false),
            "*" | "#" => {
                let r = Self::text_object(ed, true, 'w');
                if let Some(r) = r {
                    let word = ed.buffer.slice(r.clone());
                    self.registers.insert('/', Register { text: word, linewise: false });
                    ed.anchor = r.start;
                    ed.head = r.end;
                }
                VimOut::SearchNext(key == "*")
            }
            "m" => {
                let c = k(i + 1)?.chars().next()?;
                self.marks.insert(c, ed.head);
                VimOut::Handled
            }
            "`" | "'" => {
                let c = k(i + 1)?.chars().next()?;
                if let Some(&p) = self.marks.get(&c) {
                    ed.set_cursor(p.min(ed.buffer.len()), false);
                    if key == "'" {
                        let t = self.motion_target(ed, Motion::FirstNonBlank, 1).unwrap_or(ed.head);
                        ed.set_cursor(t, false);
                    }
                }
                VimOut::Handled
            }
            "g" => match k(i + 1)? {
                "d" => VimOut::Action("lsp::GotoDefinition".into()),
                "r" => VimOut::Action("lsp::FindUsages".into()),
                "i" => VimOut::Action("lsp::GotoImplementation".into()),
                "D" => VimOut::Action("lsp::GotoSuper".into()),
                "h" => VimOut::Action("lsp::Hover".into()),
                "c" => {
                    // gcc: the line(s); gc{motion} / gc{text object}: the lines it covers
                    let line = ed.buffer.line_of(ed.head);
                    let (first, last) = match k(i + 2)? {
                        "c" => (line, (line + count - 1).min(ed.buffer.line_count().saturating_sub(1))),
                        o @ ("i" | "a") => {
                            let obj = k(i + 3)?.chars().next()?;
                            let Some(r) = Self::text_object(ed, o == "i", obj) else { return Some(VimOut::Handled) };
                            (ed.buffer.line_of(r.start), ed.buffer.line_of(r.end.saturating_sub(1).max(r.start)))
                        }
                        _ => match self.parse_motion(keys, i + 2, count, false) {
                            Some(None) => return None,
                            Some(Some((m, _))) => {
                                let Some(t) = self.motion_target(ed, m, count) else { return Some(VimOut::Handled) };
                                let t = if m == Motion::FileEnd { ed.buffer.len() } else { t };
                                let lt = ed.buffer.line_of(t);
                                (line.min(lt), line.max(lt))
                            }
                            None => return Some(VimOut::Handled),
                        },
                    };
                    ed.anchor = ed.buffer.line_start(first);
                    ed.head = ed.buffer.line_end(last);
                    ed.run(Command::CommentLine);
                    ed.set_cursor(ed.buffer.line_start(first), false);
                    let t = self.motion_target(ed, Motion::FirstNonBlank, 1).unwrap_or(ed.head);
                    ed.set_cursor(t, false);
                    VimOut::Handled
                }
                "t" => VimOut::Action("workspace::NextTab".into()),
                "T" => VimOut::Action("workspace::PrevTab".into()),
                "~" | "u" | "U" => return self.operator(ed, keys, i + 1, k(i + 1)?.chars().next()?, count, reg),
                "J" => {
                    let line = ed.buffer.line_of(ed.head);
                    let e = ed.buffer.line_end(line);
                    let n = ed.buffer.line_start(line + 1);
                    let nt = ed.buffer.line_text(line + 1);
                    let lead = nt.len() - nt.trim_start().len();
                    ed.buffer.break_group();
                    ed.anchor = e;
                    ed.head = n + lead;
                    ed.insert("");
                    VimOut::Handled
                }
                _ => VimOut::Handled,
            },
            "K" => VimOut::Action("lsp::Hover".into()),
            "z" => match k(i + 1)? {
                "a" => {
                    let line = ed.buffer.line_of(ed.head);
                    ed.run(if ed.buffer.folds.iter().any(|(s, _)| *s == line) { Command::FoldExpand } else { Command::FoldCollapse });
                    VimOut::Handled
                }
                "c" => {
                    ed.run(Command::FoldCollapse);
                    VimOut::Handled
                }
                "o" => {
                    ed.run(Command::FoldExpand);
                    VimOut::Handled
                }
                "M" => {
                    ed.run(Command::FoldAll);
                    VimOut::Handled
                }
                "R" => {
                    ed.run(Command::UnfoldAll);
                    VimOut::Handled
                }
                // zz / zt / zb: the app recenters
                _ => VimOut::Action("vim::Center".into()),
            },
            "[" | "]" => match k(i + 1)? {
                "d" => VimOut::Action(if key == "]" { "lsp::NextError".into() } else { "lsp::PrevError".into() }),
                _ => VimOut::Handled,
            },
            "<C-o>" => VimOut::Action("workspace::NavBack".into()),
            "<C-i>" | "<Tab>" => VimOut::Action("workspace::NavForward".into()),
            "d" | "c" | "y" | ">" | "<" => return self.operator(ed, keys, i, key.chars().next()?, count, reg),
            "<Esc>" => VimOut::Handled,
            _ => VimOut::Handled,
        };
        Some(out)
    }

    /// Replays recorded keys (and inserted text) for `.`.
    fn replay(&mut self, ed: &mut Editor, keys: &[String]) {
        let saved = std::mem::take(&mut self.last_change);
        for k in keys {
            if let Some(t) = k.strip_prefix('\u{0}') {
                ed.insert(t);
            } else {
                let _ = self.key(ed, k);
            }
        }
        if self.mode == Mode::Insert {
            let _ = self.key(ed, "<Esc>");
        }
        self.last_change = saved;
        self.recording = None;
    }

    /// Motion at keys[i..]: None = not a motion; Some(None) = incomplete; Some(Some) = motion.
    #[allow(clippy::option_option)]
    fn parse_motion(&mut self, keys: &[String], i: usize, _count: usize, _has_count: bool) -> Option<Option<(Motion, usize)>> {
        let k = |j: usize| keys.get(j).map(String::as_str);
        let m = match k(i)? {
            "h" | "<Left>" | "<BS>" => Motion::Left,
            "l" | "<Right>" | " " => Motion::Right,
            "j" | "<Down>" | "<C-n>" => Motion::Down,
            "k" | "<Up>" | "<C-p>" => Motion::Up,
            "w" => Motion::WordFwd(false),
            "W" => Motion::WordFwd(true),
            "b" => Motion::WordBack(false),
            "B" => Motion::WordBack(true),
            "e" => Motion::WordEnd(false),
            "E" => Motion::WordEnd(true),
            "0" | "<Home>" => Motion::LineStart,
            "^" => Motion::FirstNonBlank,
            "$" | "<End>" => Motion::LineEnd,
            "G" => Motion::FileEnd,
            "}" => Motion::ParaFwd,
            "{" => Motion::ParaBack,
            "%" => Motion::MatchPair,
            "<C-d>" => Motion::HalfDown,
            "<C-u>" => Motion::HalfUp,
            "g" => match k(i + 1) {
                None => return Some(None),
                Some("g") => Motion::FileStart,
                Some("e") => return None,
                _ => return None,
            },
            "f" | "F" | "t" | "T" => {
                let key = k(i)?;
                let Some(c) = k(i + 1) else { return Some(None) };
                let ch = c.chars().next()?;
                let (fwd, till) = match key {
                    "f" => (true, false),
                    "F" => (false, false),
                    "t" => (true, true),
                    _ => (false, true),
                };
                self.last_find = Some((ch, fwd, till));
                Motion::Find(ch, fwd, till)
            }
            ";" | "," => {
                let (ch, fwd, till) = self.last_find?;
                Motion::Find(ch, if k(i)? == ";" { fwd } else { !fwd }, till)
            }
            _ => return None,
        };
        Some(Some((m, i)))
    }

    /// d/c/y/>/</g~ followed by a motion, a text object, or itself (dd).
    fn operator(&mut self, ed: &mut Editor, keys: &[String], i: usize, op: char, count: usize, reg: Option<char>) -> Option<VimOut> {
        let k = |j: usize| keys.get(j).map(String::as_str);
        let mut j = i + 1;
        // a second count: d3w
        let mut c2 = String::new();
        while let Some(c) = k(j) {
            if c.len() == 1 && c.chars().all(|ch| ch.is_ascii_digit()) && !(c == "0" && c2.is_empty()) {
                c2.push_str(c);
                j += 1;
            } else {
                break;
            }
        }
        let count = count * c2.parse::<usize>().unwrap_or(1);
        let next = k(j)?;
        if next == "<Esc>" {
            return Some(VimOut::Handled);
        }
        self.recording = Some(keys.to_vec());
        let this_key = keys[i].as_str();
        // doubled: dd, cc, yy, >>, <<, g~~, guu
        if next == this_key || (op != 'd' && op != 'c' && op != 'y' && op != '>' && op != '<' && next.chars().next() == Some(op)) {
            let line = ed.buffer.line_of(ed.head);
            let last = (line + count - 1).min(ed.buffer.line_count().saturating_sub(1));
            let r = Self::lines_range(ed, line, last);
            let at = ed.head;
            let out = self.apply_operator(ed, op, r, true, reg);
            if op == 'y' {
                ed.set_cursor(at, false);
            }
            self.finish_change();
            return Some(out);
        }
        // text objects: iw, a", i(, it…
        if next == "i" || next == "a" {
            let obj = k(j + 1)?.chars().next()?;
            let Some(r) = Self::text_object(ed, next == "i", obj) else {
                self.recording = None;
                return Some(VimOut::Handled);
            };
            let linewise = obj == 'p';
            let out = self.apply_operator(ed, op, r, linewise, reg);
            self.finish_change();
            return Some(out);
        }
        // cw behaves like ce (Vim's special case)
        let keys2: Vec<String> = if op == 'c' && (next == "w" || next == "W") { vec![if next == "w" { "e".into() } else { "E".into() }] } else { keys[j..].to_vec() };
        let res = self.parse_motion(&keys2, 0, count, false);
        let Some(Some((m, _))) = res else {
            if res.is_some() {
                return None; // incomplete (df, dg…)
            }
            self.recording = None;
            return Some(VimOut::Handled);
        };
        let start = ed.head;
        let Some(mut end) = self.motion_target(ed, m, count) else {
            self.recording = None;
            return Some(VimOut::Handled);
        };
        if m == Motion::FileEnd {
            end = ed.buffer.len();
        }
        let out = if m.linewise() {
            let (a, b) = (ed.buffer.line_of(start.min(end)), ed.buffer.line_of(start.max(end)));
            let r = Self::lines_range(ed, a, b);
            self.apply_operator(ed, op, r, true, reg)
        } else {
            let (mut a, mut b) = (start.min(end), start.max(end));
            if m.inclusive() || (op == 'c' && matches!(m, Motion::WordEnd(_))) {
                b = ed.buffer.next_grapheme(b).min(ed.buffer.line_end(ed.buffer.line_of(b)).max(b + 1).min(ed.buffer.len()));
            }
            // dw at the last word of a line stops at the line end
            if matches!(m, Motion::WordFwd(_)) && ed.buffer.line_of(b) > ed.buffer.line_of(a) {
                b = ed.buffer.line_end(ed.buffer.line_of(a));
            }
            if a > b {
                std::mem::swap(&mut a, &mut b);
            }
            self.apply_operator(ed, op, a..b, false, reg)
        };
        self.finish_change();
        Some(out)
    }

    fn update_visual(&mut self, ed: &mut Editor) {
        let head = ed.head;
        self.visual_caret = head;
        if self.mode != Mode::VisualBlock {
            ed.extra.clear();
        }
        match self.mode {
            Mode::VisualBlock => {
                // one selection per line, the caret's line is the primary one
                let b = &ed.buffer;
                let (ls, lc) = (b.line_of(self.visual_start), b.line_of(head));
                let (cs, cc) = (b.display_col(self.visual_start), b.display_col(head));
                let (c0, c1) = (cs.min(cc), cs.max(cc));
                let lines: Vec<usize> = if ls <= lc { (ls..=lc).collect() } else { (lc..=ls).rev().collect() };
                let mut ranges: Vec<Range<usize>> = lines
                    .into_iter()
                    .map(|l| {
                        let s = b.byte_at_display_col(l, c0);
                        let e = b.byte_at_display_col(l, c1);
                        let le = b.line_end(l);
                        let e = if e < le { b.next_grapheme(e).min(le) } else { le };
                        s..e.max(s)
                    })
                    .collect();
                let last = ranges.pop().unwrap_or(head..head);
                ed.extra = ranges;
                ed.anchor = last.start;
                ed.head = last.end;
            }
            Mode::VisualLine => {
                let (a, b) = (self.visual_start.min(head), self.visual_start.max(head));
                let (la, lb) = (ed.buffer.line_of(a), ed.buffer.line_of(b));
                if head >= self.visual_start {
                    ed.anchor = ed.buffer.line_start(la);
                    ed.head = ed.buffer.line_end(lb);
                } else {
                    ed.anchor = ed.buffer.line_end(lb);
                    ed.head = ed.buffer.line_start(la);
                }
            }
            _ => {
                // the character under the caret is part of the selection
                if head >= self.visual_start {
                    ed.anchor = self.visual_start;
                    ed.head = ed.buffer.next_grapheme(head).min(ed.buffer.len());
                } else {
                    ed.anchor = ed.buffer.next_grapheme(self.visual_start).min(ed.buffer.len());
                    ed.head = head;
                }
            }
        }
    }

    /// Visual selection as a range + whether it's line-wise.
    fn visual_range(&self, ed: &Editor) -> (Range<usize>, bool) {
        let s = ed.selection();
        if self.mode == Mode::VisualLine {
            let (la, lb) = (ed.buffer.line_of(s.start), ed.buffer.line_of(s.end));
            (Self::lines_range(ed, la, lb), true)
        } else {
            (s, false)
        }
    }

    /// Visual Block: d x c s I A y o r > < on the per-line selections.
    fn block_key(&mut self, ed: &mut Editor, keys: &[String], i: usize, reg: Option<char>) -> Option<VimOut> {
        let key = keys[i].as_str();
        let mut all: Vec<Range<usize>> = ed.extra.clone();
        all.push(ed.anchor..ed.head);
        all.sort_by_key(|r| r.start);
        let top_left = all.first().map_or(ed.head, |r| r.start);
        let block_text = || all.iter().map(|r| ed.buffer.slice(r.clone())).collect::<Vec<_>>().join("\n");
        let out = match key {
            "<Esc>" | "<C-c>" | "<C-v>" | "v" | "V" if key != "v" && key != "V" => {
                ed.extra.clear();
                self.mode = Mode::Normal;
                ed.set_cursor(top_left, false);
                VimOut::Handled
            }
            "v" | "V" => {
                ed.extra.clear();
                self.mode = if key == "v" { Mode::Visual } else { Mode::VisualLine };
                ed.head = self.visual_caret;
                self.update_visual(ed);
                VimOut::Handled
            }
            "d" | "x" | "c" | "s" => {
                let text = block_text();
                self.registers.insert(reg.unwrap_or('"'), Register { text: text.clone(), linewise: false });
                ed.insert_all("");
                if matches!(key, "c" | "s") {
                    // one caret per line: typing goes to every line
                    self.mode = Mode::Insert;
                } else {
                    ed.extra.clear();
                    self.mode = Mode::Normal;
                    ed.set_cursor(top_left.min(ed.buffer.len()), false);
                }
                return Some(if self.use_clipboard { VimOut::Effect(Effect::CopyToClipboard(text)) } else { VimOut::Handled });
            }
            "I" | "A" => {
                let at: Vec<usize> = all.iter().map(|r| if key == "I" { r.start } else { r.end }).collect();
                let (last, rest) = at.split_last()?;
                ed.extra = rest.iter().map(|p| *p..*p).collect();
                ed.anchor = *last;
                ed.head = *last;
                self.mode = Mode::Insert;
                VimOut::Handled
            }
            "y" => {
                let text = block_text();
                self.registers.insert(reg.unwrap_or('"'), Register { text: text.clone(), linewise: false });
                ed.extra.clear();
                self.mode = Mode::Normal;
                ed.set_cursor(top_left, false);
                return Some(if self.use_clipboard { VimOut::Effect(Effect::CopyToClipboard(text)) } else { VimOut::Handled });
            }
            "r" => {
                let c = keys.get(i + 1)?.clone();
                ed.edit_all(|b, a, h| {
                    let r = a.min(h)..a.max(h);
                    let n = b.slice(r.clone()).chars().count();
                    (r, c.repeat(n))
                });
                ed.extra.clear();
                self.mode = Mode::Normal;
                ed.set_cursor(top_left, false);
                VimOut::Handled
            }
            "o" => {
                let caret = self.visual_caret;
                ed.head = self.visual_start;
                self.visual_start = caret;
                self.update_visual(ed);
                VimOut::Handled
            }
            ">" | "<" => {
                let (f, l) = (ed.buffer.line_of(all.first()?.start), ed.buffer.line_of(all.last()?.start));
                ed.extra.clear();
                self.mode = Mode::Normal;
                let r = ed.buffer.line_start(f)..ed.buffer.line_end(l);
                self.apply_operator(ed, key.chars().next()?, r, true, None)
            }
            _ => VimOut::Handled,
        };
        Some(out)
    }

    fn visual_key(&mut self, ed: &mut Editor, keys: &[String], i: usize, reg: Option<char>) -> Option<VimOut> {
        if self.mode == Mode::VisualBlock {
            return self.block_key(ed, keys, i, reg);
        }
        let key = keys[i].as_str();
        // the caret while selecting: undo the "+1 char" of update_visual for motions
        let (r, linewise) = self.visual_range(ed);
        let leave = |s: &mut Self, ed: &mut Editor, at: usize| {
            s.mode = Mode::Normal;
            ed.set_cursor(at.min(ed.buffer.len()), false);
        };
        Some(match key {
            "<C-v>" => {
                self.mode = Mode::VisualBlock;
                ed.head = self.visual_caret;
                self.update_visual(ed);
                VimOut::Handled
            }
            "<Esc>" | "<C-c>" => {
                let at = if ed.head > ed.anchor { ed.buffer.prev_grapheme(ed.head) } else { ed.head };
                leave(self, ed, at);
                VimOut::Handled
            }
            "v" | "V" => {
                let want = if key == "v" { Mode::Visual } else { Mode::VisualLine };
                if self.mode == want {
                    let at = ed.head.min(ed.anchor);
                    leave(self, ed, at);
                } else {
                    // keep the logical caret
                    let caret = if ed.head > ed.anchor { ed.buffer.prev_grapheme(ed.head) } else { ed.head };
                    self.mode = want;
                    ed.head = caret;
                    self.update_visual(ed);
                }
                VimOut::Handled
            }
            "o" => {
                let caret = if ed.head > ed.anchor { ed.buffer.prev_grapheme(ed.head) } else { ed.head };
                let other = self.visual_start;
                self.visual_start = caret;
                ed.head = other;
                self.update_visual(ed);
                VimOut::Handled
            }
            "d" | "x" | "X" | "D" | "c" | "s" | "y" | "Y" | ">" | "<" | "~" | "u" | "U" => {
                let op = match key {
                    "x" | "X" | "D" => 'd',
                    "s" => 'c',
                    "Y" => 'y',
                    k => k.chars().next().unwrap(),
                };
                let lw = linewise || matches!(key, "X" | "D" | "Y");
                let r = if lw && !linewise { Self::lines_range(ed, ed.buffer.line_of(r.start), ed.buffer.line_of(r.end.saturating_sub(1).max(r.start))) } else { r };
                self.mode = Mode::Normal;
                let start = r.start;
                let out = self.apply_operator(ed, op, r, lw, reg);
                if op == 'c' {
                    self.mode = Mode::Insert;
                } else if op == 'y' {
                    ed.set_cursor(start, false);
                }
                out
            }
            "p" | "P" => {
                // replace the selection with the register
                let Some(regv) = self.registers.get(&reg.unwrap_or('"')).cloned() else { return Some(VimOut::Effect(Effect::RequestPaste)) };
                ed.buffer.break_group();
                ed.anchor = r.start;
                ed.head = r.end;
                ed.insert(&regv.text);
                ed.buffer.break_group();
                self.mode = Mode::Normal;
                VimOut::Handled
            }
            "J" => {
                ed.run(Command::JoinLines);
                self.mode = Mode::Normal;
                VimOut::Handled
            }
            // text objects select: viw, vi(, vat…
            "i" | "a" => {
                let obj = keys.get(i + 1)?.chars().next()?;
                ed.head = self.visual_caret;
                if let Some(r) = Self::text_object(ed, key == "i", obj).filter(|r| !r.is_empty()) {
                    self.visual_start = r.start;
                    ed.head = ed.buffer.prev_grapheme(r.end).max(r.start);
                }
                self.update_visual(ed);
                VimOut::Handled
            }
            "g" => match keys.get(i + 1)?.as_str() {
                "c" => {
                    let (f, l) = (ed.buffer.line_of(r.start), ed.buffer.line_of(r.end.saturating_sub(1).max(r.start)));
                    ed.anchor = ed.buffer.line_start(f);
                    ed.head = ed.buffer.line_end(l);
                    ed.run(Command::CommentLine);
                    let at = ed.buffer.line_start(f);
                    leave(self, ed, at);
                    VimOut::Handled
                }
                _ => VimOut::Handled,
            },
            ":" => VimOut::CommandLine,
            _ => VimOut::Handled,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Buffer;

    fn run(text: &str, at: usize, keys: &str) -> (String, usize, Mode) {
        let mut ed = Editor::new(Buffer::from_text(text));
        ed.set_cursor(at, false);
        let mut v = Vim::new();
        feed(&mut v, &mut ed, keys);
        (ed.buffer.text(), ed.head, v.mode)
    }

    /// Keys in Vim notation; plain characters one by one; in Insert mode text is inserted.
    fn feed(v: &mut Vim, ed: &mut Editor, keys: &str) {
        let mut chars = keys.chars().peekable();
        while let Some(c) = chars.next() {
            let key = if c == '<' && chars.peek().is_some_and(|n| n.is_ascii_uppercase()) {
                let mut k = String::from("<");
                for n in chars.by_ref() {
                    k.push(n);
                    if n == '>' {
                        break;
                    }
                }
                k
            } else {
                c.to_string()
            };
            if v.key(ed, &key) == VimOut::Pass {
                // like the app: typed text goes to every caret
                ed.insert_all(&key);
                v.inserted(&key);
            }
        }
    }

    #[test]
    fn motions() {
        let t = "foo bar.baz  qux\nsecond line\n";
        assert_eq!(run(t, 0, "w").1, 4);
        assert_eq!(run(t, 0, "ww").1, 7); // "." is its own word
        assert_eq!(run(t, 0, "W").1, 4);
        assert_eq!(run(t, 0, "WW").1, 13);
        assert_eq!(run(t, 0, "e").1, 2);
        assert_eq!(run(t, 8, "b").1, 7);
        assert_eq!(run(t, 0, "$").1, 15);
        assert_eq!(run(t, 5, "0").1, 0);
        assert_eq!(run(t, 0, "j").1, 17);
        assert_eq!(run(t, 0, "fz").1, 10);
        assert_eq!(run(t, 0, "tz").1, 9);
        assert_eq!(run(t, 0, "fa;").1, 9);
        assert_eq!(run(t, 20, "gg").1, 0);
        assert_eq!(run(t, 0, "G").1, 29);
        assert_eq!(run("f(a(b)c)", 1, "%").1, 7);
        assert_eq!(run(t, 0, "3l").1, 3);
    }

    #[test]
    fn operators_and_objects() {
        assert_eq!(run("foo bar baz", 0, "dw").0, "bar baz");
        assert_eq!(run("foo bar baz", 0, "d2w").0, "baz");
        assert_eq!(run("foo bar baz", 4, "cwX<Esc>").0, "foo X baz");
        assert_eq!(run("foo bar baz", 4, "ciwX<Esc>").0, "foo X baz");
        assert_eq!(run("foo bar baz", 4, "daw").0, "foo baz");
        assert_eq!(run("a\nb\nc\n", 2, "dd").0, "a\nc\n");
        assert_eq!(run("a\nb\nc\n", 0, "2dd").0, "c\n");
        assert_eq!(run("a\nb\nc", 4, "dd").0, "a\nb");
        assert_eq!(run("a\nb\nc\n", 0, "dj").0, "c\n");
        assert_eq!(run("say(\"hi there\")", 6, "di\"").0, "say(\"\")");
        assert_eq!(run("say(\"hi there\")", 6, "da(").0, "say");
        assert_eq!(run("f(a, g(b))", 8, "ci(x<Esc>").0, "f(a, g(x))");
        assert_eq!(run("<p><b>x</b> y</p>", 7, "dit").0, "<p><b></b> y</p>");
        assert_eq!(run("hello world", 0, "D").0, "");
        assert_eq!(run("hello world", 6, "C!<Esc>").0, "hello !");
        assert_eq!(run("abc", 0, "x").0, "bc");
        assert_eq!(run("abc", 0, "rX").0, "Xbc");
        assert_eq!(run("abc", 0, "~").0, "Abc");
        assert_eq!(run("hello world", 0, "de").0, " world");
        assert_eq!(run("hello world", 0, "d$").0, "");
        assert_eq!(run("hello", 0, "dfl").0, "lo");
        assert_eq!(run("x\n", 0, ">>").0, "    x\n");
    }

    #[test]
    fn insert_modes_undo_repeat_registers() {
        assert_eq!(run("abc", 0, "iX<Esc>").0, "Xabc");
        assert_eq!(run("abc", 0, "aX<Esc>").0, "aXbc");
        assert_eq!(run("  abc", 4, "IX<Esc>").0, "  Xabc");
        assert_eq!(run("abc", 0, "AX<Esc>").0, "abcX");
        assert_eq!(run("  a\nb", 0, "ox<Esc>").0, "  a\n  x\nb");
        assert_eq!(run("a\nb", 2, "Ox<Esc>").0, "a\nx\nb");
        let (t, _, m) = run("abc", 0, "iX<Esc>");
        assert_eq!((t.as_str(), m), ("Xabc", Mode::Normal));
        // undo / redo
        assert_eq!(run("foo bar", 0, "dwu").0, "foo bar");
        assert_eq!(run("foo bar", 0, "dwu<C-r>").0, "bar");
        // . repeats the last change, including inserted text
        assert_eq!(run("a b c d", 0, "dw.").0, "c d");
        assert_eq!(run("x\ny\n", 0, "AZ<Esc>j.").0, "xZ\nyZ\n");
        assert_eq!(run("foo foo", 0, "cwbar<Esc>w.").0, "bar bar");
        // yank / put
        assert_eq!(run("a\nb\n", 0, "yyp").0, "a\na\nb\n");
        assert_eq!(run("a\nb\n", 2, "yyP").0, "a\nb\nb\n");
        assert_eq!(run("ab", 0, "ylp").0, "aab");
        assert_eq!(run("foo bar", 0, "dwP").0, "foo bar");
        // named register
        assert_eq!(run("a\nb\n", 0, "\"ayyj\"ap").0, "a\nb\na\n");
    }

    #[test]
    fn visual_modes() {
        assert_eq!(run("hello world", 0, "vld").0, "llo world");
        assert_eq!(run("hello world", 0, "vex<Esc>").0, " world");
        assert_eq!(run("a\nb\nc\n", 0, "Vjd").0, "c\n");
        assert_eq!(run("hello", 0, "vlly$p").0, "hellohel");
        assert_eq!(run("ab cd", 0, "veU").0, "AB cd");
        let (_, _, m) = run("ab", 0, "v<Esc>");
        assert_eq!(m, Mode::Normal);
    }

    #[test]
    fn block_objects_gc_macros() {
        // Visual Block: delete a column, insert on every line
        assert_eq!(run("abc\nabc\nabc\n", 0, "<C-v>jjd").0, "bc\nbc\nbc\n");
        assert_eq!(run("abc\nabc\n", 1, "<C-v>jIX<Esc>").0, "aXbc\naXbc\n");
        assert_eq!(run("ab\nab\n", 0, "<C-v>jAX<Esc>").0, "aXb\naXb\n");
        assert_eq!(run("abc\nabc\n", 0, "<C-v>jlrZ").0, "ZZc\nZZc\n");
        // text objects in Visual
        assert_eq!(run("f(a, b)", 3, "vi(d").0, "f()");
        assert_eq!(run("foo bar", 5, "viwd").0, "foo ");
        // gc with a motion / object / in Visual
        assert_eq!(run("a\nb\nc\n", 0, "gcj").0, "// a\n// b\nc\n");
        assert_eq!(run("a\nb\n\nc\n", 0, "gcip").0, "// a\n// b\n\nc\n");
        assert_eq!(run("a\nb\n", 0, "Vjgc").0, "// a\n// b\n");
        // macros: record "A;<Esc>j", replay with @q and @@
        let mut ed = Editor::new(Buffer::from_text("x\ny\nz\n"));
        let mut v = Vim::new();
        for k in ["q", "q", "A"] {
            v.feed(&mut ed, k);
        }
        ed.insert(";");
        v.inserted(";");
        for k in ["<Esc>", "j", "q", "@", "q", "@", "@"] {
            v.feed(&mut ed, k);
        }
        assert_eq!(ed.buffer.text(), "x;\ny;\nz;\n");
    }

    #[test]
    fn vimrc_mappings() {
        let cfg = parse_vimrc("\" comment\nlet mapleader = \" \"\nnnoremap <leader>f <Action>(workspace::SearchEverywhere)\ninoremap jk <Esc>\nnmap <leader>r :action RenameElement<CR>\nnnoremap H ^\nset clipboard=\n");
        assert_eq!(cfg.leader, " ");
        assert_eq!(cfg.maps.len(), 4);
        assert_eq!(cfg.clipboard, Some(false));
        let mut ed = Editor::new(Buffer::from_text("  abc"));
        ed.set_cursor(4, false);
        let mut v = Vim::new();
        v.configure(&cfg);
        assert_eq!(v.feed(&mut ed, " "), [VimOut::Handled]);
        assert_eq!(v.feed(&mut ed, "f"), [VimOut::Action("workspace::SearchEverywhere".into())]);
        v.feed(&mut ed, "H");
        assert_eq!(ed.head, 2);
        v.feed(&mut ed, "i");
        assert!(v.wants_key("j"));
        assert_eq!(v.feed(&mut ed, "j"), [VimOut::Handled]);
        v.feed(&mut ed, "k");
        assert_eq!(v.mode, Mode::Normal);
        // "j" then something else: the j is typed after all
        v.feed(&mut ed, "i");
        v.feed(&mut ed, "j");
        assert_eq!(v.feed(&mut ed, "x"), [VimOut::Insert("j".into()), VimOut::Insert("x".into())]);
        assert_eq!(split_keys("<C-R>x<leader>"), ["<C-r>", "x", "<leader>"]);
    }

    #[test]
    fn ide_keys() {
        let mut ed = Editor::new(Buffer::from_text("x"));
        let mut v = Vim::new();
        assert_eq!(v.key(&mut ed, "g"), VimOut::Handled);
        assert_eq!(v.key(&mut ed, "d"), VimOut::Action("lsp::GotoDefinition".into()));
        assert_eq!(v.key(&mut ed, ":"), VimOut::CommandLine);
        assert_eq!(v.key(&mut ed, "/"), VimOut::Search(true));
    }
}
