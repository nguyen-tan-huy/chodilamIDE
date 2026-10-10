//! Bottom tool window (Run / Debug / Maven) and the run-config toolbar.
use crate::debug::{Kind, State, stack_link};
use crate::theme::{self, c};
use crate::workspace::Workspace;
use gpui::{AnyElement, Context, Entity, Focusable, MouseButton, SharedString, Window, anchored, deferred, div, prelude::*, px, uniform_list};
use std::path::PathBuf;

impl Workspace {
    /// Main toolbar, left part (IntelliJ New UI's VCS widget): branch popup, changed count,
    /// update / commit / push. The shell puts it next to the project widgets.
    pub(crate) fn toolbar_left(&mut self, cx: &mut Context<Self>) -> AnyElement {
        use crate::ui_icons::{Ico, icon, tip};
        let Some(b) = self.git.branch.clone() else { return div().into_any_element() };
        let changed = self.git.status.len();
        let conflicts = self.git.tree.changes.iter().any(|c| c.conflicted());
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_0p5()
            .child(
                div()
                    .id("git-branch")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1p5()
                    .h(px(28.))
                    .px_2()
                    .rounded_md()
                    .text_color(c(theme::FG))
                    .hover(|s| s.bg(c(theme::ROW_HOVER)))
                    .tooltip(tip("Nhánh và lệnh Git (Alt+`)"))
                    .child(icon(Ico::Branch, 15., c(if conflicts { theme::ERROR } else { theme::FG_DIM })))
                    .child(SharedString::from(b))
                    .when(changed > 0, |d| {
                        d.child(div().px_1p5().rounded_full().bg(c(theme::ROW_ACTIVE)).text_size(px(11.)).text_color(c(theme::FG)).child(SharedString::from(changed.to_string())))
                    })
                    .child(icon(Ico::ChevronDown, 11., c(theme::FG_DIM)))
                    .on_mouse_down(MouseButton::Left, cx.listener(|ws, ev: &gpui::MouseDownEvent, _, cx| {
                        let at = gpui::point(ev.position.x - px(10.), ev.position.y + px(18.));
                        ws.git_branch_menu(at, cx)
                    })),
            )
            .child(tool_btn("git-pull", Ico::Pull, theme::GIT_MODIFIED, false).tooltip(tip("Update project - Pull (Ctrl+T)")).on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.git_pull(cx))))
            .child(tool_btn("git-commit", Ico::Commit, theme::GIT_ADDED, false).tooltip(tip("Commit… (Ctrl+K)")).on_mouse_down(MouseButton::Left, cx.listener(|ws, _, window, cx| ws.open_commit(window, cx))))
            .child(tool_btn("git-push", Ico::Push, theme::GIT_MODIFIED, false).tooltip(tip("Push (Ctrl+Shift+K)")).on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.git_push(cx))))
            .into_any_element()
    }

    /// Main toolbar, right part (IntelliJ's run widget): configuration selector, Run, Debug,
    /// Stop, Search Everywhere.
    pub(crate) fn toolbar_right(&mut self, cx: &mut Context<Self>) -> AnyElement {
        use crate::ui_icons::{Ico, icon, tip};
        let label = match self.active_target() {
            Some(crate::debug::RunTarget::Config(c)) => c.name,
            Some(crate::debug::RunTarget::Test(p)) => p.name,
            None => "Chưa có cấu hình".into(),
        };
        let is_test = matches!(self.active_target(), Some(crate::debug::RunTarget::Test(_)));
        let live = self.sessions.iter().any(|s| s.is_live());
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_0p5()
            .child(
                div()
                    .id("config-select")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1p5()
                    .h(px(28.))
                    .px_2()
                    .rounded_md()
                    .hover(|s| s.bg(c(theme::ROW_HOVER)))
                    .tooltip(tip("Cấu hình chạy (Alt+Shift+F10 để sửa)"))
                    .child(icon(if is_test { Ico::Problems } else { Ico::Run }, 12., c(if is_test { theme::WARNING } else { theme::RUN_GREEN })))
                    .child(SharedString::from(label))
                    .child(icon(Ico::ChevronDown, 11., c(theme::FG_DIM)))
                    .on_mouse_down(MouseButton::Left, cx.listener(|ws, ev: &gpui::MouseDownEvent, _, cx| {
                        ws.config_menu_at = gpui::point(ev.position.x - px(40.), ev.position.y + px(18.));
                        ws.config_menu_open = !ws.config_menu_open;
                        cx.notify()
                    })),
            )
            .child(tool_btn("run", Ico::Play, theme::RUN_GREEN, false).tooltip(tip("Run (Shift+F10)")).on_mouse_down(MouseButton::Left, cx.listener(|ws, _, window, cx| ws.run_active(false, window, cx))))
            .child(tool_btn("debug", Ico::Bug, theme::RUN_GREEN, false).tooltip(tip("Debug (Shift+F9)")).on_mouse_down(MouseButton::Left, cx.listener(|ws, _, window, cx| ws.run_active(true, window, cx))))
            .when(live, |d| {
                d.child(tool_btn("stop", Ico::Stop, theme::ERROR, false).tooltip(tip("Stop (Ctrl+F2)")).on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.stop_session(None, cx))))
            })
            .child(div().w(px(8.)))
            .child(tool_btn("search-all", Ico::Search, theme::FG_DIM, false).tooltip(tip("Tìm mọi nơi (Shift Shift)")).on_mouse_down(MouseButton::Left, cx.listener(|_, _, window, cx| {
                window.dispatch_action(Box::new(crate::actions::SearchEverywhere), cx)
            })))
            .into_any_element()
    }

    /// The tool window stripe (IntelliJ New UI): icon buttons for the tool windows, the top
    /// group beside the tree, the bottom group for the panel; on the tree's side.
    pub(crate) fn render_stripe(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::ui_icons::{Ico, tip};
        let _ = window;
        let open = self.panel_open;
        let run_view = open && !self.git_tab && !self.terminal_tab && !self.maven_tab;
        let conflicts = self.git.tree.changes.iter().any(|c| c.conflicted());
        let live = self.sessions.iter().any(|s| s.is_live());
        let btn = |id: &'static str, ico: Ico, active: bool, color: u32| tool_btn(id, ico, if active { theme::FG } else { color }, active).size(px(32.));
        div()
            .flex()
            .flex_col()
            .flex_none()
            .items_center()
            .w(px(40.))
            .h_full()
            .py_1()
            .gap_1()
            .child(btn("tw-project", Ico::Folder, self.settings.tree_visible && !self.maven_side, theme::FG_DIM).tooltip(tip("Project (Alt+1)")).on_mouse_down(MouseButton::Left, cx.listener(|ws, _, window, cx| {
                if ws.maven_side {
                    // the Maven window sits in the tree's place: this brings the tree back
                    ws.maven_side = false;
                    cx.notify()
                } else {
                    window.dispatch_action(Box::new(crate::shell::ToggleProjectTree), cx)
                }
            })))
            .child(btn("tw-git", Ico::Branch, self.log_view.as_ref().is_some_and(|v| v.file.is_none()), if conflicts { theme::ERROR } else { theme::FG_DIM }).tooltip(tip("Commit / Git (Alt+9)")).on_mouse_down(MouseButton::Left, cx.listener(|ws, _, window, cx| {
                // opens the Git window (graph, branches, staging, commit); again closes it
                if ws.log_view.as_ref().is_some_and(|v| v.file.is_none()) {
                    ws.close_log(window, cx)
                } else {
                    ws.refresh_git(cx);
                    ws.git_log(None, window, cx)
                }
            })))
            .child(btn("tw-maven", Ico::Maven, self.maven_side, theme::FG_DIM).tooltip(tip("Maven")).on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| {
                // takes the place of the project tree; again = back to the tree
                ws.maven_side = !ws.maven_side;
                cx.notify()
            })))
            .child(btn("tw-db", Ico::Database, self.db_open, theme::FG_DIM).tooltip(tip("Database - MongoDB (Alt+7)")).on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.toggle_db(cx))))
            .child(div().flex_1())
            .child(btn("tw-run", if self.active_session().is_some_and(|s| s.kind == Kind::Debug) { Ico::Bug } else { Ico::Play }, run_view, if live { theme::RUN_GREEN } else { theme::FG_DIM }).tooltip(tip("Run / Debug (Alt+4 / Alt+5)")).on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| {
                if ws.panel_open && !ws.git_tab && !ws.terminal_tab && !ws.maven_tab {
                    ws.panel_open = false;
                } else {
                    ws.panel_open = true;
                    ws.maven_tab = false;
                    ws.terminal_tab = false;
                    ws.git_tab = false;
                }
                cx.notify()
            })))
            .child(btn("tw-terminal", Ico::Terminal, open && self.terminal_tab, theme::FG_DIM).tooltip(tip("Terminal (Alt+F12)")).on_mouse_down(MouseButton::Left, cx.listener(|ws, _, window, cx| {
                ws.git_tab = false;
                if ws.panel_open && ws.terminal_tab {
                    ws.panel_open = false;
                    cx.notify()
                } else {
                    ws.panel_open = false;
                    ws.toggle_terminal(window, cx)
                }
            })))
            .child(btn("tw-problems", Ico::Problems, false, theme::FG_DIM).tooltip(tip("Problems (Alt+6)")).on_mouse_down(MouseButton::Left, cx.listener(|ws, _, window, cx| ws.show_problems(window, cx))))
    }

    pub(crate) fn run_active(&mut self, debug: bool, window: &mut Window, cx: &mut Context<Self>) {
        match self.active_target() {
            Some(crate::debug::RunTarget::Config(cfg)) => self.launch(cfg, debug, window, cx),
            // a unit test selected on the toolbar runs / debugs as a test, not as a main class
            Some(crate::debug::RunTarget::Test(p)) => self.run_test_profile(p, debug, cx),
            None => self.message = Some("Chưa có cấu hình chạy: bấm ▶ cạnh hàm main hoặc @Test để tạo".into()),
        }
        cx.notify();
    }

    pub(crate) fn render_config_menu(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.config_menu_open {
            return None;
        }
        let root = self.root_str();
        let active = runner::config::active(&root);
        let rows: Vec<AnyElement> = runner::config::load(&root)
            .into_iter()
            .enumerate()
            .map(|(i, cfg)| {
                let name = cfg.name.clone();
                let selected = active.as_deref() == Some(name.as_str());
                div()
                    .id(("cfg", i))
                    .flex()
                    .gap_3()
                    .px_2()
                    .whitespace_nowrap()
                    .mx_1()
                    .rounded_md()
                    .when(selected, |d| d.bg(c(theme::ROW_ACTIVE)))
                    .hover(|s| s.bg(c(theme::ROW_HOVER)))
                    .child(div().text_color(c(theme::FG)).child(cfg.name.clone()))
                    .child(div().text_color(c(theme::FG_DIM)).child(cfg.main_class.clone()))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, cx| {
                        let _ = runner::config::set_active(&ws.root_str(), &name);
                        ws.config_menu_open = false;
                        cx.notify()
                    }))
                    .into_any_element()
            })
            .collect();
        // unit-test profiles (from ▶ on @Test, or created in nvim)
        let tests: Vec<AnyElement> = runner::test_profiles::load(&root)
            .into_iter()
            .enumerate()
            .map(|(i, p)| {
                let name = p.name.clone();
                let selected = active.as_deref() == Some(name.as_str());
                div()
                    .id(("tp", i))
                    .flex()
                    .gap_3()
                    .px_2()
                    .whitespace_nowrap()
                    .mx_1()
                    .rounded_md()
                    .when(selected, |d| d.bg(c(theme::ROW_ACTIVE)))
                    .hover(|s| s.bg(c(theme::ROW_HOVER)))
                    .child(div().text_color(c(theme::RUN_GREEN)).child("🧪"))
                    .child(div().text_color(c(theme::FG)).child(p.name.clone()))
                    .child(div().text_color(c(theme::FG_DIM)).child(crate::tree::file_name(std::path::Path::new(&p.file))))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, cx| {
                        let _ = runner::config::set_active(&ws.root_str(), &name);
                        ws.config_menu_open = false;
                        cx.notify()
                    }))
                    .into_any_element()
            })
            .collect();
        let has_tests = !tests.is_empty();
        let empty = rows.is_empty() && !has_tests;
        Some(
            deferred(
                anchored().snap_to_window().position(self.config_menu_at).child(
                    div()
                        .min_w(px(360.))
                        .py_1()
                        .bg(c(theme::POPUP_BG))
                        .border_1()
                        .border_color(c(theme::BORDER))
                        .shadow_lg().occlude().rounded_lg()
                        .children(rows)
                        .when(has_tests, |d| d.child(div().mt_1().px_2().border_t_1().border_color(c(theme::BORDER)).text_size(px(11.)).text_color(c(theme::FG_DIM)).child("Unit test")))
                        .children(tests)
                        .when(empty, |d| d.child(div().px_2().text_color(c(theme::FG_DIM)).child("Chưa có cấu hình - bấm ▶ cạnh main")))
                        .child(
                            div()
                                .id("cfg-edit")
                                .px_2()
                                .mt_1()
                                .border_t_1()
                                .border_color(c(theme::BORDER))
                                .text_color(c(theme::ACCENT))
                                .hover(|s| s.bg(c(theme::ROW_HOVER)))
                                .child("✎ Sửa cấu hình…   Alt+Shift+F10")
                                .on_mouse_down(MouseButton::Left, cx.listener(|ws, _, window, cx| ws.open_config_editor(window, cx))),
                        ),
                ),
            )
            .into_any_element(),
        )
    }

    /// Alt+F12: show the terminal (creating one in the project root), or go back to the editor.
    pub(crate) fn toggle_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focused = self.terminals.get(self.active_terminal).is_some_and(|t| t.read(cx).focus_handle(cx).is_focused(window));
        if self.panel_open && self.terminal_tab && focused {
            self.panel_open = false;
            if let Some(i) = self.active {
                self.activate(i, window, cx);
            }
            cx.notify();
            return;
        }
        if self.terminals.is_empty() {
            self.new_terminal(window, cx);
        }
        self.panel_open = true;
        self.terminal_tab = true;
        self.maven_tab = false;
        if let Some(t) = self.terminals.get(self.active_terminal) {
            window.focus(&t.read(cx).focus_handle(cx));
        }
        cx.notify();
    }

    pub(crate) fn new_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        self.new_terminal_in(root, window, cx)
    }

    /// A terminal started in `dir` ("Mở terminal ở đây"), shown in the Terminal tab.
    pub(crate) fn new_terminal_in(&mut self, dir: std::path::PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.panel_open = true;
        self.terminal_tab = true;
        self.maven_tab = false;
        // the PTY can fail to start; the view itself cannot
        let pty = match crate::terminal::Pty::spawn(&dir) {
            Ok(p) => p,
            Err(e) => {
                self.message = Some(format!("Không mở được terminal: {e}"));
                return;
            }
        };
        let t = cx.new(|cx| crate::terminal::TerminalView::new(pty, cx));
        cx.observe(&t, |_, _, cx| cx.notify()).detach();
        self.terminals.push(t.clone());
        self.active_terminal = self.terminals.len() - 1;
        window.focus(&t.read(cx).focus_handle(cx));
        cx.notify();
    }

    pub(crate) fn render_run_panel(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.panel_open {
            return None;
        }
        let tabs: Vec<AnyElement> = self
            .sessions
            .iter()
            .map(|s| {
                let id = s.id;
                let active = self.active_session == Some(id) && !self.maven_tab && !self.terminal_tab && !self.git_tab;
                let (icon, color) = match (&s.state, s.kind) {
                    (State::Building, _) => ("⟳", theme::FG_DIM),
                    (State::Stopped { .. }, _) => ("⏸", theme::WARNING),
                    (State::Running, Kind::Debug) => ("🐞", theme::RUN_GREEN),
                    (State::Exited(_), Kind::Test) if s.test_results.iter().any(|r| matches!(r.status, runner::test::Status::Failed | runner::test::Status::Error)) => ("✖", theme::ERROR),
                    (State::Exited(_), Kind::Test) if !s.test_results.is_empty() => ("✓", theme::RUN_GREEN),
                    (State::Running, _) => ("▶", theme::RUN_GREEN),
                    (State::Exited(Some(0)), _) => ("✓", theme::FG_DIM),
                    (State::Exited(_), _) => ("■", theme::FG_DIM),
                };
                div()
                    .id(("session", id))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .h(px(26.))
                    .px_2()
                    .rounded_md()
                    .text_color(c(if active { theme::FG } else { theme::FG_DIM }))
                    .when(active, |d| d.bg(c(theme::ROW_HOVER)))
                    .hover(|s| s.bg(c(theme::ROW_HOVER)))
                    .child(div().text_color(c(color)).child(icon))
                    .child(s.name.clone())
                    .when(!s.is_live(), |d| {
                        d.child(
                            div().id(("close-session", id)).text_color(c(theme::FG_DIM)).child("×").on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, cx| {
                                cx.stop_propagation();
                                ws.sessions.retain(|s| s.id != id);
                                if ws.active_session == Some(id) {
                                    ws.active_session = ws.sessions.last().map(|s| s.id);
                                }
                                cx.notify()
                            })),
                        )
                    })
                    .on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, cx| {
                        ws.active_session = Some(id);
                        ws.maven_tab = false;
                        ws.terminal_tab = false;
                        ws.git_tab = false;
                        cx.notify()
                    }))
                    .into_any_element()
            })
            .collect();

        // IntelliJ New UI tool window header: the tool's name, its tabs, its actions; the
        // tools themselves are switched from the stripe
        let title = if self.git_tab {
            if self.git.tree.changes.iter().any(|c| c.conflicted()) { "Git ⚠" } else { "Git" }
        } else if self.terminal_tab {
            "Terminal"
        } else if self.maven_tab {
            "Maven"
        } else if self.active_session().is_some_and(|s| s.kind == Kind::Debug) {
            "Debug"
        } else {
            "Run"
        };
        let run_view = !self.git_tab && !self.terminal_tab && !self.maven_tab;
        let header = div()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap_1()
            .h(px(36.))
            .pl_3()
            .pr_1()
            .child(div().pr_2().text_color(c(theme::FG)).font_weight(gpui::FontWeight::SEMIBOLD).child(title))
            .when(run_view, |d| d.children(tabs))
            .child(div().flex_1())
            .when(run_view, |d| d.child(self.render_debug_buttons(cx)))
            .child(
                tool_btn("close-panel", crate::ui_icons::Ico::Minimize, theme::FG_DIM, false).tooltip(crate::ui_icons::tip("Ẩn (Shift+Esc)")).on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| {
                    ws.panel_open = false;
                    cx.notify()
                })),
            );

        let body: AnyElement = if self.git_tab {
            self.render_git_panel(cx)
        } else if self.terminal_tab {
            self.render_terminals(cx)
        } else if self.maven_tab {
            self.render_maven_panel(cx)
        } else {
            match self.active_session() {
                None => div().p_2().text_color(c(theme::FG_DIM)).child("Chưa có phiên chạy nào (Shift+F10 chạy, Shift+F9 debug)").into_any_element(),
                Some(s) if s.debugging && matches!(s.state, State::Stopped { .. }) => div()
                    .flex()
                    .flex_row()
                    .size_full()
                    .child(div().w(px(self.sizes.frames)).flex_none().h_full().overflow_hidden().child(self.render_frames(cx)))
                    .child(self.resize_handle(crate::layout::DragKind::Frames, cx).border_l_1().border_color(c(theme::BORDER)))
                    .child(div().w(px(self.sizes.vars)).flex_none().h_full().overflow_hidden().child(self.render_variables(cx)))
                    .child(self.resize_handle(crate::layout::DragKind::Vars, cx).border_l_1().border_color(c(theme::BORDER)))
                    .child(div().flex_1().min_w_0().h_full().overflow_hidden().child(self.render_console(cx)))
                    .into_any_element(),
                Some(s) if s.kind == Kind::Test => div()
                    .flex()
                    .flex_row()
                    .size_full()
                    .child(div().w(px(self.sizes.test_tree)).flex_none().h_full().overflow_hidden().child(self.render_test_tree(cx)))
                    .child(self.resize_handle(crate::layout::DragKind::TestTree, cx).border_l_1().border_color(c(theme::BORDER)))
                    .child(div().flex_1().min_w_0().h_full().overflow_hidden().child(self.render_console(cx)))
                    .into_any_element(),
                Some(_) => self.render_console(cx),
            }
        };

        let content = div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .h_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(px(theme::ISLAND_RADIUS))
            .bg(c(theme::BG))
            .border_1()
            .border_color(c(theme::BORDER))
            .child(header)
            .child(div().flex_1().min_h_0().overflow_hidden().child(body));
        Some(match self.settings.panel_side.as_str() {
            // beside the editor: a column with the drag handle on the editor's side
            "right" => div()
                .flex()
                .flex_row()
                .flex_none()
                .w(px(self.sizes.panel_w))
                .h_full()
                .child(self.resize_handle(crate::layout::DragKind::PanelSide, cx))
                .child(content)
                .into_any_element(),
            "left" => div()
                .flex()
                .flex_row()
                .flex_none()
                .w(px(self.sizes.panel_w))
                .h_full()
                .child(content)
                .child(self.resize_handle(crate::layout::DragKind::PanelSide, cx))
                .into_any_element(),
            _ => div()
                .flex()
                .flex_col()
                .h(px(self.sizes.panel))
                .child(self.resize_handle(crate::layout::DragKind::Panel, cx))
                .child(content)
                .into_any_element(),
        })
    }

    fn render_terminals(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let tabs = self.terminals.iter().enumerate().map(|(i, t)| {
            let v = t.read(cx);
            let name = if v.title.is_empty() { format!("shell {}", i + 1) } else { v.title.clone() };
            let exited = v.exited.is_some();
            div()
                .id(("term", i))
                .flex()
                .gap_1()
                .px_2()
                .when(i == self.active_terminal, |d| d.bg(c(theme::ROW_ACTIVE)))
                .text_color(c(if exited { theme::FG_DIM } else { theme::FG }))
                .child(name)
                .child(div().id(("term-close", i)).text_color(c(theme::FG_DIM)).child("×").on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, cx| {
                    cx.stop_propagation();
                    if i < ws.terminals.len() {
                        ws.terminals.remove(i); // dropping the view shuts the shell down
                        ws.active_terminal = ws.active_terminal.min(ws.terminals.len().saturating_sub(1));
                    }
                    cx.notify()
                })))
                .on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, window, cx| {
                    ws.active_terminal = i;
                    if let Some(t) = ws.terminals.get(i) {
                        window.focus(&t.read(cx).focus_handle(cx));
                    }
                    cx.notify()
                }))
        });
        let body = self.terminals.get(self.active_terminal).cloned();
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .text_size(px(11.))
                    .border_b_1()
                    .border_color(c(theme::BORDER))
                    .children(tabs)
                    .child(div().id("term-new").px_2().text_color(c(theme::FG_DIM)).hover(|s| s.text_color(c(theme::FG))).child("+").on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|ws, _, window, cx| ws.new_terminal(window, cx)),
                    )),
            )
            .child(div().flex_1().overflow_hidden().children(body))
            .into_any_element()
    }

    fn render_debug_buttons(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let stopped = self.active_session().is_some_and(|s| matches!(s.state, State::Stopped { .. }));
        let live = self.active_session().is_some_and(|s| s.is_live());
        let debugging = self.sessions.iter().any(|s| s.kind == Kind::Debug && s.is_live());
        let btn = |id: &'static str, text: &'static str, tip: &'static str, enabled: bool| {
            div()
                .id(id)
                .px_2()
                .text_color(c(if enabled { theme::FG } else { theme::GUTTER_FG }))
                .when(enabled, |d| d.hover(|s| s.bg(c(theme::ROW_HOVER))))
                .child(format!("{text} {tip}"))
        };
        div()
            .flex()
            .flex_row()
            .text_size(px(11.))
            .child(btn("resume", "▶", "F9", stopped).on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.debug_command("continue", cx))))
            .child(btn("pause", "⏸", "", live && !stopped).on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.pause_session(cx))))
            .child(btn("over", "↷", "F8", stopped).on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.debug_command("next", cx))))
            .child(btn("into", "↓", "F7", stopped).on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.debug_command("stepIn", cx))))
            .child(btn("out", "↑", "⇧F8", stopped).on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.debug_command("stepOut", cx))))
            .child(btn("swap", "⟳", "Ctrl+F9", debugging).on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.hot_swap(cx))))
            .child(btn("stop-s", "■", "Ctrl+F2", live).on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.stop_session(None, cx))))
    }

    fn render_test_tree(&mut self, cx: &mut Context<Self>) -> AnyElement {
        use runner::test::Status;
        let Some(s) = self.active_session() else { return div().into_any_element() };
        let id = s.id;
        let live = s.is_live();
        let results = s.test_results.clone();
        let failed = results.iter().filter(|r| matches!(r.status, Status::Failed | Status::Error)).count();
        let passed = results.iter().filter(|r| r.status == Status::Passed).count();
        let summary = if live {
            "Đang chạy test…".to_string()
        } else if results.is_empty() {
            "Không có kết quả - xem console (lỗi build hoặc class không có test?)".to_string()
        } else {
            format!("✓ {passed}   ✖ {failed}   tổng {}", results.len())
        };
        // rows of equal height (a failure's message goes on the same line) so the list can be virtualized
        enum TestRow {
            Class(String),
            Test(usize),
        }
        let mut items: Vec<TestRow> = Vec::new();
        let mut last_class = String::new();
        for (i, r) in results.iter().enumerate() {
            if r.class != last_class {
                last_class = r.class.clone();
                items.push(TestRow::Class(r.class.rsplit('.').next().unwrap_or(&r.class).to_string()));
            }
            items.push(TestRow::Test(i));
        }
        let results = std::sync::Arc::new(results);
        let items = std::sync::Arc::new(items);
        let rows = uniform_list("tests-list", items.len(), cx.processor(move |_, range: std::ops::Range<usize>, _, cx| {
            range
                .map(|k| match &items[k] {
                    TestRow::Class(name) => div().h(px(22.)).flex().items_center().px_2().text_color(c(theme::ACCENT)).child(name.clone()).into_any_element(),
                    TestRow::Test(i) => {
                        let i = *i;
                        let r = &results[i];
                        let (icon, color) = match r.status {
                            Status::Passed => ("✓", theme::RUN_GREEN),
                            Status::Failed | Status::Error => ("✖", theme::ERROR),
                            Status::Skipped => ("⊘", theme::FG_DIM),
                        };
                        let class = r.class.clone();
                        let trace = r.trace.clone();
                        div()
                            .id(("test", i))
                            .h(px(22.))
                            .flex()
                            .items_center()
                            .gap_2()
                            .pl(px(20.))
                            .pr_2()
                            .whitespace_nowrap()
                            .hover(|st| st.bg(c(theme::ROW_HOVER)))
                            .child(div().text_color(c(color)).child(icon))
                            .child(r.name.clone())
                            .child(div().text_color(c(theme::FG_DIM)).child(format!("{:.2}s", r.seconds)))
                            .when(!r.message.is_empty(), |d| d.child(div().flex_1().min_w_0().truncate().text_color(c(theme::ERROR)).child(r.message.lines().next().unwrap_or("").to_string())))
                            // click: jump to the failing line inside the test class
                            .on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, window, cx| {
                                let short = format!("{}.", class);
                                if let Some(line) = trace.lines().find(|l| l.trim_start().starts_with("at ") && l.contains(&short)) {
                                    ws.open_stack_link(line, window, cx);
                                }
                            }))
                            .into_any_element()
                    }
                })
                .collect::<Vec<_>>()
        }));
        div()
            .id("tests")
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .justify_between()
                    .px_2()
                    .child(div().text_color(c(if failed > 0 { theme::ERROR } else { theme::FG })).child(summary))
                    .when(failed > 0 && !live, |d| {
                        d.child(
                            div()
                                .id("rerun-failed")
                                .px_2()
                                .border_1()
                                .border_color(c(theme::BORDER))
                                .rounded_sm()
                                .hover(|st| st.bg(c(theme::ROW_HOVER)))
                                .child("↻ Chạy lại test lỗi")
                                .on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, cx| ws.rerun_failed(id, cx))),
                        )
                    }),
            )
            .child(div().flex_1().min_h_0().child(rows.size_full()))
            .into_any_element()
    }

    fn render_frames(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(s) = self.active_session() else { return div().into_any_element() };
        let id = s.id;
        let (reason, cur_thread) = match &s.state {
            State::Stopped { reason, thread } => (reason.clone(), *thread),
            _ => (String::new(), 0),
        };
        // threads: the stopped one highlighted, click another to see its stack
        let threads: Vec<AnyElement> = s
            .threads
            .iter()
            .enumerate()
            .map(|(i, (tid, name))| {
                let tid = *tid;
                div()
                    .id(("thread", i))
                    .px_2()
                    .whitespace_nowrap()
                    .text_color(c(if tid == cur_thread { theme::FG } else { theme::FG_DIM }))
                    .when(tid == cur_thread, |d| d.bg(c(theme::ROW_ACTIVE)))
                    .hover(|st| st.bg(c(theme::ROW_HOVER)))
                    .child(format!("🧵 {name}"))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, cx| ws.select_thread(id, tid, cx)))
                    .into_any_element()
            })
            .collect();
        // virtualized: stacks of Spring apps are deep; only the rows in view are built
        let frames: std::sync::Arc<Vec<(String, String, bool)>> = std::sync::Arc::new(
            s.frames
                .iter()
                .map(|f| (f.name.clone(), f.path.as_ref().and_then(|p| p.file_name()).map(|n| format!("{}:{}", n.to_string_lossy(), f.line)).unwrap_or_default(), f.path.is_some()))
                .collect(),
        );
        let n_threads = threads.len();
        let n = frames.len();
        let list = uniform_list("frames-list", n, cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
            let selected = this.active_session().map(|s| s.selected_frame).unwrap_or(0);
            range
                .map(|i| {
                    let (name, file, has_path) = frames[i].clone();
                    div()
                        .id(("frame", i))
                        .h(px(22.))
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .whitespace_nowrap()
                        .when(i == selected, |d| d.bg(c(theme::ROW_ACTIVE)))
                        .hover(|st| st.bg(c(theme::ROW_HOVER)))
                        .child(div().text_color(c(if has_path { theme::FG } else { theme::FG_DIM })).child(name))
                        .child(div().text_color(c(theme::FG_DIM)).child(file))
                        .on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, cx| ws.select_frame(id, Some(i), cx)))
                })
                .collect::<Vec<_>>()
        }));
        div()
            .id("frames")
            .size_full()
            .flex()
            .flex_col()
            .child(div().px_2().text_color(c(theme::WARNING)).child(format!("Dừng: {reason}")))
            .child(div().flex_1().min_h_0().child(list.size_full()))
            .when(n_threads > 1, |d| {
                d.child(div().mt_2().px_2().border_t_1().border_color(c(theme::BORDER)).text_color(c(theme::FG_DIM)).child(format!("Threads ({n_threads})")))
                    .child(div().id("threads").max_h(px(120.)).overflow_y_scroll().children(threads))
            })
            .into_any_element()
    }

    fn render_variables(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(s) = self.active_session() else { return div().id("vars") };
        let id = s.id;
        // flatten the expanded tree
        let mut rows: Vec<(usize, crate::debug::Var)> = Vec::new();
        fn walk(s: &crate::debug::Session, r: i64, depth: usize, out: &mut Vec<(usize, crate::debug::Var)>) {
            for v in s.vars.get(&r).map(|v| v.as_slice()).unwrap_or_default() {
                out.push((depth, v.clone()));
                if v.reference > 0 && s.expanded.contains(&v.reference) && out.len() < 2000 {
                    walk(s, v.reference, depth + 1, out);
                }
            }
        }
        if let Some(r) = s.locals_ref {
            walk(s, r, 0, &mut rows);
        }
        let expanded = s.expanded.clone();
        let watches = self.render_watches(cx);
        // virtualized: an expanded collection can flatten into thousands of rows
        let rows = std::sync::Arc::new(rows);
        let n = rows.len();
        let list = uniform_list("vars-list", n, cx.processor(move |_, range: std::ops::Range<usize>, _, cx| {
            range
                .map(|i| {
                    let (depth, v) = &rows[i];
                    let reference = v.reference;
                    let arrow = if reference == 0 { " " } else if expanded.contains(&reference) { "▾" } else { "▸" };
                    div()
                        .id(("var", i))
                        .h(px(20.))
                        .flex()
                        .items_center()
                        .gap_1()
                        .pl(px(6. + 14. * *depth as f32))
                        .whitespace_nowrap()
                        .hover(|st| st.bg(c(theme::ROW_HOVER)))
                        .child(div().w(px(10.)).text_color(c(theme::FG_DIM)).child(arrow))
                        .child(div().text_color(c(0xc77dbb)).child(v.name.clone()))
                        .child(div().text_color(c(theme::FG_DIM)).child("="))
                        .child(div().text_color(c(theme::FG)).child(v.value.clone()))
                        .on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, cx| {
                            if reference > 0 {
                                ws.toggle_var(id, reference, cx)
                            }
                        }))
                })
                .collect::<Vec<_>>()
        }));
        div().id("vars").size_full().flex().flex_col().child(watches).child(div().flex_1().min_h_0().child(list.size_full()))
    }

    fn render_console(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(s) = self.active_session() else { return div().into_any_element() };
        let session_id = s.id;
        let (follow, wrap) = (self.console_follow, self.console_wrap);
        // wrapped lines have different heights, so that mode renders the recent tail only
        let skip = if wrap { s.console.len().saturating_sub(3000) } else { 0 };
        let lines: Vec<(SharedString, bool, bool)> =
            s.console.iter().skip(skip).map(|(l, err)| (SharedString::from(l.clone()), *err, stack_link(l).is_some())).collect();
        let n = lines.len();
        let entity = cx.entity();
        let font = SharedString::from(self.settings.log_font_family.clone());
        let size = self.settings.log_font_px();
        let line_el = move |i: usize, (text, err, link): (SharedString, bool, bool), wrap: bool, e: Entity<Workspace>| {
            div()
                .id(("line", i))
                .px_2()
                .when(!wrap, |d| d.whitespace_nowrap())
                .font_family(font.clone())
                .text_size(px(size))
                .line_height(px((size * 1.4).round()))
                .text_color(c(if err { theme::ERROR } else if link { theme::ACCENT } else { theme::FG }))
                .when(link, |d| d.cursor_pointer().hover(|st| st.underline()))
                .child(text.clone())
                .when(link, |d| {
                    d.on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        let t = text.to_string();
                        e.update(cx, |ws, cx| ws.open_stack_link(&t, window, cx));
                    })
                })
        };
        let body: AnyElement = if wrap {
            if follow {
                self.console_wrap_scroll.scroll_to_bottom();
            }
            let e = entity.clone();
            div()
                .id("console-wrap")
                .size_full()
                .overflow_y_scroll()
                .track_scroll(&self.console_wrap_scroll)
                .children(lines.into_iter().enumerate().map(move |(i, l)| line_el(i, l, true, e.clone())))
                .into_any_element()
        } else {
            if follow && n > 0 {
                self.console_scroll.scroll_to_item(n - 1, gpui::ScrollStrategy::Top);
            }
            let e = entity.clone();
            // the longest line sets the list width, so long lines scroll sideways
            let widest = lines.iter().enumerate().max_by_key(|(_, l)| l.0.chars().count()).map(|(i, _)| i);
            uniform_list("console", n, move |range, _, _| range.map(|i| line_el(i, lines[i].clone(), false, e.clone())).collect())
                .with_horizontal_sizing_behavior(gpui::ListHorizontalSizingBehavior::Unconstrained)
                .with_width_from_item(widest)
                .track_scroll(self.console_scroll.clone())
                .size_full()
                .into_any_element()
        };
        let tool = |id: &'static str, label: String, on: bool| {
            div()
                .id(id)
                .px_2()
                .rounded_sm()
                .whitespace_nowrap()
                .text_color(c(if on { theme::ACCENT } else { theme::FG_DIM }))
                .hover(|st| st.bg(c(theme::ROW_HOVER)))
                .child(label)
        };
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_1()
                    .px_1()
                    .text_size(px(11.))
                    .border_b_1()
                    .border_color(c(theme::BORDER))
                    .child(tool("con-follow", format!("⤓ Theo dõi cuối {}", if follow { "(bật)" } else { "(tắt)" }), follow).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|ws, _, _, cx| {
                            ws.console_follow = !ws.console_follow;
                            cx.notify()
                        }),
                    ))
                    .child(tool("con-wrap", format!("↩ Xuống dòng {}", if wrap { "(bật)" } else { "(tắt)" }), wrap).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|ws, _, _, cx| {
                            ws.console_wrap = !ws.console_wrap;
                            cx.notify()
                        }),
                    ))
                    .child(tool("con-clear", "🗑 Xóa log".into(), false).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |ws, _, _, cx| {
                            if let Some(s) = ws.sessions.iter_mut().find(|s| s.id == session_id) {
                                s.console.clear();
                            }
                            cx.notify()
                        }),
                    ))
                    .child(div().flex_1())
                    .child(div().text_color(c(theme::FG_DIM)).child(format!("{n} dòng"))),
            )
            .child(
                div()
                    .id("console-area")
                    .on_mouse_down(MouseButton::Right, cx.listener(|ws, ev: &gpui::MouseDownEvent, _, cx| ws.console_menu(ev.position, cx)))
                    .flex_1()
                    .overflow_hidden()
                    // scrolling up stops following the tail, so the log can be read while it runs
                    .on_scroll_wheel(cx.listener(|ws, ev: &gpui::ScrollWheelEvent, _, cx| {
                        if ev.delta.pixel_delta(px(20.)).y > px(0.) && ws.console_follow {
                            ws.console_follow = false;
                            cx.notify()
                        }
                    }))
                    .child(body),
            )
            .into_any_element()
    }

    /// Console "at a.b.C.m(C.java:12)" → open that source in the project.
    pub(crate) fn open_stack_link(&mut self, line: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some((rel, num)) = stack_link(line) else { return };
        let roots: Vec<String> = self.project.as_ref().map(|p| p.modules.iter().flat_map(|m| m.source_roots.iter().map(|s| s.path.clone())).collect()).unwrap_or_default();
        let Some(path) = roots.iter().map(|r| PathBuf::from(r).join(&rel)).find(|p| p.is_file()) else {
            self.message = Some(format!("{rel} không nằm trong project (thư viện?)"));
            cx.notify();
            return;
        };
        self.push_nav(cx);
        self.open_file(path, window, cx);
        if let Some(t) = self.active.and_then(|i| self.tabs.get(i)).cloned() {
            t.update(cx, |v, cx| {
                v.goto(num.saturating_sub(1), 0);
                cx.notify()
            });
        }
    }
}

/// A square icon button of the toolbars and tool window stripes (rounded, hover / active fill).
pub(crate) fn tool_btn(id: impl Into<gpui::ElementId>, ico: crate::ui_icons::Ico, color: u32, active: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .size(px(28.))
        .flex_none()
        .rounded_md()
        .when(active, |d| d.bg(c(theme::ROW_ACTIVE)))
        .hover(|s| s.bg(c(theme::ROW_HOVER)))
        .child(crate::ui_icons::icon(ico, 16., c(color)))
}
