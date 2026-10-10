//! Main window: project tree | editor tabs, status bar at the bottom.
use crate::actions::*;
use crate::editor_view::EditorView;
use crate::lsp_workspace::{LspState, NavEntry};
use crate::picker::{Mode, Picker, PickerEvent};
use crate::settings::Settings;
use crate::theme::{self, c};
use crate::tree::{NodeId, ProjectTree, Row};
use gpui::{
    Context, Entity, FocusHandle, Focusable, MouseButton, PromptLevel, SharedString, Window, div, prelude::*, px, uniform_list,
};
use project_model::{BuildOpts, Project};
use std::path::{Path, PathBuf};

pub struct Workspace {
    pub(crate) root: PathBuf,
    pub(crate) settings: Settings,
    pub(crate) project: Option<Project>,
    /// JSF bean name → class index (built lazily; dropped when a .java file changes)
    pub(crate) jsf_index: Option<std::sync::Arc<jsf::beans::BeanIndex>>,
    /// Spring Boot configuration metadata from the jars (loaded on first use, per model)
    pub(crate) spring_meta: Option<crate::spring_support::Metadata>,
    /// JSF tag libraries for tag / attribute completion in .xhtml
    pub(crate) jsf_tags: Option<std::sync::Arc<jsf::tags::TagLibs>>,
    model_state: ModelState,
    pub(crate) tree: ProjectTree,
    pub(crate) tabs: Vec<Entity<EditorView>>,
    pub(crate) active: Option<usize>,
    pub(crate) focus: FocusHandle,
    pub(crate) message: Option<String>,
    pub(crate) picker: Option<Entity<Picker>>,
    pub(crate) lsp: Option<std::sync::Arc<lsp_client::Client>>,
    pub(crate) lsp_state: LspState,
    /// jdtls background jobs (language/progressReport id → task), shown as a progress bar
    pub(crate) lsp_jobs: std::collections::BTreeMap<String, crate::lsp_workspace::Job>,
    /// Run/Debug/Test asked while jdtls was busy: started when it is idle
    pub(crate) pending_run: Option<crate::lsp_workspace::PendingRun>,
    /// uri → diagnostics as published (also for files that are not open)
    pub(crate) diagnostics: std::collections::HashMap<String, Vec<serde_json::Value>>,
    pub(crate) nav_back: Vec<NavEntry>,
    pub(crate) nav_forward: Vec<NavEntry>,
    // ---- run / debug / maven ----
    pub(crate) sessions: Vec<crate::debug::Session>,
    pub(crate) active_session: Option<usize>,
    pub(crate) next_session: usize,
    pub(crate) panel_open: bool,
    pub(crate) maven_tab: bool,
    /// the Maven tool window takes the place of the project tree (IntelliJ's docked tool window)
    pub(crate) maven_side: bool,
    /// the "Module Spring Boot mới" dialog
    pub(crate) new_module: Option<Entity<crate::new_project::NewProjectDialog>>,
    pub(crate) skip_tests: bool,
    pub(crate) breakpoints: std::collections::HashMap<PathBuf, std::collections::BTreeSet<usize>>,
    /// conditions of conditional breakpoints (0-based line → Java expression)
    pub(crate) bp_conditions: std::collections::HashMap<PathBuf, std::collections::BTreeMap<usize, String>>,
    /// open "breakpoint condition" dialog: file, 0-based line, input
    pub(crate) bp_dialog: Option<(PathBuf, usize, Entity<crate::input::TextInput>)>,
    pub(crate) config_editor: Option<crate::config_dialog::ConfigEditor>,
    pub(crate) maven_ui: Option<crate::maven_panel::MavenUi>,
    pub(crate) context_menu: Option<crate::context_menu::ContextMenu>,
    pub(crate) name_prompt: Option<crate::context_menu::NamePrompt>,
    pub(crate) replace_files: Option<crate::replace_dialog::ReplaceInFiles>,
    /// most recently shown files first (Ctrl+E)
    pub(crate) recent_files: Vec<PathBuf>,
    /// IntelliJ-style notification balloons (bottom right): text, shown at, error?
    pub(crate) toasts: Vec<(String, std::time::Instant, bool)>,
    /// the message last turned into a balloon (workspace, active tab)
    toast_seen: (Option<String>, Option<String>),
    pub(crate) git: crate::git_support::GitState,
    /// Git window: selection, commit message
    pub(crate) git_ui: Option<crate::git_panel::GitUi>,
    /// the Log window (commit graph)
    pub(crate) log_view: Option<crate::git_log::LogView>,
    pub(crate) db: Option<crate::db_view::DbView>,
    pub(crate) db_open: bool,
    /// a branch delete refused as unmerged: ask for the forced delete
    pub(crate) pending_force_delete: Option<String>,
    pub(crate) accounts_dialog: Option<crate::git_support::AccountsDialog>,
    /// bottom panel shows the Git tool window
    pub(crate) git_tab: bool,
    pub(crate) diff_view: Option<crate::git_panel::DiffView>,
    pub(crate) conflict_view: Option<crate::git_panel::ConflictView>,
    /// a push/pull failed for lack of a login: open the accounts dialog on the next render
    pub(crate) pending_accounts: bool,
    /// a pull / merge stopped on conflicts: open the Git window (needs the `Window`, so it waits for render)
    pub(crate) pending_git_window: bool,
    pub(crate) bookmarks: std::collections::HashMap<PathBuf, std::collections::BTreeSet<usize>>,
    /// the next test run collects coverage (Run with Coverage)
    pub(crate) coverage_next: bool,
    /// last coverage report: file → per-line coverage
    pub(crate) coverage: std::collections::HashMap<PathBuf, crate::coverage::LineCoverage>,
    /// double-Shift detection
    last_shift: Option<std::time::Instant>,
    shift_down: bool,
    pub(crate) confirm: Option<crate::context_menu::Confirm>,
    /// start jdtls again once the running model reload finishes (module list changed)
    pub(crate) restart_lsp_after_reload: bool,
    /// watches the project folder for changes made outside the IDE
    pub(crate) watcher: Option<notify::RecommendedWatcher>,
    // ---- debugger values (debug_values.rs) ----
    pub(crate) value_popup: Option<crate::debug_values::ValuePopup>,
    pub(crate) eval_dialog: Option<crate::debug_values::EvalDialog>,
    pub(crate) watches: Vec<String>,
    pub(crate) watch_values: Vec<Option<Result<crate::debug::Var, String>>>,
    pub(crate) watch_input: Option<Entity<crate::input::TextInput>>,
    /// resizable areas (drag handles) and the drag in progress
    pub(crate) sizes: crate::layout::Sizes,
    pub(crate) drag: Option<crate::layout::Drag>,
    /// console: keep scrolled to the newest line / wrap long lines
    pub(crate) console_follow: bool,
    pub(crate) console_wrap: bool,
    pub(crate) console_wrap_scroll: gpui::ScrollHandle,
    /// (file, 0-based line) to show as the execution point on the next render
    pub(crate) pending_exec: Option<(PathBuf, usize)>,
    pub(crate) config_menu_open: bool,
    /// where the configuration dropdown opens (under the selector that was clicked)
    pub(crate) config_menu_at: gpui::Point<gpui::Pixels>,
    pub(crate) console_scroll: gpui::UniformListScrollHandle,
    pub(crate) tree_scroll: gpui::UniformListScrollHandle,
    /// set by reveal_in_tree: scroll this file's row into view on the next render
    tree_scroll_to: Option<PathBuf>,
    /// integrated terminals of this project (Alt+F12)
    pub(crate) terminals: Vec<Entity<crate::terminal::TerminalView>>,
    pub(crate) active_terminal: usize,
    pub(crate) terminal_tab: bool,
    /// the active project tab (jdtls starts lazily for shown projects only)
    pub(crate) visible: bool,
    /// restore the editor session on the first render (needs a Window)
    session_loaded: bool,
}

enum ModelState {
    Loading,
    Ready,
    Failed(String),
}

impl Workspace {
    pub fn new(root: PathBuf, settings: Settings, settings_error: Option<String>, cx: &mut Context<Self>) -> Self {
        let mut ws = Self {
            tree: ProjectTree::new(root.clone()),
            root,
            settings,
            project: None,
            jsf_index: None,
            spring_meta: None,
            jsf_tags: None,
            model_state: ModelState::Loading,
            tabs: Vec::new(),
            active: None,
            focus: cx.focus_handle(),
            message: settings_error,
            picker: None,
            lsp: None,
            lsp_state: LspState::Off,
            lsp_jobs: Default::default(),
            pending_run: None,
            diagnostics: Default::default(),
            nav_back: Vec::new(),
            nav_forward: Vec::new(),
            sessions: Vec::new(),
            active_session: None,
            next_session: 0,
            panel_open: false,
            maven_tab: false,
            maven_side: false,
            new_module: None,
            skip_tests: false,
            breakpoints: Default::default(),
            bp_conditions: Default::default(),
            bp_dialog: None,
            config_editor: None,
            maven_ui: None,
            context_menu: None,
            name_prompt: None,
            replace_files: None,
            recent_files: Vec::new(),
            toasts: Vec::new(),
            toast_seen: (None, None),
            git: Default::default(),
            git_ui: None,
            log_view: None,
            db: None,
            db_open: false,
            pending_force_delete: None,
            accounts_dialog: None,
            git_tab: false,
            diff_view: None,
            conflict_view: None,
            pending_accounts: false,
            pending_git_window: false,
            bookmarks: Default::default(),
            coverage_next: false,
            coverage: Default::default(),
            last_shift: None,
            shift_down: false,
            confirm: None,
            restart_lsp_after_reload: false,
            watcher: None,
            value_popup: None,
            eval_dialog: None,
            watches: Vec::new(),
            watch_values: Vec::new(),
            watch_input: None,
            sizes: crate::layout::Sizes { tree: 340., panel: 300., maven_split: 520., panel_w: 560., frames: 300., vars: 420., test_tree: 460. },
            drag: None,
            console_follow: true,
            console_wrap: false,
            console_wrap_scroll: gpui::ScrollHandle::new(),
            pending_exec: None,
            config_menu_open: false,
            config_menu_at: gpui::point(px(0.), px(0.)),
            console_scroll: gpui::UniformListScrollHandle::new(),
            tree_scroll: gpui::UniformListScrollHandle::new(),
            tree_scroll_to: None,
            terminals: Vec::new(),
            active_terminal: 0,
            terminal_tab: false,
            visible: false,
            session_loaded: false,
        };
        ws.start_watcher(cx);
        ws.refresh_git(cx);
        ws.load_watches();
        ws.load_breakpoints();
        ws.load_bookmarks();
        ws.load_sizes();
        ws.skip_tests = runner::config::maven_prefs(&ws.root_str()).skip_tests;
        cx.on_app_quit(|ws, cx| {
            ws.save_session(cx);
            ws.shutdown_sessions();
            ws.shutdown_lsp()
        })
        .detach();
        // keep the session file fresh like nvim's 30 s timer
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(std::time::Duration::from_secs(30)).await;
            // commits / branch switches made in a terminal don't touch watched files
            if this.update(cx, |ws, cx| {
                ws.save_session(cx);
                ws.refresh_git(cx)
            })
            .is_err()
            {
                break;
            }
        })
        .detach();
        ws.load_model(false, cx);
        ws
    }

    /// Resolves the Maven model off the UI thread. Uses the shared model-cache.json,
    /// so an unchanged project opens without running mvn.
    pub(crate) fn model_loading(&self) -> bool {
        matches!(self.model_state, ModelState::Loading)
    }

    /// Re-resolves the Maven model now (Reload button / Ctrl+Shift+O), with the panel's profiles.
    /// Reload: re-run Maven online (which downloads jars missing from ~/.m2, e.g. deleted from
    /// the tree), refresh the tree, and make jdtls re-import every module's classpath.
    pub(crate) fn reload_model(&mut self, cx: &mut Context<Self>) {
        self.message = Some("Đang reload Maven (tải lại jar còn thiếu)…".into());
        self.load_model(true, cx);
    }

    /// After a reload: tell jdtls the poms changed so it re-reads dependencies, and report
    /// jars that are still missing (offline, or not downloadable).
    fn after_reload(&mut self, cx: &mut Context<Self>) {
        let Some(p) = self.project.as_ref() else { return };
        let missing = crate::debug::missing_jars(p);
        let poms: Vec<serde_json::Value> = p
            .modules
            .iter()
            .map(|m| std::path::Path::new(&m.path).join("pom.xml"))
            .filter(|pom| pom.is_file())
            .map(|pom| serde_json::json!({ "uri": lsp_client::path_to_uri(&pom) }))
            .collect();
        let n = p.modules.len();
        let jdtls = match self.lsp.as_ref() {
            Some(client) => {
                client.notify("java/projectConfigurationsUpdate", serde_json::json!({ "identifiers": poms }));
                ", jdtls đang cập nhật classpath"
            }
            None => "",
        };
        self.message = Some(if missing.is_empty() {
            format!("Reload xong: {n} module, jar đầy đủ{jdtls}")
        } else {
            format!("Reload xong nhưng còn {} jar chưa tải được (mạng/offline?), vd: {}", missing.len(), crate::tree::file_name(&missing[0]))
        });
        cx.notify();
    }

    fn load_model(&mut self, force: bool, cx: &mut Context<Self>) {
        self.model_state = ModelState::Loading;
        let root = self.root.clone();
        let profiles = self.maven_profiles();
        let task = cx.background_executor().spawn(async move {
            project_model::open_project(&root, BuildOpts { force, quiet: true, profiles, ..Default::default() })
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |ws, cx| {
                match result {
                    Ok(p) if force && ws.project.is_some() => {
                        ws.project = Some(p);
                        ws.spring_meta = None;
                        ws.jsf_tags = None;
                        ws.model_state = ModelState::Ready;
                        ws.tree.refresh();
                        ws.after_reload(cx);
                        // module list changed: a fresh jdtls picks up the new workspace folders
                        if std::mem::take(&mut ws.restart_lsp_after_reload) && ws.settings.jdtls_enabled && ws.visible {
                            ws.start_lsp(cx);
                        }
                    }
                    Ok(p) => {
                        // open the first module so the tree isn't a wall of collapsed rows
                        if ws.project.is_none() && p.modules.len() == 1 {
                            ws.tree.expand(NodeId::Module(PathBuf::from(&p.modules[0].path)));
                        }
                        ws.project = Some(p);
                        ws.spring_meta = None;
                        ws.jsf_tags = None;
                        ws.model_state = ModelState::Ready;
                        if ws.settings.jdtls_enabled && ws.visible {
                            ws.start_lsp(cx);
                        }
                    }
                    Err(e) => {
                        ws.message = Some(format!("Reload Maven lỗi: {}", format!("{e:#}").lines().next().unwrap_or("")));
                        ws.model_state = ModelState::Failed(format!("{e:#}"));
                    }
                }
                ws.tree.refresh();
                cx.notify();
            });
        })
        .detach();
    }

    /// Close the topmost git / database overlay; false when Esc belongs to something else.
    pub(crate) fn esc_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.confirm.is_some() || self.name_prompt.is_some() || self.context_menu.is_some() {
            return false;
        }
        if self.diff_view.take().is_some() || self.conflict_view.take().is_some() {
            cx.notify();
            return true;
        }
        if self.log_view.is_some() {
            self.close_log(window, cx);
            return true;
        }
        if self.db_open {
            self.db_escape(cx);
            return true;
        }
        false
    }

    pub(crate) fn open_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(i) = self.tabs.iter().position(|t| t.read(cx).path() == Some(&path)) {
            self.activate(i, window, cx);
            return;
        }
        let buffer = match editor_core::Buffer::open(&path) {
            Ok(b) => b,
            Err(e) => {
                self.message = Some(format!("Không mở được {}: {e}", path.display()));
                cx.notify();
                return;
            }
        };
        let tab_width = self.settings.tab_width;
        let view = cx.new(|cx| EditorView::new(buffer, tab_width, cx));
        cx.observe(&view, |_, _, cx| cx.notify()).detach();
        self.subscribe_tab(&view, window, cx);
        self.attach_tab(&view, cx);
        self.jsf_check(view.clone(), cx);
        self.git_attach(&view, cx);
        {
            let on = self.settings.keymap_mode == "Vim";
            if on {
                let cfg = crate::vim_mode::load_config();
                let keys = self.settings.vim_ctrl_keys.clone();
                view.update(cx, |v, _| v.set_vim(true, keys, &cfg));
            }
        }
        let marks = self.tab_bookmarks(&path);
        let cov = self.coverage.get(&path).cloned().unwrap_or_default();
        view.update(cx, |v, _| {
            v.bookmarks = marks;
            v.coverage = cov;
        });
        self.jsf_lens(view.clone(), cx);
        let bps: Vec<usize> = self.breakpoints.get(&path).map(|s| s.iter().copied().collect()).unwrap_or_default();
        let conds: Vec<usize> = self.bp_conditions.get(&path).map(|c| c.keys().copied().collect()).unwrap_or_default();
        view.update(cx, |v, _| {
            v.breakpoints = bps;
            v.conditional_breakpoints = conds;
        });
        self.message = None;
        self.tabs.push(view);
        self.activate(self.tabs.len() - 1, window, cx);
        self.save_session(cx);
    }

    pub(crate) fn activate(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        if i < self.tabs.len() {
            self.active = Some(i);
            if let Some(p) = self.tabs[i].read(cx).path().cloned() {
                self.recent_files.retain(|r| *r != p);
                self.recent_files.insert(0, p);
                self.recent_files.truncate(50);
            }
            let fh = self.tabs[i].read(cx).focus_handle(cx);
            window.focus(&fh);
            cx.notify();
        }
    }

    pub(crate) fn close_tab(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        if i >= self.tabs.len() {
            return;
        }
        if self.tabs[i].read(cx).is_dirty() {
            let name = self.tabs[i].read(cx).title();
            let answer = window.prompt(
                PromptLevel::Warning,
                &format!("{name} chưa lưu"),
                Some("Lưu thay đổi trước khi đóng?"),
                &["Lưu", "Không lưu", "Hủy"],
                cx,
            );
            let tab = self.tabs[i].clone();
            cx.spawn_in(window, async move |this, cx| {
                let Ok(choice) = answer.await else { return };
                let _ = this.update_in(cx, |ws, window, cx| {
                    let Some(i) = ws.tabs.iter().position(|t| *t == tab) else { return };
                    match choice {
                        0 => {
                            tab.update(cx, |v, _| v.save());
                            if tab.read(cx).is_dirty() {
                                return; // save failed: keep the tab, the error is in the status bar
                            }
                        }
                        1 => {}
                        _ => return,
                    }
                    ws.remove_tab(i, window, cx);
                });
            })
            .detach();
            return;
        }
        self.remove_tab(i, window, cx);
    }

    /// Asks about unsaved files of this project: resolves true when it may close
    /// (saved or discarded), false on Cancel or a failed save.
    pub fn confirm_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::Task<bool> {
        let dirty: Vec<Entity<EditorView>> = self.tabs.iter().filter(|t| t.read(cx).is_dirty()).cloned().collect();
        if dirty.is_empty() {
            self.save_session(cx);
            return gpui::Task::ready(true);
        }
        let names: Vec<String> = dirty.iter().map(|t| t.read(cx).title()).collect();
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("{}: {} file chưa lưu", crate::projects::title(&self.root), dirty.len()),
            Some(&names.join(", ")),
            &["Lưu tất cả", "Không lưu", "Hủy"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let Ok(choice) = answer.await else { return false };
            this.update_in(cx, |ws, _, cx| {
                match choice {
                    0 => {
                        for t in &dirty {
                            t.update(cx, |v, _| {
                                v.save();
                                v.lsp_did_save();
                            });
                        }
                        if let Some(t) = dirty.iter().find(|t| t.read(cx).is_dirty()) {
                            ws.message = t.read(cx).message.clone();
                            cx.notify();
                            return false; // a save failed: stay open
                        }
                    }
                    1 => {}
                    _ => return false,
                }
                ws.save_session(cx);
                true
            })
            .unwrap_or(false)
        })
    }

    /// Stops this project's jdtls and run/debug sessions (project tab closed).
    pub fn shutdown(&mut self, _cx: &mut Context<Self>) -> impl std::future::Future<Output = ()> + 'static + use<> {
        self.shutdown_sessions();
        self.shutdown_lsp()
    }

    /// Shown/hidden as the active project tab. jdtls starts the first time a project is shown.
    pub fn set_visible(&mut self, on: bool, cx: &mut Context<Self>) {
        self.visible = on;
        if on && matches!(self.model_state, ModelState::Ready) && self.settings.jdtls_enabled {
            self.start_lsp(cx);
        }
        if !on {
            self.save_session(cx);
        }
    }

    /// New settings from the Settings dialog (fonts, sizes, jdtls memory…).
    pub fn apply_settings(&mut self, settings: Settings, cx: &mut Context<Self>) {
        // sizes typed in the Settings dialog override the dragged ones
        if settings.tree_width != self.settings.tree_width {
            self.sizes.tree = settings.tree_width;
        }
        if settings.panel_height != self.settings.panel_height {
            self.sizes.panel = settings.panel_height;
        }
        self.sizes = self.sizes.clamp();
        self.settings = settings;
        self.apply_vim(cx);
        for t in &self.tabs {
            t.update(cx, |_, cx| cx.notify());
        }
        cx.notify();
    }

    /// Vim on/off + Ctrl key split + vimrc for every tab.
    pub(crate) fn apply_vim(&mut self, cx: &mut Context<Self>) {
        let on = self.settings.keymap_mode == "Vim";
        let cfg = if on { crate::vim_mode::load_config() } else { Default::default() };
        let keys = self.settings.vim_ctrl_keys.clone();
        for t in &self.tabs {
            t.update(cx, |v, cx| {
                // class files from jars stay IntelliJ-style (read-only anyway)
                v.set_vim(on && !v.read_only, keys.clone(), &cfg);
                cx.notify()
            });
        }
    }

    pub fn focus_current(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.active {
            Some(i) => self.activate(i, window, cx),
            None => window.focus(&self.focus),
        }
    }

    /// Open files + cursors, in nvim's session format so both apps restore each other's.
    pub(crate) fn save_session(&self, cx: &Context<Self>) {
        if !self.session_loaded {
            return; // never overwrite a session we have not restored yet
        }
        let mut buffers = Vec::new();
        for t in &self.tabs {
            let v = t.read(cx);
            let Some(p) = v.path() else { continue };
            let b = &v.editor.buffer;
            let line = b.line_of(v.editor.head);
            buffers.push(crate::projects::SessionBuffer { file: p.display().to_string(), line: line + 1, col: v.editor.head - b.line_start(line) });
        }
        let current = self.active.and_then(|i| self.tabs.get(i)).and_then(|t| t.read(cx).path().map(|p| p.display().to_string()));
        crate::projects::write_session(&crate::projects::Session { root: self.root.display().to_string(), current, buffers });
    }

    fn restore_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.session_loaded = true;
        let Some(s) = crate::projects::read_session(&self.root) else { return };
        for b in &s.buffers {
            let p = PathBuf::from(&b.file);
            if !p.is_file() {
                continue;
            }
            self.open_file(p, window, cx);
            if let Some(t) = self.active.and_then(|i| self.tabs.get(i)).cloned() {
                t.update(cx, |v, _| {
                    let line = b.line.saturating_sub(1);
                    let start = v.editor.buffer.line_start(line.min(v.editor.buffer.line_count().saturating_sub(1)));
                    let end = v.editor.buffer.line_end(v.editor.buffer.line_of(start));
                    v.editor.set_cursor((start + b.col).min(end), false);
                    v.request_center();
                });
            }
        }
        if let Some(cur) = s.current.map(PathBuf::from) {
            if let Some(i) = self.tabs.iter().position(|t| t.read(cx).path() == Some(&cur)) {
                self.activate(i, window, cx);
            }
        }
    }

    /// Closes a tab without asking (its file was deleted).
    pub(crate) fn discard_tab(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.remove_tab(i, window, cx)
    }

    fn remove_tab(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.tabs[i].update(cx, |v, _| v.detach_lsp());
        self.tabs.remove(i);
        self.save_session(cx);
        self.active = match self.tabs.len() {
            0 => None,
            n => Some(i.min(n - 1)),
        };
        match self.active {
            Some(a) => self.activate(a, window, cx),
            None => {
                window.focus(&self.focus);
                cx.notify();
            }
        }
    }

    fn open_picker(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        let initial = match mode {
            Mode::Text => self
                .active
                .and_then(|i| self.tabs.get(i))
                .map(|t| {
                    let e = &t.read(cx).editor;
                    let s = e.buffer.slice(e.selection());
                    if s.contains('\n') { String::new() } else { s }
                })
                .unwrap_or_default(),
            Mode::Files | Mode::List | Mode::Symbols | Mode::Actions | Mode::Everywhere => String::new(),
        };
        let root = self.root.clone();
        let picker = cx.new(|cx| Picker::new(mode, root, &initial, window, cx));
        self.show_picker(picker, window, cx);
    }

    pub(crate) fn show_picker(&mut self, picker: Entity<Picker>, window: &mut Window, cx: &mut Context<Self>) {
        cx.subscribe_in(&picker, window, |ws, _, ev: &PickerEvent, window, cx| {
            match ev {
                PickerEvent::Open { path, .. } if ws.git_open_commit(path, window, cx) => ws.picker = None,
                // Ctrl+N on a class of a jar: open the class file (read-only) through jdtls
                PickerEvent::Open { path, line, .. } if path.to_string_lossy().starts_with("jdt:") => {
                    ws.picker = None;
                    ws.push_nav(cx);
                    let l = line.unwrap_or(0);
                    let loc = serde_json::json!({ "uri": path.to_string_lossy(), "range": { "start": { "line": l, "character": 0 }, "end": { "line": l, "character": 0 } } });
                    ws.open_location(loc, window, cx);
                }
                // a Local History version: read-only
                PickerEvent::Open { path, .. } if crate::code_tools::version_origin(path).is_some() => {
                    ws.picker = None;
                    ws.open_file(path.clone(), window, cx);
                    if let Some(t) = ws.active.and_then(|i| ws.tabs.get(i)) {
                        t.update(cx, |v, _| v.read_only = true);
                    }
                }
                PickerEvent::Open { path, line, col } => {
                    ws.picker = None;
                    if line.is_some() {
                        ws.push_nav(cx);
                    }
                    ws.open_file(path.clone(), window, cx);
                    if let (Some(line), Some(t)) = (line, ws.active.and_then(|i| ws.tabs.get(i))) {
                        t.update(cx, |v, cx| {
                            v.goto(*line, *col);
                            cx.notify()
                        });
                    }
                }
                PickerEvent::Dismissed => ws.dismiss_picker(window, cx),
                PickerEvent::RunAction(name) => {
                    // back to the editor/tree that had focus, then run it there
                    ws.dismiss_picker(window, cx);
                    match cx.build_action(name, None) {
                        Ok(action) => window.defer(cx, move |window, cx| window.dispatch_action(action, cx)),
                        Err(e) => ws.message = Some(format!("{name}: {e}")),
                    }
                }
                PickerEvent::Rebind { name, keys } => {
                    let shown = keys.iter().map(|k| crate::keymap::pretty(k)).collect::<Vec<_>>().join(", ");
                    ws.message = Some(match crate::keymap::save_override(name, keys.clone()) {
                        Ok(()) => {
                            let errors = crate::keymap::apply(cx);
                            if errors.is_empty() {
                                if keys.is_empty() { format!("Đã bỏ phím tắt của {name}") } else { format!("{name} → {shown}") }
                            } else {
                                errors.join("; ")
                            }
                        }
                        Err(e) => format!("Không ghi được keymap.toml: {e}"),
                    });
                    if let Some(p) = ws.picker.clone() {
                        p.update(cx, |p, cx| p.reload_actions(cx));
                    }
                }
            }
            cx.notify();
        })
        .detach();
        self.picker = Some(picker);
        cx.notify();
    }

    fn dismiss_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.picker = None;
        match self.active {
            Some(i) => self.activate(i, window, cx),
            None => window.focus(&self.focus),
        }
        cx.notify();
    }

    fn click_row(&mut self, row: &Row, window: &mut Window, cx: &mut Context<Self>) {
        match &row.id {
            NodeId::File(p) => self.open_file(p.clone(), window, cx),
            id => {
                self.tree.toggle(id);
                cx.notify();
            }
        }
    }

    fn active_path(&self, cx: &gpui::App) -> Option<PathBuf> {
        self.active.and_then(|i| self.tabs.get(i)).and_then(|t| t.read(cx).path().cloned())
    }

    /// Expands the tree down to `path` and scrolls its row into view (IntelliJ "Select Opened File").
    pub(crate) fn reveal_in_tree(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.tree.reveal(self.project.as_ref(), path);
        self.tree_scroll_to = Some(path.to_path_buf());
        cx.notify();
    }

    fn render_tree(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let active_path = self.active_path(cx);
        let rows = std::sync::Arc::new(self.tree.rows(self.project.as_ref()));
        if let Some(p) = self.tree_scroll_to.take() {
            if let Some(ix) = rows.iter().position(|r| r.id == NodeId::File(p.clone())) {
                self.tree_scroll.scroll_to_item(ix, gpui::ScrollStrategy::Center);
            }
        }
        let header = match &self.model_state {
            ModelState::Loading => "Đang đọc mô hình Maven…".to_string(),
            ModelState::Failed(_) => "Lỗi mô hình Maven (xem thanh trạng thái)".to_string(),
            ModelState::Ready => crate::tree::file_name(&self.root),
        };
        div()
            .flex()
            .flex_col()
            .debug_selector(|| "project-tree".into())
            .w(px(self.sizes.tree))
            .h_full()
            .rounded(px(theme::ISLAND_RADIUS))
            .overflow_hidden()
            .bg(c(theme::BG))
            .border_1()
            .border_color(c(theme::BORDER))
            .child({
                use crate::ui_icons::{Ico, tip};
                // tool window header: title, then the actions as icons (IntelliJ New UI)
                div()
                    .h(px(36.))
                    .flex_none()
                    .pl_3()
                    .pr_1()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_0p5()
                    .child(div().text_color(c(theme::FG)).font_weight(gpui::FontWeight::SEMIBOLD).child("Project"))
                    .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().pl_2().text_color(c(theme::FG_DIM)).child(header))
                    .child(crate::run_panel::tool_btn("reveal-active", Ico::Locate, theme::FG_DIM, false).tooltip(tip("Tới file đang mở (Alt+F1)")).on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                        if let Some(p) = this.active_path(cx) {
                            this.reveal_in_tree(&p, cx);
                        }
                    })))
                    .child(
                        crate::run_panel::tool_btn("toggle-hidden", Ico::Eye, if self.tree.show_hidden { theme::ACCENT } else { theme::FG_DIM }, self.tree.show_hidden)
                            .tooltip(tip(if self.tree.show_hidden { "Ẩn file ẩn (dotfiles, target, .git)" } else { "Hiện file ẩn (dotfiles, target, .git)" }))
                            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                this.tree.show_hidden = !this.tree.show_hidden;
                                this.tree.refresh();
                                cx.notify()
                            })),
                    )
                    .child(crate::run_panel::tool_btn("hide-tree", Ico::Minimize, theme::FG_DIM, false).tooltip(tip("Ẩn cây project (Alt+1)")).on_mouse_down(MouseButton::Left, cx.listener(|_, _, window, cx| {
                        window.dispatch_action(Box::new(crate::shell::ToggleProjectTree), cx)
                    })))
            })
            .child(
                // virtualized: only the rows in view are built (the tree can hold thousands of jars)
                uniform_list("tree", rows.len(), cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                  range.map(|i| {
                    let row = rows[i].clone();
                    let selected = matches!(&row.id, NodeId::File(p) if Some(p) == active_path.as_ref());
                    let row_path = match &row.id {
                        NodeId::File(p) => Some(p.clone()),
                        _ => None,
                    };
                    let arrow: gpui::AnyElement = if !row.expandable {
                        div().into_any_element()
                    } else {
                        // chevron right / down like IntelliJ New UI
                        let ico = if row.expanded { crate::ui_icons::Ico::ChevronDown } else { crate::ui_icons::Ico::ChevronRight };
                        crate::ui_icons::icon(ico, 12., c(theme::FG_DIM)).into_any_element()
                    };
                    let git_color = match &row.id {
                        NodeId::File(p) => this.git.status.get(p).map(|s| crate::git_support::status_color(*s)),
                        _ => None,
                    };
                    let r2 = row.clone();
                    let r3 = row.clone();
                    let sel = row.label.clone();
                    div()
                        .id(("row", i))
                        .debug_selector(move || format!("tree:{sel}"))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_1p5()
                        .h(px(24.))
                        .mx_1p5()
                        .rounded_md()
                        .pl(px(6. + 16. * row.depth as f32))
                        .pr_2()
                        .whitespace_nowrap()
                        .when(selected, |d| d.bg(c(theme::ROW_ACTIVE)))
                        .hover(|s| s.bg(c(theme::ROW_HOVER)))
                        .child(div().w(px(12.)).flex_none().child(arrow))
                        .child(crate::icons::tree_icon(row.icon, row_path.as_deref()))
                        .children({
                            // "name  [independent]": the tag dim and small so the name keeps the space
                            let (name, tag) = match row.label.split_once("  [") {
                                Some((n, t)) => (n.to_string(), Some(format!("[{t}"))),
                                None => (row.label.clone(), None),
                            };
                            let mut parts = vec![div().text_color(c(git_color.unwrap_or(theme::FG))).child(name).into_any_element()];
                            if let Some(t) = tag {
                                parts.push(div().text_size(px(11.)).text_color(c(theme::FG_DIM)).child(t).into_any_element());
                            }
                            parts
                        })
                        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| this.click_row(&r2, window, cx)))
                        .on_mouse_down(MouseButton::Right, cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| this.tree_menu(ev.position, &r3.id, cx)))
                  }).collect::<Vec<_>>()
                }))
                .size_full().min_h_0()
                .track_scroll(self.tree_scroll.clone()),
            )
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::ui_icons::{Ico, icon};
        // IntelliJ New UI editor tabs: no separators, the active one underlined in the accent
        div()
            .id("tabs")
            .flex()
            .flex_row()
            .flex_none()
            .h(px(36.))
            .px_1()
            .overflow_x_scroll()
            // transparent: the tabs are a block of their own (see editor_block)
            .when(!theme::glass_on(), |d| d.border_b_1().border_color(c(theme::BORDER)))
            .children(self.tabs.iter().enumerate().map(|(i, t)| {
                let v = t.read(cx);
                let active = self.active == Some(i);
                let dirty = v.is_dirty();
                div()
                    .id(("tab", i))
                    .relative()
                    .flex()
                    .flex_row()
                    .flex_none()
                    .items_center()
                    .gap_1p5()
                    .h_full()
                    .pl_2p5()
                    .pr_1p5()
                    .text_color(c(if active { theme::FG } else { theme::FG_DIM }))
                    .hover(|s| s.text_color(c(theme::FG)))
                    .children(v.path().map(|p| crate::icons::file_icon(p)))
                    .child(SharedString::from(v.title()))
                    .child(
                        div()
                            .id(("close", i))
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(18.))
                            .rounded_sm()
                            .hover(|s| s.bg(c(theme::ROW_HOVER)))
                            // unsaved: a dot that turns into × under the mouse (IntelliJ)
                            .child(if dirty && !active {
                                div().size(px(7.)).rounded_full().bg(c(theme::MODIFIED)).into_any_element()
                            } else {
                                icon(Ico::Close, 12., c(if dirty { theme::MODIFIED } else { theme::FG_DIM })).into_any_element()
                            })
                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.close_tab(i, window, cx)
                            })),
                    )
                    .when(active, |d| d.child(div().absolute().bottom_0().left(px(6.)).right(px(6.)).h(px(2.)).rounded_full().bg(c(theme::ACCENT))))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| this.activate(i, window, cx)))
                    .on_mouse_down(MouseButton::Right, cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| this.tab_menu(ev.position, i, cx)))
            }))
    }

    fn push_toast(&mut self, text: String, cx: &mut Context<Self>) {
        let low = text.to_lowercase();
        let error = ["lỗi", "error", "không ", "thất bại", "failed", "cannot"].iter().any(|w| low.contains(w));
        // the same text again: just refresh its time
        self.toasts.retain(|(t, _, _)| *t != text);
        self.toasts.push((text, std::time::Instant::now(), error));
        if self.toasts.len() > 4 {
            self.toasts.remove(0);
        }
        let secs = if error { 10 } else { 5 };
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_secs(secs)).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }

    /// Notification balloons, bottom right above the status bar (IntelliJ).
    fn render_toasts(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let now = std::time::Instant::now();
        self.toasts.retain(|(_, at, err)| now.duration_since(*at).as_secs() < if *err { 10 } else { 5 });
        if self.toasts.is_empty() {
            return None;
        }
        let items: Vec<gpui::AnyElement> = self
            .toasts
            .iter()
            .enumerate()
            .map(|(i, (text, _, err))| {
                let t = text.clone();
                div()
                    .id(("toast", i))
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap_2()
                    .max_w(px(460.))
                    .px_3()
                    .py_2()
                    .bg(c(theme::POPUP_BG))
                    .border_1()
                    .border_color(c(if *err { theme::ERROR } else { theme::BORDER }))
                    .rounded_md()
                    .shadow_lg()
                    .child(div().flex_none().text_color(c(if *err { theme::ERROR } else { theme::ACCENT })).child(if *err { "✖" } else { "ℹ" }))
                    .child(div().flex_1().text_color(c(theme::FG)).child(SharedString::from(text.clone())))
                    .child(
                        div()
                            .id(("toast-x", i))
                            .flex_none()
                            .px_1()
                            .text_color(c(theme::FG_DIM))
                            .hover(|s| s.text_color(c(theme::FG)))
                            .child("×")
                            .on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, cx| {
                                ws.toasts.retain(|(x, _, _)| *x != t);
                                cx.stop_propagation();
                                cx.notify()
                            })),
                    )
                    .into_any_element()
            })
            .collect();
        Some(div().absolute().right(px(16.)).bottom(px(34.)).flex().flex_col().items_end().gap_2().occlude().children(items).into_any_element())
    }

    fn render_status(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut left: Vec<String> = Vec::new();
        let mut right: Vec<String> = Vec::new();
        if let Some(t) = self.active.and_then(|i| self.tabs.get(i)) {
            let v = t.read(cx);
            if let Some(p) = v.path() {
                // breadcrumbs: project › module › … › file
                let rel = p.strip_prefix(&self.root).unwrap_or(p);
                let mut crumbs = vec![crate::tree::file_name(&self.root)];
                crumbs.extend(rel.components().map(|c| c.as_os_str().to_string_lossy().to_string()));
                left.push(crumbs.join("  ›  "));
            }
            if let Some(d) = v.diagnostic_at_cursor() {
                left.push(format!("{} {}", if d.severity == 1 { "✖" } else { "⚠" }, d.message.lines().next().unwrap_or("")));
            }
            if let Some(s) = v.vim_status() {
                left.insert(0, s);
            }
            if v.read_only {
                right.push("chỉ đọc".into());
            }
            let (line, col) = v.editor.cursor_line_col();
            right.push(format!("{}:{}", line + 1, col + 1));
            right.push(if v.editor.indent == "\t" { "Tab".into() } else { format!("{} spaces", v.editor.indent.len()) });
            right.push("UTF-8".into());
        }
        if let Some(p) = toolchain::managed::progress() {
            right.insert(0, p);
        }
        let lsp = self.lsp_status();
        if !lsp.is_empty() {
            right.insert(0, lsp);
        }
        match &self.model_state {
            ModelState::Loading => right.insert(0, "Maven: đang resolve…".into()),
            ModelState::Failed(e) => left.push(format!("Maven lỗi: {}", e.lines().next().unwrap_or(""))),
            ModelState::Ready => {
                let n = self.project.as_ref().map_or(0, |p| p.modules.len());
                right.insert(0, format!("{n} module"));
            }
        }
        // messages become balloons (render_toasts), not status bar text
        let tab_msg = self.active.and_then(|i| self.tabs.get(i)).and_then(|t| t.read(cx).message.clone());
        let (ws_msg, seen) = (self.message.clone(), self.toast_seen.clone());
        if ws_msg != seen.0 {
            if let Some(m) = ws_msg.clone() {
                self.push_toast(m, cx);
            }
            self.toast_seen.0 = ws_msg;
        }
        if tab_msg != seen.1 {
            if let Some(m) = tab_msg.clone() {
                self.push_toast(m, cx);
            }
            self.toast_seen.1 = tab_msg;
        }
        div()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .justify_between()
            .gap_4()
            .h(px(26.))
            .px_3()
            .bg(c(theme::FRAME_BG))
            .when(theme::glass_on(), |d| d.bg(c(theme::BG)).border_1().border_color(c(theme::BORDER)).rounded_full().mx(px(theme::ISLAND_GAP)).mb(px(theme::ISLAND_GAP)))
            .text_size(px(12.))
            .text_color(c(theme::FG_DIM))
            // the left part shrinks (…) instead of running into the right one
            .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().flex().gap_4().children(left.into_iter().map(SharedString::from)))
            .child(
                div()
                    .flex_none()
                    .whitespace_nowrap()
                    .flex()
                    .items_center()
                    .gap_4()
                    .children(self.render_progress())
                    .children(right.into_iter().map(SharedString::from))

            )
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // react to a save in any tab: refresh the tree; a pom.xml save re-resolves the model
        let mut reload = false;
        for t in &self.tabs {
            t.update(cx, |v, cx| v.refresh_semantic(cx));
        }
        let mut changed = Vec::new();
        let mut xhtml_saved = Vec::new();
        let mut java_saved = Vec::new();
        let mut saved_any = false;
        let mut saved_paths: Vec<PathBuf> = Vec::new();
        for t in &self.tabs {
            // (path, is_pom, synced with jdtls as an open Java document)
            let saved = t.update(cx, |v, _| {
                let s = v.just_saved;
                v.just_saved = false;
                s.then(|| (v.path().cloned(), v.path().is_some_and(|p| p.ends_with("pom.xml")), v.lsp_uri().is_some()))
            });
            if let Some((path, pom, synced)) = saved {
                saved_any = true;
                if let Some(p) = &path {
                    saved_paths.push(p.clone());
                }
                if let Some(p) = path.as_ref().filter(|p| crate::code_tools::version_origin(p).is_none()) {
                    crate::code_tools::record_version(p, &t.read(cx).editor.buffer.text());
                }
                if path.as_ref().is_some_and(|p| p.extension().is_some_and(|e| e == "xhtml")) {
                    xhtml_saved.push(t.clone());
                }
                if path.as_ref().is_some_and(|p| p.extension().is_some_and(|e| e == "java")) {
                    java_saved.push(t.clone());
                }
                self.tree.refresh();
                reload |= pom;
                // Java files already reported didSave; tell jdtls about the others (xml, yml, xhtml…)
                if let (Some(p), false) = (path, synced) {
                    changed.push((p, crate::lsp_workspace::FileChange::Changed));
                }
            }
        }
        self.notify_files(&changed);
        if saved_any {
            self.refresh_git(cx);
        }
        for p in &saved_paths {
            self.db_file_saved(p, cx);
        }
        if saved_paths.iter().any(|p| *p == crate::vim_mode::vimrc_path()) {
            self.apply_vim(cx);
            self.message = Some("Đã nạp lại vimrc".into());
        }
        for t in java_saved {
            self.jsf_lens(t, cx);
        }
        if !xhtml_saved.is_empty() {
            for t in xhtml_saved {
                self.jsf_check(t, cx);
            }
            // pages changed: bean usage counts too
            for t in self.tabs.clone() {
                self.jsf_lens(t, cx);
            }
        }
        if reload {
            // a saved pom.xml: full reload, which also re-imports the classpath in jdtls
            self.reload_model(cx);
        }
        if !self.session_loaded {
            self.restore_session(window, cx);
        }
        // the debugger stopped somewhere: open that file and mark the line
        if let Some((path, line)) = self.pending_exec.take() {
            self.open_file(path, window, cx);
            if let Some(t) = self.active.and_then(|i| self.tabs.get(i)).cloned() {
                t.update(cx, |v, cx| {
                    v.goto(line, 0);
                    v.exec_line = Some(line);
                    cx.notify()
                });
            }
            // locals may already be here (the tab was just opened)
            self.refresh_inline_values(cx);
        }
        // a run waiting for jdtls starts as soon as it is idle
        if self.pending_run.is_some() && !self.jdtls_busy() {
            self.start_pending_run(window, cx);
        }
        let panel = self.render_run_panel(cx);
        let config_menu = self.render_config_menu(cx);
        let bp_dialog = self.render_bp_dialog(cx);
        let config_editor = self.render_config_editor(cx);
        let name_prompt = self.render_name_prompt(cx);
        let context_menu = self.render_context_menu(cx);
        let confirm = self.render_confirm(cx);
        let replace_files = self.render_replace_in_files(cx);
        if std::mem::take(&mut self.pending_git_window) && !self.log_view.as_ref().is_some_and(|v| v.file.is_none()) {
            self.git_log(None, window, cx);
        }
        if std::mem::take(&mut self.pending_accounts) && self.accounts_dialog.is_none() {
            self.open_git_accounts(window, cx);
        }
        let accounts = self.render_git_accounts(cx);
        let log_view = self.render_log_view(window, cx);
        let db_view = self.render_db_view(window, cx);
        let overlay = self.db_open || self.log_view.is_some() || self.diff_view.is_some() || self.conflict_view.is_some();
        cx.set_global(EscTarget(Some(cx.entity().downgrade())));
        if cx.try_global::<crate::vim_mode::OverlayOpen>().map(|o| o.0) != Some(overlay) {
            cx.set_global(crate::vim_mode::OverlayOpen(overlay));
        }
        let conflict_view = self.render_conflicts(window, cx);
        let diff_view = self.render_diff_view(window, cx);
        // the status bar turns new messages into balloons: build it before them
        let status = self.render_status(cx).into_any_element();
        let toasts = self.render_toasts(cx);
        self.sync_debug_hover_flag(cx);
        let value_popup = self.render_value_popup(cx);
        let eval_dialog = self.render_evaluate(cx);

        let s = self.settings.clone();
        let editor_area = match self.active.and_then(|i| self.tabs.get(i)) {
            Some(t) => div()
                .debug_selector(|| "editor-area".into())
                .flex_1()
                .overflow_hidden()
                .font_family(SharedString::from(s.font_family.clone()))
                .text_size(px(s.font_px()))
                .font_weight(crate::settings::Settings::weight(&s.font_weight))
                .line_height(px((s.font_px() * s.line_height).round()))
                .child(t.clone()),
            // empty area when nothing is open - no "[No Name]" tab
            None => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(c(theme::FG_DIM))
                .child("Chọn file trong cây project để mở"),
        };

        // editor tabs + editor as one island, with the tool panel island below / beside it;
        // transparent: the tabs are a pill of their own above the editor block
        let editor_block = if theme::glass_on() {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(theme::ISLAND_GAP))
                .child(glass_pane(div().flex_none().rounded_full().overflow_hidden().px_2()).child(self.render_tabs(cx)))
                .child(glass_pane(div().flex_1().min_h_0().flex().flex_col().overflow_hidden().rounded(px(theme::ISLAND_RADIUS))).child(editor_area))
        } else {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .overflow_hidden()
                .rounded(px(theme::ISLAND_RADIUS))
                .bg(c(theme::BG))
                .border_1()
                .border_color(c(theme::BORDER))
                .child(self.render_tabs(cx))
                .child(editor_area)
        };
        let middle = match self.settings.panel_side.as_str() {
            "right" => div().flex_1().min_w_0().flex().flex_row().overflow_hidden().child(editor_block).children(panel),
            "left" => div().flex_1().min_w_0().flex().flex_row().overflow_hidden().children(panel).child(editor_block),
            _ => div().flex_1().min_w_0().flex().flex_col().overflow_hidden().child(editor_block).children(panel),
        };
        let tree_left = self.settings.tree_side != "right";
        let stripe = if theme::glass_on() {
            // the tool window buttons: a block of their own, apart from the islands
            glass_pane(div().mx(px(theme::ISLAND_GAP)).flex_none().rounded(px(theme::ISLAND_RADIUS)).overflow_hidden()).child(self.render_stripe(window, cx)).into_any_element()
        } else {
            self.render_stripe(window, cx).into_any_element()
        };
        let mut tree = (self.settings.tree_visible || self.maven_side).then(|| {
            let handle = self.resize_handle(crate::layout::DragKind::Tree, cx);
            let t = if self.maven_side { self.render_maven_panel(cx) } else { self.render_tree(cx).into_any_element() };
            let row = div().flex().flex_row().h_full();
            if tree_left { row.child(t).child(handle) } else { row.child(handle).child(t) }
        });

        div()
            .key_context("Workspace")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                if let Some(i) = this.active {
                    this.close_tab(i, window, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &NextTab, window, cx| {
                if let Some(i) = this.active {
                    this.activate((i + 1) % this.tabs.len(), window, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &PrevTab, window, cx| {
                if let Some(i) = this.active {
                    this.activate((i + this.tabs.len() - 1) % this.tabs.len(), window, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &SelectOpenedFile, window, cx| {
                if !this.settings.tree_visible {
                    window.dispatch_action(Box::new(crate::shell::ToggleProjectTree), cx);
                }
                if let Some(p) = this.active_path(cx) {
                    this.reveal_in_tree(&p, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &FocusTree, window, cx| {
                // Alt+1 on a hidden tree shows it first (IntelliJ)
                if !this.settings.tree_visible {
                    window.dispatch_action(Box::new(crate::shell::ToggleProjectTree), cx);
                }
                window.focus(&this.focus);
                cx.notify()
            }))
            .on_action(cx.listener(|this, _: &ReloadModel, _, cx| this.load_model(true, cx)))
            .on_action(cx.listener(|this, _: &GoToFile, window, cx| this.open_picker(Mode::Files, window, cx)))
            .on_action(cx.listener(|this, _: &crate::actions::FindAction, window, cx| this.open_picker(Mode::Actions, window, cx)))
            .on_action(cx.listener(|this, _: &NavBack, window, cx| this.navigate(true, window, cx)))
            .on_action(cx.listener(|this, _: &NavForward, window, cx| this.navigate(false, window, cx)))
            .on_action(cx.listener(|this, _: &Problems, window, cx| this.show_problems(window, cx)))
            .on_action(cx.listener(|this, _: &GotoSymbol, window, cx| this.goto_symbol(window, cx)))
            .on_action(cx.listener(|this, _: &SpringEndpoints, window, cx| this.show_endpoints(window, cx)))
            .on_action(cx.listener(|this, _: &SpringBeans, window, cx| this.show_beans(window, cx)))
            .on_action(cx.listener(|this, _: &JsfPageMap, window, cx| this.jsf_page_map(window, cx)))
            .on_action(cx.listener(|this, _: &ReplaceInFiles, window, cx| this.open_replace_in_files(window, cx)))
            .on_action(cx.listener(|this, _: &RecentFiles, window, cx| this.show_recent_files(window, cx)))
            .on_action(cx.listener(|this, _: &SearchEverywhere, window, cx| this.search_everywhere(window, cx)))
            .on_action(cx.listener(|this, _: &GitCommit, window, cx| this.open_commit(window, cx)))
            .on_action(cx.listener(|this, _: &ToggleBookmark, _, cx| this.toggle_bookmark(cx)))
            .on_action(cx.listener(|this, _: &AttachJvm, window, cx| this.open_name_prompt(crate::context_menu::PromptKind::AttachJvm, window, cx)))
            .on_action(cx.listener(|this, _: &PauseSession, _, cx| this.pause_session(cx)))
            .on_action(cx.listener(|this, _: &RunWithCoverage, window, cx| {
                // the test file in the editor, else the toolbar's active configuration
                let file = this.active.and_then(|i| this.tabs.get(i)).and_then(|t| t.read(cx).path().cloned());
                this.coverage_next = true;
                match file.filter(|p| std::fs::read_to_string(p).is_ok_and(|t| t.contains("@Test"))) {
                    Some(p) => this.run_file_pub(p, false, window, cx),
                    None => this.run_active(false, window, cx),
                }
            }))
            .on_action(cx.listener(|this, _: &HideCoverage, _, cx| {
                this.coverage.clear();
                this.push_coverage_to_tabs(cx)
            }))
            .on_action(cx.listener(|this, _: &RunHttpRequest, _, cx| {
                let Some(t) = this.active.and_then(|i| this.tabs.get(i)) else { return };
                let v = t.read(cx);
                if v.path().is_some_and(|p| crate::http_client::is_http_file(p)) {
                    let line = v.editor.cursor_line_col().0;
                    this.run_http_request(line, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &ShowBookmarks, window, cx| this.show_bookmarks(window, cx)))
            .on_action(cx.listener(|this, _: &TodoList, window, cx| this.show_todos(window, cx)))
            .on_action(cx.listener(|this, _: &LocalHistory, window, cx| this.show_local_history(window, cx)))
            .on_action(cx.listener(|this, _: &GitPush, _, cx| this.git_push(cx)))
            .on_action(cx.listener(|this, _: &GitPull, _, cx| this.git_pull(cx)))
            .on_action(cx.listener(|this, _: &GitLog, window, cx| this.git_log(None, window, cx)))
            .on_action(cx.listener(|this, _: &crate::db_view::DbToggle, _, cx| this.toggle_db(cx)))
            // the Git button / shortcut opens the Git window (graph, branches, staging, commit)
            .on_action(cx.listener(|this, _: &GitPanelToggle, window, cx| {
                if this.log_view.as_ref().is_some_and(|v| v.file.is_none()) {
                    this.close_log(window, cx)
                } else {
                    this.refresh_git(cx);
                    this.git_log(None, window, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &CloseDiff, _, cx| {
                this.diff_view = None;
                cx.notify()
            }))
            .on_action(cx.listener(|this, _: &GitAnnotate, _, cx| this.git_annotate(cx)))
            .on_action(cx.listener(|this, _: &GitRollbackHunk, _, cx| this.git_rollback_hunk(cx)))
            .on_action(cx.listener(|this, _: &GitFileHistory, window, cx| {
                let p = this.active.and_then(|i| this.tabs.get(i)).and_then(|t| t.read(cx).path().cloned());
                if p.is_some() {
                    this.git_log(p, window, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &GitDiffFile, window, cx| {
                let p = this.active.and_then(|i| this.tabs.get(i)).and_then(|t| t.read(cx).path().cloned());
                this.git_show_diff(p, window, cx)
            }))
            .on_action(cx.listener(|this, _: &VcsPopup, window, cx| {
                let at = gpui::point(window.bounds().size.width / 2. - px(150.), px(120.));
                this.git_branch_menu(at, cx)
            }))
            // Shift pressed twice quickly (alone): Search Everywhere, like IntelliJ
            .on_modifiers_changed(cx.listener(|this, ev: &gpui::ModifiersChangedEvent, window, cx| {
                let m = ev.modifiers;
                let only_shift = m.shift && !m.control && !m.alt && !m.platform;
                if only_shift && !this.shift_down {
                    let now = std::time::Instant::now();
                    if this.last_shift.is_some_and(|t| now.duration_since(t) < std::time::Duration::from_millis(400)) {
                        this.last_shift = None;
                        this.search_everywhere(window, cx);
                    } else {
                        this.last_shift = Some(now);
                    }
                } else if !m.shift && (m.control || m.alt || m.platform) {
                    this.last_shift = None;
                }
                this.shift_down = m.shift;
            }))
            .on_action(cx.listener(|this, _: &GotoBean, window, cx| this.goto_bean(window, cx)))
            .on_action(cx.listener(|this, _: &RunActive, window, cx| this.run_active(false, window, cx)))
            .on_action(cx.listener(|this, _: &DebugActive, window, cx| this.run_active(true, window, cx)))
            .on_action(cx.listener(|this, _: &StopSession, _, cx| this.stop_session(None, cx)))
            .on_action(cx.listener(|this, _: &Resume, _, cx| this.debug_command("continue", cx)))
            .on_action(cx.listener(|this, _: &StepOver, _, cx| this.debug_command("next", cx)))
            .on_action(cx.listener(|this, _: &StepInto, _, cx| this.debug_command("stepIn", cx)))
            .on_action(cx.listener(|this, _: &StepOut, _, cx| this.debug_command("stepOut", cx)))
            .on_action(cx.listener(|this, _: &HotSwap, _, cx| this.hot_swap(cx)))
            .on_action(cx.listener(|this, _: &ToggleTerminal, window, cx| this.toggle_terminal(window, cx)))
            .on_action(cx.listener(|this, _: &EditConfigurations, window, cx| this.open_config_editor(window, cx)))
            .on_action(cx.listener(|this, _: &EvaluateExpression, window, cx| this.open_evaluate(window, cx)))
            .on_action(cx.listener(|this, _: &TogglePanel, _, cx| {
                this.panel_open = !this.panel_open;
                cx.notify()
            }))
            .on_action(cx.listener(|this, _: &FindInFiles, window, cx| this.open_picker(Mode::Text, window, cx)))
            .relative()
            .on_mouse_move(cx.listener(|ws, ev: &gpui::MouseMoveEvent, _, cx| ws.on_drag_move(ev, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.end_drag(cx)))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.end_drag(cx)))
            .size_full()
            .flex()
            .flex_col()
            .bg(c(theme::FRAME_BG))
            .text_color(c(theme::FG))
            .font_family(SharedString::from(s.ui_font_family.clone()))
            .text_size(px(s.ui_font_px()))
            .font_weight(crate::settings::Settings::weight(&s.ui_font_weight))
            .child(
                // islands on the frame: stripe | tree | editor (+ panel) | tree | stripe
                div()
                    .when(theme::glass_on(), |d| d.py(px(theme::ISLAND_GAP)))
                    .flex_1()
                    .flex()
                    .flex_row()
                    .overflow_hidden()
                    .children(if tree_left {
                        vec![stripe, tree.take().map(|t| t.into_any_element()).unwrap_or_else(|| div().into_any_element()), middle.into_any_element(), div().w(px(theme::ISLAND_GAP)).flex_none().into_any_element()]
                    } else {
                        vec![div().w(px(theme::ISLAND_GAP)).flex_none().into_any_element(), middle.into_any_element(), tree.take().map(|t| t.into_any_element()).unwrap_or_else(|| div().into_any_element()), stripe]
                    }),
            )
            .child(status)
            .children(config_menu)
            .children(bp_dialog)
            .children(config_editor)
            .children(replace_files)
            .children(accounts)
            // log < conflicts < diff (opened from both) < prompts, menus and confirms on top
            .children(db_view)
            .children(log_view)
            .children(conflict_view)
            .children(diff_view)
            .children(self.new_module.clone())
            .children(name_prompt)
            .children(context_menu)
            .children(confirm)
            .children(toasts)
            .children(value_popup)
            .children(eval_dialog)
            .when_some(self.picker.clone(), |d, picker| {
                d.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .flex()
                        .flex_col()
                        .items_center()
                        .pt(px(70.))
                        .bg(gpui::rgba(0x00000055))
                        .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.dismiss_picker(window, cx)))
                        .child(div().on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()).child(picker)),
                )
            })
    }
}

/// A block in transparent mode: no fill (BG is clear), only the faint outline.
fn glass_pane(d: gpui::Div) -> gpui::Div {
    d.bg(c(theme::BG)).border_1().border_color(c(theme::BORDER))
}

/// The workspace that owns the git / database windows.
#[derive(Default)]
struct EscTarget(Option<gpui::WeakEntity<Workspace>>);
impl gpui::Global for EscTarget {}

/// Esc closes the top git / database window wherever the focus is (an editor or terminal below
/// would otherwise take it). Installed before the vim and terminal interceptors.
pub fn install_esc(cx: &mut gpui::App) {
    cx.intercept_keystrokes(|ev: &gpui::KeystrokeEvent, window, cx| {
        let k = &ev.keystroke;
        if k.key != "escape" || k.modifiers.modified() || !cx.try_global::<crate::vim_mode::OverlayOpen>().is_some_and(|o| o.0) {
            return;
        }
        let Some(ws) = cx.try_global::<EscTarget>().and_then(|t| t.0.clone()).and_then(|w| w.upgrade()) else { return };
        if ws.update(cx, |ws, cx| ws.esc_overlay(window, cx)) {
            cx.stop_propagation();
        }
    })
    .detach();
}
