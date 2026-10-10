//! Modal picker: "Go to File" (Ctrl+Shift+N, fuzzy) and "Find in Files" (Ctrl+Shift+F).
//! Searching runs on the background executor; stale results are dropped by generation.
use crate::input::{InputEvent, TextInput};
use crate::search::{self, Hit};
use crate::theme::{self, c};
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyBinding, MouseButton, ScrollStrategy,
    UniformListScrollHandle, Window, actions, div, prelude::*, px, uniform_list,
};
use std::path::PathBuf;
use std::sync::Arc;

actions!(picker, [SelectNext, SelectPrev, Confirm, Dismiss, ChangeShortcut]);

pub fn bind_keys(cx: &mut App) {
    let ctx = Some("Picker");
    cx.bind_keys([
        KeyBinding::new("down", SelectNext, ctx),
        KeyBinding::new("up", SelectPrev, ctx),
        KeyBinding::new("enter", Confirm, ctx),
        KeyBinding::new("escape", Dismiss, ctx),
        KeyBinding::new("ctrl-enter", ChangeShortcut, ctx),
    ]);
}

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    Files,
    Text,
    /// A fixed list (usages, problems) filtered locally by the query.
    List,
    /// Ask the language server (workspace/symbol) on every query change.
    Symbols,
    /// Find Action: every IDE action with its shortcut; Ctrl+Enter changes the shortcut.
    Actions,
    /// Search Everywhere (Shift Shift): classes/symbols (jdtls), files and actions together.
    Everywhere,
}

pub enum PickerEvent {
    Open { path: PathBuf, line: Option<usize>, col: usize },
    Dismissed,
    /// run this GPUI action (name) on the element that had focus before the picker
    RunAction(String),
    /// new shortcuts for an action ([] = none)
    Rebind { name: String, keys: Vec<String> },
}

#[derive(Clone)]
enum Item {
    File(String),
    Hit(Hit),
    Action { name: &'static str, title: &'static str, group: &'static str, keys: Vec<String> },
}

/// The picker waiting for a new shortcut (Find Action → Ctrl+Enter).
#[derive(Default)]
struct Capturing(Option<gpui::WeakEntity<Picker>>);
impl gpui::Global for Capturing {}

/// Keystrokes go to a capturing picker before any binding can run (so Ctrl+S, Enter… can be
/// recorded as shortcuts). Installed once at startup.
pub fn install_capture(cx: &mut App) {
    cx.intercept_keystrokes(|ev: &gpui::KeystrokeEvent, _, cx| {
        if !ev.context_stack.iter().any(|c| c.contains("KeyCapture")) {
            return;
        }
        let Some(p) = cx.default_global::<Capturing>().0.as_ref().and_then(|w| w.upgrade()) else { return };
        cx.stop_propagation();
        p.update(cx, |p, cx| p.captured(&ev.keystroke, cx));
    })
    .detach();
}

const MODIFIER_KEYS: &[&str] = &["shift", "control", "alt", "platform", "function", "capslock", "super", "ctrl", "cmd"];

pub struct Picker {
    mode: Mode,
    root: PathBuf,
    input: Entity<TextInput>,
    files: Arc<Vec<String>>,
    items: Vec<Item>,
    selected: usize,
    generation: usize,
    busy: bool,
    scroll: UniformListScrollHandle,
    client: Option<Arc<lsp_client::Client>>,
    title: String,
    all_hits: Arc<Vec<Hit>>,
    /// Find Action: the action whose shortcut is being recorded
    capturing: Option<&'static str>,
}

impl EventEmitter<PickerEvent> for Picker {}

const MAX_FILES: usize = 200;
const MAX_HITS: usize = 500;

impl Picker {
    pub fn new(mode: Mode, root: PathBuf, initial: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let placeholder = match mode {
            Mode::Files => "Tên file (gõ tắt được: pwa → ProductWebApplication)…",
            Mode::Text => "Tìm chữ trong project…",
            Mode::List => "Lọc…",
            Mode::Symbols => "Tên class / method (gõ tắt được: PWA, getNa)…",
            Mode::Actions => "Tên action (vd: run, format, breakpoint, maven)…",
            Mode::Everywhere => "Class, file, symbol hay action…",
        };
        let input = cx.new(|cx| TextInput::new(placeholder, cx));
        cx.subscribe(&input, |this, _, ev: &InputEvent, cx| {
            let InputEvent::Changed(q) = ev;
            this.refresh(q.clone(), cx);
        })
        .detach();
        crate::dialogs::focus_later(input.read(cx).focus_handle(cx), window, cx);
        let mut p = Self {
            mode,
            root: root.clone(),
            input: input.clone(),
            files: Arc::new(Vec::new()),
            items: Vec::new(),
            selected: 0,
            generation: 0,
            busy: true,
            scroll: UniformListScrollHandle::new(),
            title: match mode {
                Mode::Files => "Mở file".into(),
                Mode::Text => "Tìm trong project".into(),
                Mode::List => String::new(),
                Mode::Symbols => "Tìm symbol (jdtls)".into(),
                Mode::Actions => "Tìm action".into(),
                Mode::Everywhere => "Tìm mọi nơi (Shift Shift)".into(),
            },
            capturing: None,
            all_hits: Arc::new(Vec::new()),
            client: None,
        };
        if mode == Mode::Actions {
            p.busy = false;
            p.refresh(String::new(), cx);
            return p;
        }
        if matches!(mode, Mode::List | Mode::Symbols) {
            p.busy = false;
            return p;
        }
        // listing files is cheap but not free on big trees: do it off the UI thread
        let initial = initial.to_string();
        let task = cx.background_executor().spawn(async move { search::list_files(&root) });
        cx.spawn(async move |this, cx| {
            let files = task.await;
            let _ = this.update(cx, |p, cx| {
                p.files = Arc::new(files);
                p.busy = false;
                if initial.is_empty() {
                    p.refresh(String::new(), cx);
                } else {
                    p.input.update(cx, |i, cx| i.set_text(&initial, cx));
                }
            });
        })
        .detach();
        p.busy = true;
        p
    }

    /// A fixed list of locations (Find Usages, Problems).
    pub fn with_hits(title: String, root: PathBuf, hits: Vec<Hit>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut p = Self::new(Mode::List, root, "", window, cx);
        p.title = title;
        p.all_hits = Arc::new(hits);
        p.items = p.all_hits.iter().cloned().map(Item::Hit).collect();
        p
    }

    /// A fixed list of files (Ctrl+E Recent Files), filtered fuzzily; order kept when empty.
    pub fn with_files(title: String, root: PathBuf, files: Vec<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut p = Self::new(Mode::List, root, "", window, cx);
        p.mode = Mode::Files;
        p.title = title;
        p.files = Arc::new(files);
        p.busy = false;
        p.refresh(String::new(), cx);
        p
    }

    pub fn everywhere(root: PathBuf, client: Option<Arc<lsp_client::Client>>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut p = Self::new(Mode::Everywhere, root, "", window, cx);
        p.client = client;
        p
    }

    pub fn symbols(root: PathBuf, client: Arc<lsp_client::Client>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut p = Self::new(Mode::Symbols, root, "", window, cx);
        p.client = Some(client);
        p
    }

    fn refresh(&mut self, query: String, cx: &mut Context<Self>) {
        if self.mode == Mode::Symbols {
            let Some(client) = self.client.clone() else { return };
            self.generation += 1;
            let generation = self.generation;
            if query.trim().is_empty() {
                self.items.clear();
                cx.notify();
                return;
            }
            self.busy = true;
            let req = client.request("workspace/symbol", serde_json::json!({ "query": query }));
            let id = req.id;
            cx.spawn(async move |this, cx| {
                let res = req.response().await;
                let _ = this.update(cx, |p, cx| {
                    if p.generation != generation {
                        return;
                    }
                    p.busy = false;
                    let mut syms = res.ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
                    // project sources before library classes (jdt:// can't be listed as files yet)
                    syms.sort_by_key(|s| !s["location"]["uri"].as_str().is_some_and(|u| u.starts_with("file:")));
                    p.items = syms
                        .iter()
                        .filter_map(|s| {
                            let uri = s["location"]["uri"].as_str()?;
                            // a class in a jar: the jdt:// uri rides in the path, the workspace opens it
                            let path = lsp_client::uri_to_path(uri).or_else(|| uri.starts_with("jdt:").then(|| PathBuf::from(uri)))?;
                            let line = s["location"]["range"]["start"]["line"].as_u64().unwrap_or(0) as usize;
                            let container = s["containerName"].as_str().unwrap_or("");
                            Some(Item::Hit(Hit { path, line, col: 0, text: format!("{}  {container}", s["name"].as_str()?), mark: None }))
                        })
                        .take(MAX_FILES)
                        .collect();
                    p.selected = 0;
                    cx.notify();
                });
            })
            .detach();
            // drop the previous in-flight query
            let _ = id;
            return;
        }
        if self.mode == Mode::Actions {
            let q = query.to_lowercase();
            let words: Vec<&str> = q.split_whitespace().collect();
            self.items = crate::keymap::CATALOG
                .iter()
                .filter(|a| {
                    let hay = format!("{} {} {}", a.title, a.group, a.name).to_lowercase();
                    words.iter().all(|w| hay.contains(w))
                })
                .map(|a| Item::Action { name: a.name, title: a.title, group: a.group, keys: crate::keymap::shortcuts(cx, a.name) })
                .collect();
            self.selected = self.selected.min(self.items.len().saturating_sub(1));
            cx.notify();
            return;
        }
        if self.mode == Mode::List {
            let q = query.to_lowercase();
            self.items = self
                .all_hits
                .iter()
                .filter(|h| q.is_empty() || h.text.to_lowercase().contains(&q) || h.path.to_string_lossy().to_lowercase().contains(&q))
                .cloned()
                .map(Item::Hit)
                .collect();
            self.selected = 0;
            cx.notify();
            return;
        }
        if self.mode == Mode::Everywhere {
            return self.refresh_everywhere(query, cx);
        }
        self.generation += 1;
        let generation = self.generation;
        let files = self.files.clone();
        let root = self.root.clone();
        let mode = self.mode;
        self.busy = true;
        let task = cx.background_executor().spawn(async move {
            match mode {
                Mode::Files => search::fuzzy_files(&files, &query, MAX_FILES).into_iter().map(Item::File).collect::<Vec<_>>(),
                Mode::Text if query.chars().count() < 2 => Vec::new(),
                Mode::Text => search::search_text(&root, &files, &query, MAX_HITS).into_iter().map(Item::Hit).collect(),
                Mode::List | Mode::Symbols | Mode::Actions | Mode::Everywhere => Vec::new(),
            }
        });
        cx.spawn(async move |this, cx| {
            let items = task.await;
            let _ = this.update(cx, |p, cx| {
                if p.generation == generation {
                    p.items = items;
                    p.selected = 0;
                    p.busy = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Files (fuzzy) and actions at once, then the project's classes/symbols from jdtls on top.
    fn refresh_everywhere(&mut self, query: String, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        let words: Vec<String> = query.to_lowercase().split_whitespace().map(String::from).collect();
        let actions: Vec<Item> = if words.is_empty() {
            Vec::new()
        } else {
            crate::keymap::CATALOG
                .iter()
                .filter(|a| {
                    let hay = format!("{} {} {}", a.title, a.group, a.name).to_lowercase();
                    words.iter().all(|w| hay.contains(w.as_str()))
                })
                .take(8)
                .map(|a| Item::Action { name: a.name, title: a.title, group: a.group, keys: crate::keymap::shortcuts(cx, a.name) })
                .collect()
        };
        let files = self.files.clone();
        let q = query.clone();
        let task = cx.background_executor().spawn(async move { search::fuzzy_files(&files, &q, if q.trim().is_empty() { 30 } else { 15 }) });
        let symbols = (query.trim().chars().count() >= 2).then(|| self.client.clone()).flatten().map(|c| c.request("workspace/symbol", serde_json::json!({ "query": query })));
        self.busy = true;
        cx.spawn(async move |this, cx| {
            let files: Vec<Item> = task.await.into_iter().map(Item::File).collect();
            let _ = this.update(cx, |p, cx| {
                if p.generation == generation {
                    p.items = files.iter().cloned().chain(actions.iter().cloned()).collect();
                    p.selected = 0;
                    p.busy = symbols.is_some();
                    cx.notify();
                }
            });
            let Some(req) = symbols else { return };
            let res = req.response().await;
            let syms: Vec<Item> = res
                .ok()
                .and_then(|v| v.as_array().cloned())
                .unwrap_or_default()
                .iter()
                // project sources first, then classes of jars (jdt://)
                .filter(|s| s["location"]["uri"].as_str().is_some_and(|u| u.starts_with("file:") || u.starts_with("jdt:")))
                .take(20)
                .filter_map(|s| {
                    let uri = s["location"]["uri"].as_str()?;
                    let path = lsp_client::uri_to_path(uri).or_else(|| uri.starts_with("jdt:").then(|| PathBuf::from(uri)))?;
                    let line = s["location"]["range"]["start"]["line"].as_u64().unwrap_or(0) as usize;
                    let container = s["containerName"].as_str().unwrap_or("");
                    Some(Item::Hit(Hit { path, line, col: 0, text: format!("{}  {container}", s["name"].as_str()?), mark: None }))
                })
                .collect();
            let _ = this.update(cx, |p, cx| {
                if p.generation == generation {
                    p.items = syms.into_iter().chain(files).chain(actions).collect();
                    p.selected = 0;
                    p.busy = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn select(&mut self, i: usize, cx: &mut Context<Self>) {
        if self.items.is_empty() {
            return;
        }
        self.selected = i.min(self.items.len() - 1);
        self.scroll.scroll_to_item(self.selected, ScrollStrategy::Top);
        cx.notify();
    }

    fn confirm(&mut self, cx: &mut Context<Self>) {
        let Some(item) = self.items.get(self.selected) else { return };
        let ev = match item {
            Item::File(rel) => PickerEvent::Open { path: self.root.join(rel), line: None, col: 0 },
            Item::Hit(h) => PickerEvent::Open { path: h.path.clone(), line: Some(h.line), col: h.col },
            Item::Action { name, .. } => PickerEvent::RunAction(name.to_string()),
        };
        cx.emit(ev);
    }

    /// Find Action: start recording a new shortcut for the selected action.
    fn start_capture(&mut self, cx: &mut Context<Self>) {
        let Some(Item::Action { name, .. }) = self.items.get(self.selected) else { return };
        self.capturing = Some(name);
        cx.set_global(Capturing(Some(cx.entity().downgrade())));
        cx.notify();
    }

    fn captured(&mut self, k: &gpui::Keystroke, cx: &mut Context<Self>) {
        let Some(name) = self.capturing else { return };
        if MODIFIER_KEYS.contains(&k.key.as_str()) {
            return; // wait for the real key
        }
        let plain = !(k.modifiers.control || k.modifiers.alt || k.modifiers.shift || k.modifiers.platform);
        self.capturing = None;
        cx.set_global(Capturing(None));
        match k.key.as_str() {
            "escape" if plain => {}
            "backspace" | "delete" if plain => cx.emit(PickerEvent::Rebind { name: name.to_string(), keys: Vec::new() }),
            _ => cx.emit(PickerEvent::Rebind { name: name.to_string(), keys: vec![k.unparse()] }),
        }
        cx.notify();
    }

    /// Re-reads the shortcuts after a rebind.
    pub fn reload_actions(&mut self, cx: &mut Context<Self>) {
        let q = self.input.read(cx).content.clone();
        self.refresh(q, cx);
    }
}

impl Focusable for Picker {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }
}

/// One list row, ready to draw.
#[derive(Clone)]
struct RowView {
    /// file icon from this path; None = action marker
    icon: Option<PathBuf>,
    /// file name (hits) or file name (Go to File)
    name: String,
    /// 1-based line for hits
    line: Option<usize>,
    /// code snippet / directory
    text: String,
    mark: Option<(usize, usize)>,
    /// shown in the footer when selected
    path: String,
    /// Find Action: current shortcuts
    keys: Vec<String>,
}


impl Render for Picker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title = self.title.clone();
        let n = self.items.len();
        let count = if self.busy {
            "…".to_string()
        } else if self.mode == Mode::Files {
            format!("{n} file")
        } else {
            format!("{n} kết quả")
        };
        let root = self.root.clone();
        let rows: Vec<RowView> = self
            .items
            .iter()
            .map(|it| match it {
                Item::File(rel) => {
                    let p = std::path::Path::new(rel);
                    RowView {
                        icon: Some(self.root.join(p)),
                        name: crate::tree::file_name(p),
                        line: None,
                        text: p.parent().map(|d| d.display().to_string()).unwrap_or_default(),
                        mark: None,
                        path: rel.clone(),
                        keys: Vec::new(),
                    }
                }
                Item::Hit(h) => {
                    let rel = h.path.strip_prefix(&root).unwrap_or(&h.path);
                    let p = h.path.to_string_lossy();
                    // jdt://contents/lib.jar/pkg/Name.class?=… → "Name.class", the jar as its path
                    let (name, path) = if p.starts_with("jdt:") {
                        let clean = p.split('?').next().unwrap_or(&p);
                        let jar = clean.trim_start_matches("jdt://contents/").split('/').next().unwrap_or("").to_string();
                        (clean.rsplit('/').next().unwrap_or("").to_string(), format!("{jar} (thư viện)"))
                    } else {
                        (crate::tree::file_name(&h.path), format!("{}:{}", rel.display(), h.line + 1))
                    };
                    RowView {
                        icon: Some(h.path.clone()),
                        name,
                        line: Some(h.line + 1),
                        text: h.text.clone(),
                        mark: h.mark,
                        path,
                        keys: Vec::new(),
                    }
                }
                Item::Action { name, title, group, keys } => RowView {
                    icon: None,
                    name: title.to_string(),
                    line: None,
                    text: group.to_string(),
                    mark: None,
                    path: name.to_string(),
                    keys: keys.clone(),
                },
            })
            .collect();
        let selected = self.selected;
        let actions_mode = self.mode == Mode::Actions;
        let capturing = self.capturing.is_some();
        let footer = if let Some(name) = self.capturing {
            let title = crate::keymap::CATALOG.iter().find(|a| a.name == name).map(|a| a.title).unwrap_or(name);
            format!("Nhấn tổ hợp phím mới cho «{title}»…   Esc: hủy · Backspace: bỏ phím tắt")
        } else if actions_mode {
            format!("Enter: chạy · Ctrl+Enter: đổi phím tắt · lưu ở {}", crate::keymap::path().display())
        } else {
            rows.get(selected).map(|r| r.path.clone()).unwrap_or_default()
        };
        let entity = cx.entity();
        let files_mode = self.mode == Mode::Files || actions_mode;
        // IntelliJ usages popup: [icon] File.java | line | code with the match bold
        let name_w = px(if actions_mode { 420. } else if files_mode { 300. } else { 260. });

        div()
            .key_context(if capturing { "Picker KeyCapture" } else { "Picker" })
            .debug_selector(|| "picker".into())
            .on_action(cx.listener(|this, _: &ChangeShortcut, _, cx| this.start_capture(cx)))
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.select(this.selected + 1, cx)))
            .on_action(cx.listener(|this, _: &SelectPrev, _, cx| this.select(this.selected.saturating_sub(1), cx)))
            .on_action(cx.listener(|this, _: &Confirm, _, cx| this.confirm(cx)))
            .on_action(cx.listener(|_, _: &Dismiss, _, cx| cx.emit(PickerEvent::Dismissed)))
            .w(px(980.))
            .max_h(px(620.))
            .flex()
            .flex_col()
            .bg(c(theme::POPUP_BG))
            .border_1()
            .border_color(c(theme::BORDER))
            .rounded_lg()
            .shadow_lg().occlude()
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .px_3()
                    .pt_2()
                    .pb_1()
                    .child(div().text_color(c(theme::FG)).font_weight(gpui::FontWeight::SEMIBOLD).whitespace_nowrap().overflow_hidden().text_ellipsis().child(title))
                    .child(div().text_color(c(theme::FG_DIM)).whitespace_nowrap().child(count)),
            )
            .child(div().px_3().pb_2().child(self.input.clone()))
            .child(div().h(px(1.)).bg(c(theme::BORDER)))
            .child(
                uniform_list("picker-items", rows.len(), move |range, _, _| {
                    range
                        .map(|i| {
                            let r = rows[i].clone();
                            let e = entity.clone();
                            let active = i == selected;
                            let snippet = match r.mark {
                                Some((s, e)) if !files_mode => div()
                                    .flex()
                                    .child(div().text_color(c(theme::FG)).child(r.text[..s].to_string()))
                                    .child(div().text_color(c(theme::CURSOR)).font_weight(gpui::FontWeight::BOLD).child(r.text[s..e].to_string()))
                                    .child(div().text_color(c(theme::FG)).child(r.text[e..].to_string())),
                                _ => div().text_color(c(if files_mode { theme::FG_DIM } else { theme::FG })).child(r.text.clone()),
                            };
                            div()
                                .id(("pick", i))
                                .flex()
                                .items_center()
                                .gap_2()
                                .mx_1()
                                .px_2()
                                .py(px(1.))
                                .rounded_md()
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .when(active, |d| d.bg(c(theme::ROW_ACTIVE)))
                                .when(!active, |d| d.hover(|s| s.bg(c(theme::ROW_HOVER))))
                                .child(match &r.icon {
                                    Some(p) => crate::icons::file_icon(p),
                                    None => div().flex_none().w(px(15.)).text_color(c(theme::ACCENT)).font_weight(gpui::FontWeight::BOLD).child("▸").into_any_element(),
                                })
                                .child(div().flex_none().w(name_w).overflow_hidden().text_ellipsis().text_color(c(theme::FG)).child(r.name))
                                .when_some(r.line, |d, l| {
                                    d.child(div().flex_none().w(px(52.)).flex().justify_end().pr_3().text_color(c(theme::FG_DIM)).child(l.to_string()))
                                })
                                .child(div().flex_1().min_w_0().overflow_hidden().when(!files_mode, |d| d.font_family("JetBrains Mono")).child(snippet))
                                .children(r.keys.iter().map(|k| {
                                    div().flex_none().px_2().rounded_md().border_1().border_color(c(theme::BORDER)).bg(c(theme::BG)).text_size(px(12.)).text_color(c(theme::FG)).child(k.clone())
                                }))
                                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                    e.update(cx, |p, cx| {
                                        p.selected = i;
                                        p.confirm(cx)
                                    })
                                })
                        })
                        .collect()
                })
                .track_scroll(self.scroll.clone())
                .py_1()
                .h(px(470.)),
            )
            .child(div().h(px(1.)).bg(c(theme::BORDER)))
            .child(
                div()
                    .px_3()
                    .py_1()
                    .text_size(px(12.))
                    .text_color(c(if capturing { theme::WARNING } else { theme::FG_DIM }))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(footer),
            )
    }
}
