//! Vim keymap in the editor (Settings → Kiểu phím: Vim), like IdeaVim: keys reach the Vim
//! engine (editor_core::vim) before the IDE's shortcuts, except the Ctrl keys the user left to
//! the IDE (Settings), function keys and Alt combinations. Normal/Visual modes register no text
//! input handler, so fcitx5 (Vietnamese) never composes Vim commands; Insert mode types normally.
use crate::editor_view::EditorView;
use crate::input::TextInput;
use editor_core::vim::{Mode, Vim, VimConfig, VimOut};
use editor_core::Effect;
use gpui::{App, AppContext, Context, Focusable, Keystroke, Window};
use std::path::PathBuf;

/// Ctrl keys Vim and the IDE both want; each goes to one of them (Settings).
pub const CTRL_KEYS: &[(&str, &str)] = &[
    ("ctrl-r", "redo / IDE: Replace"),
    ("ctrl-d", "½ trang xuống / IDE: nhân dòng"),
    ("ctrl-u", "½ trang lên"),
    ("ctrl-o", "nhảy lùi"),
    ("ctrl-i", "nhảy tới"),
    ("ctrl-w", "Insert: xóa từ / IDE: đóng tab"),
    ("ctrl-v", "Visual Block / IDE: dán"),
    ("ctrl-n", "xuống / IDE: tìm class"),
    ("ctrl-p", "lên / IDE: parameter info"),
    ("ctrl-[", "= Esc"),
];

pub fn default_vim_keys() -> Vec<String> {
    ["ctrl-r", "ctrl-d", "ctrl-u", "ctrl-o", "ctrl-i", "ctrl-w", "ctrl-v", "ctrl-["].iter().map(|s| s.to_string()).collect()
}

pub fn vimrc_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).unwrap_or_default()).join(".config"));
    base.join("chodilamide/vimrc")
}

pub const VIMRC_TEMPLATE: &str = r#"" ChoDiLamIDE vimrc (giống .ideavimrc): lưu file là áp dụng ngay.
" Map tới action của IDE: <Action>(tên) - tên xem trong Find Action (Ctrl+Shift+A), vd workspace::GoToFile.
let mapleader = " "
set clipboard=unnamedplus

inoremap jk <Esc>
nnoremap <leader>f <Action>(workspace::SearchEverywhere)
nnoremap <leader>e <Action>(workspace::RecentFiles)
nnoremap <leader>r <Action>(lsp::Rename)
nnoremap <leader>a <Action>(lsp::ShowCodeActions)
nnoremap <leader>g <Action>(workspace::GitCommit)
nnoremap <leader>t <Action>(workspace::ToggleTerminal)
"#;

pub fn load_config() -> VimConfig {
    std::fs::read_to_string(vimrc_path()).map(|s| editor_core::vim::parse_vimrc(&s)).unwrap_or_default()
}

/// A gpui keystroke in Vim notation: "a", "A", "$", "<Esc>", "<CR>", "<C-r>".
pub fn vim_key(k: &Keystroke) -> Option<String> {
    let m = &k.modifiers;
    let name = k.key.as_str();
    if matches!(name, "shift" | "control" | "alt" | "platform" | "function" | "capslock") {
        return None;
    }
    if m.control {
        let c = if name == "[" { "[".to_string() } else { name.to_string() };
        return Some(format!("<C-{c}>"));
    }
    Some(match name {
        "escape" => "<Esc>".into(),
        "enter" => "<CR>".into(),
        "backspace" => "<BS>".into(),
        "tab" => "<Tab>".into(),
        "space" => " ".into(),
        "left" => "<Left>".into(),
        "right" => "<Right>".into(),
        "up" => "<Up>".into(),
        "down" => "<Down>".into(),
        "home" => "<Home>".into(),
        "end" => "<End>".into(),
        "delete" => "x".into(),
        _ => match &k.key_char {
            Some(c) if !c.is_empty() => c.clone(),
            _ if m.shift && name.chars().count() == 1 => name.to_uppercase(),
            _ => name.to_string(),
        },
    })
}

/// The editor Vim keys go to (the focused one), set while painting.
#[derive(Default)]
struct VimFocus(Option<gpui::WeakEntity<EditorView>>);
impl gpui::Global for VimFocus {}

pub fn set_focus(view: gpui::WeakEntity<EditorView>, cx: &mut App) {
    cx.set_global(VimFocus(Some(view)));
}

/// Set by the workspace while a git / database window covers the editors.
#[derive(Default)]
pub struct OverlayOpen(pub bool);
impl gpui::Global for OverlayOpen {}

/// Installed once: keystrokes in a Vim editor go to Vim before any IDE binding.
pub fn install(cx: &mut App) {
    cx.intercept_keystrokes(|ev: &gpui::KeystrokeEvent, window, cx| {
        // only when the editor itself has focus (not its find bar, rename box, command line)
        // a git / database window is on top: its keys (Esc) are not for the editor below
        if cx.try_global::<OverlayOpen>().is_some_and(|o| o.0) {
            return;
        }
        let Some(ctx) = ev.context_stack.last() else { return };
        if !(ctx.contains("Editor") && ctx.contains("vim")) {
            return;
        }
        let Some(view) = cx.default_global::<VimFocus>().0.as_ref().and_then(|w| w.upgrade()) else { return };
        let Some(key) = vim_key(&ev.keystroke) else { return };
        let ks = ev.keystroke.unparse();
        let m = ev.keystroke.modifiers;
        let route = view.read_with(cx, |v, _| {
            let Some(vim) = v.vim.as_ref() else { return false };
            if m.alt || m.platform || ev.keystroke.key.starts_with('f') && ev.keystroke.key.len() > 1 && ev.keystroke.key[1..].chars().all(|c| c.is_ascii_digit()) {
                return false;
            }
            if m.control {
                return v.vim_keys.iter().any(|k| *k == ks) && (vim.mode != Mode::Insert || matches!(key.as_str(), "<C-w>" | "<C-u>" | "<C-[>" | "<C-o>"));
            }
            match vim.mode {
                Mode::Insert => key == "<Esc>" || (key.chars().count() == 1 && vim.wants_key(&key)),
                _ => true,
            }
        });
        if route {
            cx.stop_propagation();
            view.update(cx, |v, cx| v.vim_input(&key, window, cx));
        }
    })
    .detach();
}

impl EditorView {
    pub fn set_vim(&mut self, on: bool, keys: Vec<String>, cfg: &VimConfig) {
        self.vim_keys = keys;
        match (on, self.vim.is_some()) {
            (true, false) => {
                let mut v = Vim::new();
                v.configure(cfg);
                v.clamp(&mut self.editor);
                self.vim = Some(v);
            }
            (true, true) => {
                if let Some(v) = self.vim.as_mut() {
                    v.configure(cfg);
                }
            }
            (false, _) => self.vim = None,
        }
    }

    /// "-- NORMAL --" + pending keys, for the status bar.
    pub fn vim_status(&self) -> Option<String> {
        let v = self.vim.as_ref()?;
        let p = v.pending();
        Some(if p.is_empty() { format!("-- {} --", v.mode_label()) } else { format!("-- {} -- {p}", v.mode_label()) })
    }

    pub fn vim_block_cursor(&self) -> bool {
        self.vim.as_ref().is_some_and(|v| v.mode != Mode::Insert)
    }

    /// Typed text in Insert mode, recorded for `.`.
    pub(crate) fn vim_inserted(&mut self, text: &str) {
        if let Some(v) = self.vim.as_mut().filter(|v| v.mode == Mode::Insert) {
            v.inserted(text);
        }
    }

    pub(crate) fn vim_input(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut vim) = self.vim.take() else { return };
        self.completion = None;
        self.hover = None;
        let outs = vim.feed(&mut self.editor, key);
        self.vim = Some(vim);
        for o in outs {
            self.vim_out(o, window, cx);
        }
        self.scroll_to_cursor_pub();
        cx.notify();
    }

    fn vim_out(&mut self, o: VimOut, window: &mut Window, cx: &mut Context<Self>) {
        match o {
            VimOut::Handled | VimOut::Pass => {}
            VimOut::Insert(t) => {
                self.editor.insert_all(&t);
                // its keys are already in a macro being recorded: only `.` needs the text
                if let Some(v) = self.vim.as_mut() {
                    v.inserted_for_repeat(&t);
                }
                self.after_typed(&t, cx);
            }
            VimOut::Effect(Effect::CopyToClipboard(t)) => cx.write_to_clipboard(gpui::ClipboardItem::new_string(t)),
            VimOut::Effect(Effect::RequestPaste) => {
                // empty register: the clipboard is the register (clipboard=unnamedplus)
                if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) {
                    let lw = text.ends_with('\n');
                    if let Some(v) = self.vim.as_mut() {
                        v.set_register('"', text, lw);
                    }
                    self.vim_input("p", window, cx);
                }
            }
            VimOut::Effect(Effect::RequestSave) => self.save_formatted(cx),
            VimOut::Effect(_) => {}
            VimOut::Action(name) if name == "vim::Center" => self.request_center(),
            VimOut::Action(name) => match cx.build_action(&name, None) {
                Ok(action) => window.defer(cx, move |window, cx| window.dispatch_action(action, cx)),
                Err(e) => self.message = Some(format!("vimrc: action {name}: {e}")),
            },
            VimOut::CommandLine => self.open_vim_cmdline(window, cx),
            VimOut::Search(fwd) => {
                self.vim_search_back = !fwd;
                self.open_find_pub(window, cx);
            }
            VimOut::SearchNext(fwd) => {
                // * / #: the word under the caret became the search
                if let Some(w) = self.vim.as_ref().and_then(|v| v.register('/')).map(|r| r.text.clone()) {
                    if self.last_find.as_deref() != Some(&w) {
                        self.last_find = Some(w);
                    }
                    if let Some(v) = self.vim.as_mut() {
                        v.set_register('/', String::new(), false);
                    }
                }
                let back = fwd == self.vim_search_back;
                self.vim_search(!back);
            }
        }
    }

    /// n / N: next / previous match of the last search, the caret on its start.
    fn vim_search(&mut self, forward: bool) {
        let Some(q) = self.last_find.clone().filter(|q| !q.is_empty()) else { return };
        let text = self.editor.buffer.text();
        let (hay, needle) = (text.to_lowercase(), q.to_lowercase());
        let at = self.editor.head;
        let hit = if forward {
            hay[(at + 1).min(hay.len())..].find(&needle).map(|i| at + 1 + i).or_else(|| hay.find(&needle))
        } else {
            hay[..at.min(hay.len())].rfind(&needle).or_else(|| hay.rfind(&needle))
        };
        match hit {
            Some(i) => self.editor.set_cursor(i, false),
            None => self.message = Some(format!("Không tìm thấy \"{q}\"")),
        }
    }

    // ---- ':' command line ------------------------------------------------------------

    fn open_vim_cmdline(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| TextInput::new(":", cx));
        window.focus(&input.read(cx).focus_handle(cx));
        self.vim_cmdline = Some(input);
        cx.notify();
    }

    pub(crate) fn close_vim_cmdline(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.vim_cmdline = None;
        window.focus(&self.focus_handle(cx));
        cx.notify();
    }

    pub(crate) fn run_vim_cmdline(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.vim_cmdline.take() else { return };
        let cmd = input.read(cx).content.trim().trim_start_matches(':').to_string();
        window.focus(&self.focus_handle(cx));
        self.run_ex(&cmd, window, cx);
        cx.notify();
    }

    /// :w :q :wq :x :qa :e file :N :s/a/b/g :%s/a/b/g :noh :bn :bp
    pub(crate) fn run_ex(&mut self, cmd: &str, window: &mut Window, cx: &mut Context<Self>) {
        let dispatch = |name: &str, window: &mut Window, cx: &mut Context<Self>| {
            if let Ok(a) = cx.build_action(name, None) {
                window.defer(cx, move |window, cx| window.dispatch_action(a, cx));
            }
        };
        match cmd {
            "w" | "w!" => {
                self.save();
                self.lsp_did_save();
            }
            "q" | "q!" | "bd" => dispatch("workspace::CloseTab", window, cx),
            "wq" | "x" => {
                self.save();
                self.lsp_did_save();
                dispatch("workspace::CloseTab", window, cx);
            }
            "wa" => dispatch("shell::SaveAll", window, cx),
            "qa" | "qa!" | "wqa" => dispatch("shell::QuitApp", window, cx),
            "bn" | "bnext" => dispatch("workspace::NextTab", window, cx),
            "bp" | "bprev" => dispatch("workspace::PrevTab", window, cx),
            "noh" | "nohlsearch" => self.last_find = None,
            c if c.chars().all(|ch| ch.is_ascii_digit()) && !c.is_empty() => {
                let n: usize = c.parse().unwrap_or(1);
                self.goto(n.saturating_sub(1), 0);
            }
            c if c.starts_with("e ") || c.starts_with("edit ") => {
                let path = c.split_once(' ').map(|(_, p)| p.trim()).unwrap_or("");
                cx.emit(crate::editor_view::EditorEvent::OpenPath(path.to_string()));
            }
            c if c.starts_with("s/") || c.starts_with("%s/") => {
                let whole = c.starts_with('%');
                let body = &c[if whole { 3 } else { 2 }..];
                let parts: Vec<&str> = body.split('/').collect();
                if parts.len() < 2 {
                    self.message = Some("Dạng: :s/cũ/mới/g".into());
                    return;
                }
                let (from, to, global) = (parts[0], parts[1], parts.get(2).is_some_and(|f| f.contains('g')));
                let b = &self.editor.buffer;
                let range = if whole {
                    0..b.len()
                } else {
                    let l = b.line_of(self.editor.head);
                    b.line_start(l)..b.line_end(l)
                };
                let old = b.slice(range.clone());
                let new: String = if whole {
                    old.lines().map(|l| if global { l.replace(from, to) } else { l.replacen(from, to, 1) }).collect::<Vec<_>>().join("\n") + if old.ends_with('\n') { "\n" } else { "" }
                } else if global {
                    old.replace(from, to)
                } else {
                    old.replacen(from, to, 1)
                };
                let n = old.matches(from).count();
                if new != old {
                    let at = self.editor.head;
                    self.editor.buffer.break_group();
                    self.editor.anchor = range.start;
                    self.editor.head = range.end;
                    self.editor.insert(&new);
                    self.editor.buffer.break_group();
                    self.editor.set_cursor(at.min(self.editor.buffer.len()), false);
                }
                self.message = Some(format!("{n} chỗ khớp"));
            }
            "" => {}
            other => self.message = Some(format!("Lệnh không hỗ trợ: :{other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keystrokes_to_vim_notation() {
        let k = |s: &str| vim_key(&Keystroke::parse(s).unwrap());
        assert_eq!(k("escape").as_deref(), Some("<Esc>"));
        assert_eq!(k("ctrl-r").as_deref(), Some("<C-r>"));
        assert_eq!(k("enter").as_deref(), Some("<CR>"));
        assert_eq!(k("space").as_deref(), Some(" "));
        assert!(editor_core::vim::parse_vimrc(VIMRC_TEMPLATE).maps.len() >= 6);
    }
}
