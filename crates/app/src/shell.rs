//! The window: one tab per project (what wezterm.lua did), a project picker for new tabs,
//! and app-level quit. Each project is a `Workspace`; jdtls starts only for projects that
//! have been shown, so restoring several projects doesn't start several JVMs at once.
use crate::input::{InputEvent, TextInput};
use crate::picker::{Confirm, Dismiss, SelectNext, SelectPrev};
use crate::projects;
use crate::settings::Settings;
use crate::theme::{self, c};
use crate::workspace::Workspace;
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, KeyBinding, MouseButton, SharedString, Window, actions, div, prelude::*, px, uniform_list,
};
use std::path::PathBuf;

actions!(shell, [NewProjectTab, CloseProjectTab, NextProject, PrevProject, QuitApp, OpenSettings,
    ToggleProjectTree, MoveProjectTree, PanelBottom, PanelRight, PanelLeft]);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-shift-t", NewProjectTab, None),
        KeyBinding::new("ctrl-shift-w", CloseProjectTab, None),
        KeyBinding::new("ctrl-shift-]", NextProject, None),
        KeyBinding::new("ctrl-shift-[", PrevProject, None),
        KeyBinding::new("ctrl-shift-q", QuitApp, None),
        KeyBinding::new("ctrl-alt-s", OpenSettings, None),
        KeyBinding::new("alt-shift-1", ToggleProjectTree, None),
    ]);
}

pub struct Shell {
    settings: Settings,
    settings_error: Option<String>,
    projects: Vec<(PathBuf, Entity<Workspace>)>,
    active: Option<usize>,
    pub(crate) chooser: Option<Chooser>,
    /// the "Clone từ Git" dialog
    pub(crate) clone: Option<crate::clone_panel::CloneForm>,
    /// the "Dự án Spring Boot mới" dialog
    new_project: Option<Entity<crate::new_project::NewProjectDialog>>,
    focus: FocusHandle,
    pub(crate) settings_editor: Option<crate::settings_dialog::SettingsEditor>,
}

pub(crate) struct Chooser {
    input: Entity<TextInput>,
    all: Vec<PathBuf>,
    shown: Vec<PathBuf>,
    selected: usize,
    scroll: gpui::UniformListScrollHandle,
    /// the folder picker failed (no portal)
    error: Option<String>,
}

impl Shell {
    pub fn new(settings: Settings, settings_error: Option<String>, open: Vec<PathBuf>, focus_on: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        crate::theme::set_theme(&settings.theme);
        crate::theme::set_glass(settings.glass);
        crate::theme::set_editor_theme(&settings.editor_theme);
        settings.apply_toolchain();
        // JDK/Maven downloads run on background threads: repaint the status bar while one is going
        cx.spawn(async move |this, cx| {
            let mut last = None;
            loop {
                cx.background_executor().timer(std::time::Duration::from_millis(700)).await;
                let now = toolchain::managed::progress();
                if now != last {
                    last = now;
                    if this.update(cx, |_, cx| cx.refresh_windows()).is_err() {
                        break;
                    }
                }
            }
        })
        .detach();
        let mut s = Self { settings, settings_error, projects: Vec::new(), active: None, chooser: None, clone: None, new_project: None, focus: cx.focus_handle(), settings_editor: None };
        for p in open {
            s.add_project(p, false, window, cx);
        }
        match focus_on.and_then(|f| s.projects.iter().position(|(p, _)| *p == f)) {
            Some(i) => s.activate(i, window, cx),
            None if !s.projects.is_empty() => s.activate(0, window, cx),
            None => s.open_chooser(window, cx),
        }
        // screenshot checks: CHODILAMIDE_OPEN=settings opens a dialog at start
        match std::env::var("CHODILAMIDE_OPEN").as_deref() {
            Ok(which @ ("settings" | "settings-java" | "settings-keys")) => {
                let page = match which {
                    "settings-java" => 5,
                    "settings-keys" => 4,
                    _ => 0,
                };
                cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(std::time::Duration::from_millis(1200)).await;
                    let _ = this.update_in(cx, |s, window, cx| {
                        s.open_settings(window, cx);
                        s.debug_settings_page(page);
                    });
                })
                .detach();
            }
            Ok("configs") => {
                if let Some(ws) = s.active.and_then(|i| s.projects.get(i)).map(|(_, w)| w.clone()) {
                    ws.update(cx, |w, cx| w.open_config_editor(window, cx));
                }
            }
            Ok(which @ ("maven" | "maven-deps")) => {
                // the model loads asynchronously: open once it is there
                let deps = which == "maven-deps";
                if let Some(ws) = s.active.and_then(|i| s.projects.get(i)).map(|(_, w)| w.clone()) {
                    cx.spawn(async move |_, cx| {
                        cx.background_executor().timer(std::time::Duration::from_millis(1500)).await;
                        let _ = ws.update(cx, |w, cx| w.debug_open_maven(deps, cx));
                    })
                    .detach();
                }
            }
            Ok("clone") => {
                cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(std::time::Duration::from_millis(1200)).await;
                    let _ = this.update_in(cx, |s, window, cx| {
                        s.open_chooser(window, cx);
                        s.open_clone(window, cx);
                        // screenshot checks: CHODILAMIDE_CLONE_URL prefills the address
                        if let (Ok(u), Some(f)) = (std::env::var("CHODILAMIDE_CLONE_URL"), s.clone.as_ref()) {
                            f.set_url(&u, cx);
                        }
                    });
                })
                .detach();
            }
            Ok("new-project") => {
                cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(std::time::Duration::from_millis(1200)).await;
                    let _ = this.update_in(cx, |s, window, cx| {
                        s.open_chooser(window, cx);
                        s.open_new_project(window, cx);
                    });
                })
                .detach();
            }
            Ok("new-module") => {
                cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(std::time::Duration::from_millis(2500)).await;
                    let _ = this.update_in(cx, |s, window, cx| {
                        let Some(ws) = s.active.and_then(|i| s.projects.get(i)).map(|(_, w)| w.clone()) else { return };
                        ws.update(cx, |w, cx| w.open_new_module(window, cx));
                    });
                })
                .detach();
            }
            Ok("chooser") => {
                // after the projects restored at start have settled
                cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(std::time::Duration::from_millis(1200)).await;
                    let _ = this.update_in(cx, |s, window, cx| s.open_chooser(window, cx));
                })
                .detach();
            }
            // screenshot checks of the Git window: on the working tree / on the newest commit
            Ok(which @ ("git" | "git-commit")) => {
                let commit = which == "git-commit";
                cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(std::time::Duration::from_millis(2500)).await;
                    let _ = this.update_in(cx, |s, window, cx| {
                        let Some(ws) = s.active.and_then(|i| s.projects.get(i)).map(|(_, w)| w.clone()) else { return };
                        ws.update(cx, |w, cx| {
                            w.refresh_git(cx);
                            w.git_log(None, window, cx);
                        });
                        if commit {
                            cx.spawn_in(window, async move |_, cx| {
                                cx.background_executor().timer(std::time::Duration::from_millis(2500)).await;
                                let _ = ws.update(cx, |w, cx| w.log_goto(0, cx));
                            })
                            .detach();
                        }
                    });
                })
                .detach();
            }
            // Database window (Alt+7), or its connection dialog
            Ok(which @ ("db" | "db-dialog")) => {
                let dialog = which == "db-dialog";
                cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(std::time::Duration::from_millis(1500)).await;
                    let _ = this.update_in(cx, |s, window, cx| {
                        let Some(ws) = s.active.and_then(|i| s.projects.get(i)).map(|(_, w)| w.clone()) else { return };
                        ws.update(cx, |w, cx| {
                            w.toggle_db(cx);
                            if dialog {
                                w.debug_open_db_dialog(window, cx);
                            }
                        });
                    });
                })
                .detach();
            }
            Ok("settings-font") => {
                s.open_settings(window, cx);
                s.debug_open_font_select(window, cx);
            }
            _ => {}
        }
        s
    }

    /// Root of the project in the active tab.
    pub(crate) fn active_root(&self, cx: &gpui::App) -> Option<String> {
        self.active.and_then(|i| self.projects.get(i)).map(|(_, w)| w.read(cx).root_str())
    }

    /// Layout actions (tree/panel position): change the settings, save, apply everywhere.
    fn change_layout(&mut self, f: impl FnOnce(&mut Settings), cx: &mut Context<Self>) {
        let mut s = self.settings.clone();
        f(&mut s);
        if let Err(e) = s.save() {
            self.set_message(&format!("Không ghi được settings.toml: {e}"), cx);
        }
        self.apply_settings(s, cx);
    }

    pub(crate) fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Applies new settings everywhere (theme is global, the rest per project).
    pub(crate) fn apply_settings(&mut self, settings: Settings, cx: &mut Context<Self>) {
        crate::theme::set_theme(&settings.theme);
        crate::theme::set_glass(settings.glass);
        crate::theme::set_editor_theme(&settings.editor_theme);
        settings.apply_toolchain();
        self.settings = settings.clone();
        for (_, ws) in &self.projects {
            ws.update(cx, |w, cx| w.apply_settings(settings.clone(), cx));
        }
        cx.refresh_windows();
    }

    pub(crate) fn active_workspace(&self) -> Option<gpui::Entity<crate::workspace::Workspace>> {
        self.active.and_then(|i| self.projects.get(i)).map(|(_, w)| w.clone())
    }

    pub(crate) fn refocus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ws) = self.active.and_then(|i| self.projects.get(i)).map(|(_, w)| w.clone()) {
            ws.update(cx, |w, cx| w.focus_current(window, cx));
        }
    }

    pub(crate) fn set_message(&mut self, msg: &str, cx: &mut Context<Self>) {
        if let Some(ws) = self.active.and_then(|i| self.projects.get(i)).map(|(_, w)| w.clone()) {
            ws.update(cx, |w, cx| {
                w.message = Some(msg.to_string());
                cx.notify()
            });
        }
    }

    /// The system folder picker (xdg-desktop-portal on Linux); the chosen folder opens as a
    /// project like a typed path.
    fn chooser_error(&mut self, e: String, cx: &mut Context<Self>) {
        if let Some(ch) = self.chooser.as_mut() {
            ch.error = Some(e);
        }
        cx.notify()
    }

    fn browse_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Mở project".into()) });
        cx.spawn_in(window, async move |this, cx| {
            let res = rx.await;
            let _ = this.update_in(cx, |shell, window, cx| match res {
                Ok(Ok(Some(paths))) => {
                    if let Some(p) = paths.into_iter().next() {
                        shell.add_project(p, true, window, cx);
                    }
                }
                Ok(Ok(None)) => {} // cancelled
                Ok(Err(e)) => shell.chooser_error(format!("Không mở được hộp chọn thư mục: {e} - gõ đường dẫn vào ô rồi Enter"), cx),
                Err(_) => {}
            });
        })
        .detach();
    }

    pub(crate) fn open_new_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use crate::new_project::{NewMode, NewProjectDialog, NewProjectEvent};
        let d = cx.new(|cx| NewProjectDialog::new(NewMode::Project, &crate::clone_panel::default_parent(), cx));
        cx.subscribe_in(&d, window, |shell, _, ev: &NewProjectEvent, window, cx| {
            shell.new_project = None;
            if let NewProjectEvent::Created { dir, .. } = ev {
                shell.chooser = None;
                shell.add_project(dir.clone(), true, window, cx);
            }
            cx.notify();
        })
        .detach();
        let focus = d.read(cx).first_focus(cx);
        crate::dialogs::focus_later(focus, window, cx);
        self.new_project = Some(d);
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn has_project(&self, root: &std::path::Path) -> bool {
        self.projects.iter().any(|(p, _)| p == root)
    }

    pub(crate) fn add_project(&mut self, root: PathBuf, activate: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(i) = self.projects.iter().position(|(p, _)| *p == root) {
            if activate {
                self.activate(i, window, cx);
            }
            return;
        }
        let settings = self.settings.clone();
        let err = self.settings_error.take();
        let ws = cx.new(|cx| Workspace::new(root.clone(), settings, err, cx));
        cx.observe(&ws, |_, _, cx| cx.notify()).detach();
        self.projects.push((root.clone(), ws));
        projects::touch_recent(&root);
        self.save_open();
        if activate {
            self.activate(self.projects.len() - 1, window, cx);
        }
    }

    fn activate(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, ws)) = self.projects.get(i).cloned() else { return };
        if let Some(prev) = self.active.and_then(|a| self.projects.get(a)).map(|(_, w)| w.clone()) {
            if prev != ws {
                prev.update(cx, |w, cx| w.set_visible(false, cx));
            }
        }
        self.active = Some(i);
        self.chooser = None;
        ws.update(cx, |w, cx| w.set_visible(true, cx));
        let title = format!("{} - ChoDiLamIDE", projects::title(&self.projects[i].0));
        window.set_window_title(&title);
        ws.update(cx, |w, cx| w.focus_current(window, cx));
        cx.notify();
    }

    fn save_open(&self) {
        let list: Vec<PathBuf> = self.projects.iter().map(|(p, _)| p.clone()).collect();
        projects::write_open(&list);
    }

    fn close_project(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, ws)) = self.projects.get(i).cloned() else { return };
        let answer = ws.update(cx, |w, cx| w.confirm_close(window, cx));
        cx.spawn_in(window, async move |this, cx| {
            if !answer.await {
                return;
            }
            let _ = this.update_in(cx, |shell, window, cx| {
                let Some(i) = shell.projects.iter().position(|(_, w)| *w == ws) else { return };
                let (_, ws) = shell.projects.remove(i);
                let shutdown = ws.update(cx, |w, cx| w.shutdown(cx));
                cx.background_executor().spawn(shutdown).detach();
                shell.save_open();
                shell.active = None;
                if shell.projects.is_empty() {
                    shell.open_chooser(window, cx);
                } else {
                    shell.activate(i.min(shell.projects.len() - 1), window, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Quit: every project asks about its unsaved files first.
    pub fn quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let all: Vec<Entity<Workspace>> = self.projects.iter().map(|(_, w)| w.clone()).collect();
        let answers: Vec<_> = all.iter().map(|w| w.update(cx, |w, cx| w.confirm_close(window, cx))).collect();
        cx.spawn_in(window, async move |_, cx| {
            for a in answers {
                if !a.await {
                    return;
                }
            }
            let _ = cx.update(|_, cx| cx.quit());
        })
        .detach();
    }

    // ---- project chooser ------------------------------------------------------

    fn open_chooser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| TextInput::new("Tên project, hoặc đường dẫn thư mục (~/...)", cx));
        cx.subscribe(&input, |shell, _, ev: &InputEvent, cx| {
            let InputEvent::Changed(q) = ev;
            shell.filter_chooser(q, cx);
        })
        .detach();
        crate::dialogs::focus_later(input.read(cx).focus_handle(cx), window, cx);
        // open ones first, then recent, then everything else by name (like wezterm.lua)
        let open: Vec<PathBuf> = self.projects.iter().map(|(p, _)| p.clone()).collect();
        let recent = projects::read_recent();
        let mut rest = projects::find_projects();
        rest.sort_by_key(|p| projects::title(p).to_lowercase());
        let mut all: Vec<PathBuf> = Vec::new();
        for p in open.iter().chain(recent.iter()).chain(rest.iter()) {
            if !all.contains(p) && p.is_dir() {
                all.push(p.clone());
            }
        }
        self.chooser = Some(Chooser { input, shown: all.clone(), all, selected: 0, scroll: gpui::UniformListScrollHandle::new(), error: None });
        cx.notify();
    }

    fn filter_chooser(&mut self, q: &str, cx: &mut Context<Self>) {
        let Some(ch) = self.chooser.as_mut() else { return };
        let q = q.trim().to_lowercase();
        ch.shown = ch.all.iter().filter(|p| q.is_empty() || projects::short(p).to_lowercase().contains(&q)).cloned().collect();
        ch.selected = 0;
        cx.notify();
    }

    fn choose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ch) = self.chooser.as_ref() else { return };
        let typed = ch.input.read(cx).content.trim().to_string();
        let target = match ch.shown.get(ch.selected) {
            Some(p) => Some(p.clone()),
            // "Mở thư mục khác…": a typed path
            None if !typed.is_empty() => {
                let p = if let Some(rest) = typed.strip_prefix("~/") {
                    PathBuf::from(std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).unwrap_or_default()).join(rest)
                } else {
                    PathBuf::from(&typed)
                };
                p.is_dir().then(|| std::path::absolute(&p).unwrap_or(p))
            }
            None => None,
        };
        match target {
            Some(p) => self.add_project(p, true, window, cx),
            None => {
                if let Some(ch) = self.chooser.as_mut() {
                    ch.input.update(cx, |i, cx| i.set_text(&typed, cx));
                }
            }
        }
    }

    fn dismiss_chooser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.projects.is_empty() {
            return; // nothing to go back to
        }
        self.chooser = None;
        if let Some(i) = self.active {
            self.activate(i, window, cx);
        }
        cx.notify();
    }

    fn render_chooser(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(ch) = self.chooser.as_ref() else { return div() };
        let open: Vec<PathBuf> = self.projects.iter().map(|(p, _)| p.clone()).collect();
        let recent = projects::read_recent();
        // virtualized: every project found is listed, only the rows in view are built
        let shown = std::sync::Arc::new(ch.shown.clone());
        let n = shown.len();
        let scroll = ch.scroll.clone();
        let rows = uniform_list("proj-list-uni", n, cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
            let selected = this.chooser.as_ref().map(|c| c.selected).unwrap_or(0);
            range
                .map(|i| {
                    let p = &shown[i];
                    let mark = if open.contains(p) { "●" } else if recent.contains(p) { "◷" } else { " " };
                    let p2 = p.clone();
                    div()
                        .id(("proj", i))
                        .flex()
                        .items_center()
                        .gap_3()
                        .h(px(26.))
                        .px_3()
                        .mx_1()
                        .rounded_md()
                        .when(i == selected, |d| d.bg(c(theme::ROW_ACTIVE)))
                        .hover(|s| s.bg(c(theme::ROW_HOVER)))
                        .child(div().w(px(12.)).text_color(c(theme::RUN_GREEN)).child(mark))
                        .child(div().w(px(260.)).text_color(c(theme::FG)).child(projects::title(p)))
                        .child(div().text_color(c(theme::FG_DIM)).child(projects::short(p)))
                        .on_mouse_down(MouseButton::Left, cx.listener(move |shell, _, window, cx| shell.add_project(p2.clone(), true, window, cx)))
                })
                .collect::<Vec<_>>()
        }))
        .track_scroll(scroll);
        let empty = ch.shown.is_empty();
        div()
            .key_context("Picker")
            .on_action(cx.listener(|s, _: &SelectNext, _, cx| {
                if let Some(ch) = s.chooser.as_mut() {
                    ch.selected = (ch.selected + 1).min(ch.shown.len().saturating_sub(1));
                    ch.scroll.scroll_to_item(ch.selected, gpui::ScrollStrategy::Top);
                }
                cx.notify()
            }))
            .on_action(cx.listener(|s, _: &SelectPrev, _, cx| {
                if let Some(ch) = s.chooser.as_mut() {
                    ch.selected = ch.selected.saturating_sub(1);
                    ch.scroll.scroll_to_item(ch.selected, gpui::ScrollStrategy::Top);
                }
                cx.notify()
            }))
            .on_action(cx.listener(|s, _: &Confirm, window, cx| s.choose(window, cx)))
            .on_action(cx.listener(|s, _: &Dismiss, window, cx| s.dismiss_chooser(window, cx)))
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .pt(px(60.))
            .bg(c(theme::BG))
            .child(
                div()
                    .w(px(820.))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().text_color(c(theme::FG)).text_size(px(18.)).child("Chọn project"))
                    .child(div().text_color(c(theme::FG_DIM)).child("● đang mở   ◷ gần đây   ·  gõ đường dẫn rồi Enter để mở thư mục khác  ·  Esc quay lại"))
                    .child(
                        // the input with a "browse" button at its right (system folder picker)
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(div().flex_1().min_w_0().child(ch.input.clone()))
                            .child(
                                div()
                                    .id("browse-folder")
                                    .flex()
                                    .flex_row()
                                    .flex_none()
                                    .items_center()
                                    .gap_1p5()
                                    .h(px(30.))
                                    .px_3()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(c(theme::BORDER))
                                    .text_color(c(theme::FG))
                                    .hover(|s| s.bg(c(theme::ROW_HOVER)))
                                    .tooltip(crate::ui_icons::tip("Mở hộp chọn thư mục của hệ thống"))
                                    .child(crate::ui_icons::icon(crate::ui_icons::Ico::Folder, 15., c(theme::FG_DIM)))
                                    .child("Chọn thư mục…")
                                    .on_mouse_down(MouseButton::Left, cx.listener(|shell, _, window, cx| shell.browse_folder(window, cx))),
                            )
                            .child(
                                div()
                                    .id("clone-from-git")
                                    .flex()
                                    .flex_row()
                                    .flex_none()
                                    .items_center()
                                    .gap_1p5()
                                    .h(px(30.))
                                    .px_3()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(c(theme::ACCENT))
                                    .text_color(c(theme::ACCENT))
                                    .hover(|s| s.bg(c(theme::ROW_HOVER)))
                                    .tooltip(crate::ui_icons::tip("Kéo project mới về từ Git (tự hướng dẫn tài khoản / SSH key nếu máy chưa có)"))
                                    .child(crate::ui_icons::icon(crate::ui_icons::Ico::Pull, 15., c(theme::ACCENT)))
                                    .child("Clone từ Git…")
                                    .on_mouse_down(MouseButton::Left, cx.listener(|shell, _, window, cx| shell.open_clone(window, cx))),
                            )
                            .child(
                                div()
                                    .id("new-spring-project")
                                    .flex()
                                    .flex_row()
                                    .flex_none()
                                    .items_center()
                                    .gap_1p5()
                                    .h(px(30.))
                                    .px_3()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(c(theme::BORDER))
                                    .text_color(c(theme::FG))
                                    .hover(|s| s.bg(c(theme::ROW_HOVER)))
                                    .tooltip(crate::ui_icons::tip("Tạo project Maven + Spring Boot mới (một hoặc nhiều module)"))
                                    .child(crate::ui_icons::icon(crate::ui_icons::Ico::Plus, 15., c(theme::ACCENT)))
                                    .child("Dự án mới…")
                                    .on_mouse_down(MouseButton::Left, cx.listener(|shell, _, window, cx| shell.open_new_project(window, cx))),
                            ),
                    )
                    .when_some(ch.error.clone(), |d, e| d.child(div().px_1().text_color(c(theme::ERROR)).child(SharedString::from(e))))
                    .child(div().id("proj-list").h(px((n as f32 * 26.).min(560.))).child(rows.size_full()))
                    .when(empty, |d| d.child(div().px_3().text_color(c(theme::FG_DIM)).child("Không khớp project nào - Enter để mở như đường dẫn"))),
            )
    }

    /// IntelliJ New UI main toolbar, one row: the open projects as project widgets (colored
    /// initials + name), the active project's VCS widget, then its run widget, search and
    /// settings on the right.
    fn render_project_tabs(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::ui_icons::{Ico, icon, tip};
        let names: Vec<String> = self.projects.iter().map(|(p, _)| projects::title(p)).collect();
        let tabs: Vec<_> = names.into_iter().enumerate().map(|(i, name)| {
            let active = self.active == Some(i) && self.chooser.is_none();
            div()
                .id(("ptab", i))
                .flex()
                .flex_row()
                .items_center()
                .gap_1p5()
                .h(px(28.))
                .pl_1()
                .pr_1p5()
                .rounded_md()
                .when(active, |d| d.bg(c(theme::ROW_HOVER)).text_color(c(theme::FG)))
                .when(!active, |d| d.text_color(c(theme::FG_DIM)))
                .hover(|s| s.bg(c(theme::ROW_HOVER)).text_color(c(theme::FG)))
                .child(project_badge(&name, 20.))
                .child(SharedString::from(name))
                .child(
                    div()
                        .id(("pclose", i))
                        .rounded_sm()
                        .hover(|s| s.bg(c(theme::ROW_ACTIVE)))
                        .child(icon(Ico::Close, 12., c(theme::FG_DIM)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |shell, _, window, cx| {
                                cx.stop_propagation();
                                shell.close_project(i, window, cx)
                            }),
                        ),
                )
                .on_mouse_down(MouseButton::Left, cx.listener(move |shell, _, window, cx| shell.activate(i, window, cx)))
        }).collect();
        let ws = if self.chooser.is_none() { self.active.and_then(|i| self.projects.get(i)).map(|(_, w)| w.clone()) } else { None };
        // row 1: the open projects (scrolls sideways when many), + and settings
        let project_row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .h(px(36.))
            .px_2()
            .bg(c(theme::FRAME_BG))
            .when(theme::glass_on(), |d| d.bg(c(theme::BG)).border_1().border_color(c(theme::BORDER)).rounded_full().mx(px(theme::ISLAND_GAP)).mt(px(theme::ISLAND_GAP)))
            .child(div().id("project-tabs").flex().flex_row().items_center().gap_1().min_w_0().overflow_x_scroll().children(tabs))
            .child(
                crate::run_panel::tool_btn("new-project", Ico::Plus, theme::FG_DIM, false)
                    .tooltip(tip("Mở project khác (Ctrl+Shift+T)"))
                    .on_mouse_down(MouseButton::Left, cx.listener(|shell, _, window, cx| shell.open_chooser(window, cx))),
            )
            .child(div().flex_1())
            .child(
                crate::run_panel::tool_btn("open-settings", Ico::Gear, theme::FG_DIM, false)
                    .tooltip(tip("Cài đặt (Ctrl+Alt+S)"))
                    .on_mouse_down(MouseButton::Left, cx.listener(|shell, _, window, cx| shell.open_settings(window, cx))),
            );
        // row 2: the active project's main toolbar (VCS widget | run widget, search)
        let toolbar = ws.map(|ws| {
            let (left, right) = ws.update(cx, |w, cx| (w.toolbar_left(cx), w.toolbar_right(cx)));
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .h(px(36.))
                .px_2()
                .bg(c(theme::FRAME_BG))
                .border_t_1()
                .border_color(c(theme::BORDER))
                .when(theme::glass_on(), |d| d.bg(c(theme::BG)).border_1().border_color(c(theme::BORDER)).rounded_full().mx(px(theme::ISLAND_GAP)).mt(px(theme::ISLAND_GAP)))
                .child(left)
                .child(div().flex_1())
                .child(right)
        });
        div().flex().flex_col().flex_none().child(project_row).children(toolbar)
    }
}

/// IntelliJ's project icon: the project's initials on a color picked from its name.
pub(crate) fn project_badge(name: &str, size: f32) -> impl IntoElement {
    const COLORS: [u32; 8] = [0x3574f0, 0x5fb865, 0xe08a3c, 0xc77dbb, 0x4fb5c4, 0xd1675a, 0xb3ae60, 0x8c6fd6];
    let initials: String = name.split(['-', '_', ' ', '.']).filter_map(|w| w.chars().next()).take(2).collect::<String>().to_uppercase();
    let hash = name.bytes().fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
    div()
        .flex()
        .items_center()
        .justify_center()
        .flex_none()
        .size(px(size))
        .rounded(px(size * 0.25))
        .bg(gpui::rgb(COLORS[hash as usize % COLORS.len()]))
        .text_color(gpui::white())
        .text_size(px(size * 0.5))
        .font_weight(gpui::FontWeight::BOLD)
        .child(SharedString::from(initials))
}

impl Focusable for Shell {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Shell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (font, size) = (self.settings.ui_font_family.clone(), self.settings.ui_font_px());
        let settings_dialog = self.render_settings(cx);
        let clone_dialog = self.render_clone(cx);
        let new_project_dialog = self.new_project.clone();
        let current = self.active.and_then(|i| self.projects.get(i)).map(|(_, ws)| ws.clone());
        let body: gpui::AnyElement = match (self.chooser.is_some(), current) {
            (false, Some(ws)) => ws.into_any_element(),
            _ => self.render_chooser(cx).into_any_element(),
        };
        div()
            .key_context("Shell")
            .track_focus(&self.focus)
            .on_action(cx.listener(|shell, _: &NewProjectTab, window, cx| shell.open_chooser(window, cx)))
            .on_action(cx.listener(|shell, _: &CloseProjectTab, window, cx| {
                if let Some(i) = shell.active {
                    shell.close_project(i, window, cx)
                }
            }))
            .on_action(cx.listener(|shell, _: &NextProject, window, cx| {
                if let Some(i) = shell.active {
                    shell.activate((i + 1) % shell.projects.len(), window, cx)
                }
            }))
            .on_action(cx.listener(|shell, _: &PrevProject, window, cx| {
                if let Some(i) = shell.active {
                    let n = shell.projects.len();
                    shell.activate((i + n - 1) % n, window, cx)
                }
            }))
            .on_action(cx.listener(|shell, _: &QuitApp, window, cx| shell.quit(window, cx)))
            .on_action(cx.listener(|shell, _: &OpenSettings, window, cx| shell.open_settings(window, cx)))
            .on_action(cx.listener(|shell, _: &ToggleProjectTree, _, cx| shell.change_layout(|s| s.tree_visible = !s.tree_visible, cx)))
            .on_action(cx.listener(|shell, _: &MoveProjectTree, _, cx| {
                shell.change_layout(|s| s.tree_side = if s.tree_side == "right" { "left".into() } else { "right".into() }, cx)
            }))
            .on_action(cx.listener(|shell, _: &PanelBottom, _, cx| shell.change_layout(|s| s.panel_side = "bottom".into(), cx)))
            .on_action(cx.listener(|shell, _: &PanelRight, _, cx| shell.change_layout(|s| s.panel_side = "right".into(), cx)))
            .on_action(cx.listener(|shell, _: &PanelLeft, _, cx| shell.change_layout(|s| s.panel_side = "left".into(), cx)))
            .size_full()
            .flex()
            .flex_col()
            .bg(c(theme::FRAME_BG))
            .text_color(c(theme::FG))
            .font_family(SharedString::from(font))
            .text_size(px(size))
            .relative()
            .child(self.render_project_tabs(cx))
            .child(div().flex_1().overflow_hidden().child(body))
            .children(settings_dialog)
            .children(clone_dialog)
            .children(new_project_dialog)
    }
}
