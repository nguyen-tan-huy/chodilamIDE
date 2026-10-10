//! Maven tool window - the features of java-debug-model's maven_panel / dependency_tree /
//! maven_jdk / :JavaMavenGoal / :JavaModelAddModule, with the same files on disk
//! (maven-jdk.json, manifest.json), plus per-project profiles and Skip Tests.
use crate::input::{InputEvent, TextInput};
use crate::theme::{self, c};
use crate::workspace::Workspace;
use crate::ui_icons::{Ico, icon, tip};
use gpui::{AnyElement, Context, Entity, MouseButton, SharedString, Window, anchored, deferred, div, prelude::*, px, uniform_list};
use runner::maven::{LIFECYCLE, TreeMark};
use std::collections::HashSet;
use std::path::PathBuf;

pub struct MavenUi {
    expanded: HashSet<String>,
    /// module path the goal box / dependency tree apply to
    selected: Option<String>,
    goal: Entity<TextInput>,
    profiles: Entity<TextInput>,
    add_module: Entity<TextInput>,
    jdk_menu: bool,
    dep: Option<DepTree>,
}

struct DepTree {
    module: String,
    name: String,
    lines: Vec<String>,
    filter: Entity<TextInput>,
    loading: bool,
    error: Option<String>,
}

const REACTOR: &str = "@reactor";
const REACTOR_LIFECYCLE: &str = "@reactor-lifecycle";

impl Workspace {
    fn maven_ui(&mut self, cx: &mut Context<Self>) -> &mut MavenUi {
        if self.maven_ui.is_none() {
            let prefs = runner::config::maven_prefs(&self.root_str());
            let profiles = cx.new(|cx| TextInput::new("Profiles: dev,local", cx));
            profiles.update(cx, |i, cx| i.set_value(&prefs.profiles.join(","), cx));
            // profiles are saved as they are typed (and used for runs + the next model reload)
            cx.subscribe(&profiles, |ws, _, ev: &InputEvent, _| {
                let InputEvent::Changed(t) = ev;
                let list: Vec<String> = t.split(',').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect();
                ws.save_maven_prefs_with(Some(list), None);
            })
            .detach();
            let mut expanded = HashSet::new();
            expanded.insert(REACTOR.to_string());
            self.maven_ui = Some(MavenUi {
                expanded,
                selected: None,
                goal: cx.new(|cx| TextInput::new("goal, vd: dependency:tree", cx)),
                profiles,
                add_module: cx.new(|cx| TextInput::new("Thêm module (đường dẫn)", cx)),
                jdk_menu: false,
                dep: None,
            });
        }
        self.maven_ui.as_mut().unwrap()
    }

    /// Profiles typed in the Maven panel (empty = none).
    pub(crate) fn maven_profiles(&self) -> Vec<String> {
        runner::config::maven_prefs(&self.root_str()).profiles
    }

    fn save_maven_prefs_with(&mut self, profiles: Option<Vec<String>>, skip: Option<bool>) {
        let root = self.root_str();
        let mut p = runner::config::maven_prefs(&root);
        if let Some(pr) = profiles {
            p.profiles = pr;
        }
        if let Some(s) = skip {
            p.skip_tests = s;
            self.skip_tests = s;
        }
        let _ = runner::config::set_maven_prefs(&root, &p);
    }

    fn run_reactor(&mut self, goals: Vec<String>, cx: &mut Context<Self>) {
        let refs: Vec<&str> = goals.iter().map(String::as_str).collect();
        let cmd = runner::maven::reactor_command(&self.root, &refs, &self.maven_profiles(), self.skip_tests, runner::config::project_jdk(&self.root_str()));
        self.maven_tab = false;
        self.run_command(format!("mvn {} (reactor)", goals.join(" ")), cmd, cx);
    }

    fn run_goal_box(&mut self, cx: &mut Context<Self>) {
        let ui = self.maven_ui(cx);
        let line = ui.goal.read(cx).content.trim().to_string();
        let Some(module) = ui.selected.clone() else {
            self.message = Some("Chọn một module trong cây Maven trước".into());
            return;
        };
        let goals = runner::maven::split_goal_line(&line);
        if goals.is_empty() {
            return;
        }
        self.maven_tab = false;
        self.run_maven(&module, goals, cx);
    }

    fn add_module_from_box(&mut self, cx: &mut Context<Self>) {
        let raw = self.maven_ui(cx).add_module.read(cx).content.trim().to_string();
        let path = match raw.strip_prefix("~/") {
            Some(rest) => PathBuf::from(std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).unwrap_or_default()).join(rest),
            None => PathBuf::from(&raw),
        };
        let path = std::path::absolute(&path).unwrap_or(path);
        if !path.join("pom.xml").is_file() {
            self.message = Some(format!("Không có pom.xml trong {}", path.display()));
            cx.notify();
            return;
        }
        let dir = path.display().to_string().trim_end_matches('/').to_string();
        match project_model::cache::manifest_add(&self.root_str(), &dir) {
            Ok(()) => {
                self.message = Some(format!("Đã thêm module {dir}, đang đọc lại mô hình…"));
                let ui = self.maven_ui(cx);
                ui.add_module.update(cx, |i, cx| i.set_value("", cx));
                self.restart_lsp(cx);
                self.reload_model(cx);
            }
            Err(e) => self.message = Some(format!("Không ghi được manifest.json: {e}")),
        }
        cx.notify();
    }

    pub(crate) fn remove_module_pub(&mut self, module: String, cx: &mut Context<Self>) {
        self.remove_module(module, cx)
    }

    pub(crate) fn open_dependency_tree_pub(&mut self, module: String, cx: &mut Context<Self>) {
        self.open_dependency_tree(module, cx)
    }

    fn remove_module(&mut self, module: String, cx: &mut Context<Self>) {
        match project_model::cache::manifest_remove(&self.root_str(), &module) {
            Ok(()) => {
                self.message = Some("Đã bỏ module khỏi project (file trên đĩa không bị đụng tới)".into());
                self.restart_lsp(cx);
                self.reload_model(cx);
            }
            Err(e) => self.message = Some(format!("Không ghi được manifest.json: {e}")),
        }
        cx.notify();
    }

    fn open_dependency_tree(&mut self, module_path: String, cx: &mut Context<Self>) {
        let Some(module) = self.project.as_ref().and_then(|p| p.modules.iter().find(|m| m.path == module_path)).cloned() else { return };
        let filter = cx.new(|cx| TextInput::new("lọc, vd: jackson (giữ cả nhánh cha)", cx));
        cx.subscribe(&filter, |_, _, _: &InputEvent, cx| cx.notify()).detach();
        let name = module.artifact_id.clone().unwrap_or_else(|| module_path.clone());
        self.maven_ui(cx).dep = Some(DepTree { module: module_path.clone(), name, lines: Vec::new(), filter, loading: true, error: None });
        cx.notify();
        let out = std::env::temp_dir().join(format!("chodilamide-deptree-{}-{}.txt", std::process::id(), module_path.len()));
        let cmd = runner::maven::dependency_tree_command(&module, &self.maven_profiles(), runner::config::project_jdk(&self.root_str()), &out);
        let task = cx.background_executor().spawn(async move {
            let mut c = std::process::Command::new(&cmd.program);
            c.args(&cmd.args).current_dir(&cmd.cwd);
            crate::debug::apply_java_home(&mut c, cmd.java_home.as_deref());
            let res = c.output();
            let text = std::fs::read_to_string(&out).ok();
            let _ = std::fs::remove_file(&out);
            match (res, text) {
                (Ok(o), Some(t)) if o.status.success() => Ok(t.lines().map(String::from).filter(|l| !l.is_empty()).collect::<Vec<_>>()),
                (Ok(o), _) => Err(String::from_utf8_lossy(&o.stdout).lines().filter(|l| l.contains("ERROR")).take(6).collect::<Vec<_>>().join("\n")),
                (Err(e), _) => Err(e.to_string()),
            }
        });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |ws, cx| {
                if let Some(dep) = ws.maven_ui.as_mut().and_then(|u| u.dep.as_mut()).filter(|d| d.module == module_path) {
                    dep.loading = false;
                    match res {
                        Ok(lines) => dep.lines = lines,
                        Err(e) => dep.error = Some(if e.is_empty() { "mvn dependency:tree lỗi".into() } else { e }),
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// "Module Spring Boot mới": a module of the open project (child of its parent pom, or independent).
    pub(crate) fn open_new_module(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use crate::new_project::{NewMode, NewProjectDialog, NewProjectEvent};
        let root = self.root.clone();
        let parent = crate::new_project::read_parent(&root);
        let d = cx.new(|cx| NewProjectDialog::new(NewMode::Module { root, parent }, "", cx));
        cx.subscribe_in(&d, window, |ws, _, ev: &NewProjectEvent, window, cx| {
            ws.new_module = None;
            if let NewProjectEvent::Created { dir, attached, .. } = ev {
                let name = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                if *attached {
                    ws.message = Some(format!("Đã tạo module {name} và thêm vào parent pom, đang đọc lại mô hình…"));
                } else {
                    // independent: the IDE's own module list (manifest.json), like "Thêm module"
                    ws.message = Some(match project_model::cache::manifest_add(&ws.root_str(), &dir.display().to_string()) {
                        Ok(()) => format!("Đã tạo module {name}, đang đọc lại mô hình…"),
                        Err(e) => format!("Đã tạo module {name} nhưng không ghi được manifest.json: {e}"),
                    });
                }
                ws.restart_lsp(cx);
                ws.reload_model(cx);
                ws.open_file(dir.join("pom.xml"), window, cx);
            }
            cx.notify();
        })
        .detach();
        let focus = d.read(cx).first_focus(cx);
        crate::dialogs::focus_later(focus, window, cx);
        self.new_module = Some(d);
        cx.notify();
    }

    /// Screenshot checks (CHODILAMIDE_OPEN=maven / maven-deps).
    pub(crate) fn debug_open_maven(&mut self, deps: bool, cx: &mut Context<Self>) {
        self.maven_side = true;
        if let Some(m) = self.project.as_ref().and_then(|p| p.modules.first()).map(|m| m.path.clone()) {
            let ui = self.maven_ui(cx);
            ui.selected = Some(m.clone());
            ui.expanded.insert(m.clone());
            if deps {
                self.open_dependency_tree(m, cx);
            }
        }
        cx.notify();
    }

    pub(crate) fn render_maven_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        self.maven_ui(cx);
        let root_name = crate::tree::file_name(&self.root);
        let modules: Vec<(String, String, bool)> = self
            .project
            .as_ref()
            .map(|p| {
                let mut v: Vec<_> = p.modules.iter().map(|m| (m.path.clone(), m.artifact_id.clone().unwrap_or_else(|| m.path.clone()), m.in_reactor)).collect();
                v.sort_by(|a, b| a.1.cmp(&b.1));
                v
            })
            .unwrap_or_default();
        let jdk_label = runner::config::project_jdk(&self.root_str())
            .and_then(|p| toolchain::jdk_major(std::path::Path::new(&p)).map(|m| format!("JDK {m}")))
            .unwrap_or_else(|| "JDK mặc định".into());
        let skip = self.skip_tests;
        let ui = self.maven_ui.as_ref().unwrap();
        let (expanded, selected) = (ui.expanded.clone(), ui.selected.clone());
        let (goal, profiles, add_module, jdk_menu) = (ui.goal.clone(), ui.profiles.clone(), ui.add_module.clone(), ui.jdk_menu);
        let selected_name = selected.as_ref().and_then(|s| modules.iter().find(|m| &m.0 == s)).map(|m| m.1.clone());

        // ---- tree rows (depth, element)
        let mut rows: Vec<AnyElement> = Vec::new();
        let has_reactor = modules.iter().any(|m| m.2);
        let row = |depth: usize, id: SharedString| div().id(id).flex().flex_row().items_center().gap_1p5().h(px(26.)).mx_1().rounded_md().pl(px(6. + 16. * depth as f32)).pr_2().whitespace_nowrap().hover(|s| s.bg(c(theme::ROW_HOVER)));
        // lifecycle phases as a wrapping row of chips instead of a vertical list
        let phase_rows = |depth: usize, key: &str, module: Option<String>, cx: &mut Context<Self>| -> AnyElement {
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .gap_1()
                .py_1()
                .pl(px(8. + 16. * depth as f32))
                .pr_2()
                .children(LIFECYCLE.iter().map(|phase| {
                    let module = module.clone();
                    div()
                        .id(SharedString::from(format!("{key}-{phase}")))
                        .px_2()
                        .h(px(22.))
                        .flex()
                        .items_center()
                        .rounded_full()
                        .border_1()
                        .border_color(c(theme::BORDER))
                        .text_size(px(12.))
                        .text_color(c(theme::FG))
                        .hover(|s| s.bg(c(theme::ROW_HOVER)).border_color(c(theme::RUN_GREEN)).text_color(c(theme::RUN_GREEN)))
                        .child(*phase)
                        .on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, cx| {
                            ws.maven_tab = false;
                            match &module {
                                Some(m) => ws.run_maven(m, vec![phase.to_string()], cx),
                                None => ws.run_reactor(vec![phase.to_string()], cx),
                            }
                        }))
                }))
                .into_any_element()
        };
        let toggle = |key: String| {
            move |ws: &mut Workspace, _: &gpui::MouseDownEvent, _: &mut Window, cx: &mut Context<Workspace>| {
                if let Some(u) = ws.maven_ui.as_mut() {
                    if !u.expanded.remove(&key) {
                        u.expanded.insert(key.clone());
                    }
                }
                cx.notify()
            }
        };
        let arrow = |open: bool| div().w(px(12.)).flex_none().child(icon(if open { Ico::ChevronDown } else { Ico::ChevronRight }, 12., c(theme::FG_DIM)));

        let module_rows = |ms: Vec<&(String, String, bool)>, depth: usize, rows: &mut Vec<AnyElement>, cx: &mut Context<Self>| {
            for (path, name, in_reactor) in ms {
                let open = expanded.contains(path);
                let is_sel = selected.as_deref() == Some(path.as_str());
                let (p1, p2, p3) = (path.clone(), path.clone(), path.clone());
                rows.push(
                    row(depth, SharedString::from(format!("mod-{path}")))
                        .group(SharedString::from(format!("g-{path}")))
                        .on_mouse_down(MouseButton::Right, cx.listener({
                            let p = path.clone();
                            move |ws, ev: &gpui::MouseDownEvent, _, cx| ws.maven_module_menu(ev.position, p.clone(), cx)
                        }))
                        .when(is_sel, |d| d.bg(c(theme::ROW_ACTIVE)))
                        .child(arrow(open))
                        .child(if *in_reactor {
                            div().size(px(10.)).flex_none().rounded_sm().bg(c(theme::ACCENT))
                        } else {
                            div().size(px(10.)).flex_none().rounded_sm().border_1().border_color(c(theme::ACCENT))
                        })
                        .child(div().text_color(c(theme::FG)).child(name.clone()))
                        .when(!in_reactor, |d| d.child(div().px_1p5().rounded_full().bg(c(theme::ROW_HOVER)).text_size(px(10.)).text_color(c(theme::FG_DIM)).child("independent")))
                        .child(div().flex_1())
                        // actions appear while the row is hovered
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .gap_0p5()
                                .opacity(0.)
                                .group_hover(SharedString::from(format!("g-{path}")), |s| s.opacity(1.))
                                .child(
                                    div()
                                        .id(SharedString::from(format!("deps-{path}")))
                                        .p_1()
                                        .rounded_sm()
                                        .hover(|s| s.bg(c(theme::ROW_ACTIVE)))
                                        .tooltip(tip("Dependency tree"))
                                        .child(icon(Ico::Tree, 13., c(theme::ACCENT)))
                                        .on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, cx| {
                                            cx.stop_propagation();
                                            ws.open_dependency_tree(p2.clone(), cx)
                                        })),
                                )
                                .child(
                                    div()
                                        .id(SharedString::from(format!("rm-{path}")))
                                        .p_1()
                                        .rounded_sm()
                                        .hover(|s| s.bg(c(theme::ROW_ACTIVE)))
                                        .tooltip(tip("Bỏ module khỏi project"))
                                        .child(icon(Ico::Close, 12., c(theme::ERROR)))
                                        .on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, cx| {
                                            cx.stop_propagation();
                                            ws.remove_module(p3.clone(), cx)
                                        })),
                                ),
                        )
                        .on_mouse_down(MouseButton::Left, cx.listener(move |ws, ev: &gpui::MouseDownEvent, window, cx| {
                            if let Some(u) = ws.maven_ui.as_mut() {
                                u.selected = Some(p1.clone());
                            }
                            toggle(p1.clone())(ws, ev, window, cx)
                        }))
                        .into_any_element(),
                );
                if open {
                    rows.push(phase_rows(depth + 1, path, Some(path.clone()), cx));
                }
            }
        };

        if has_reactor {
            let open = expanded.contains(REACTOR);
            rows.push(
                row(0, "mvn-reactor".into())
                    .child(arrow(open))
                    .child(div().text_color(c(theme::WARNING)).child("◆"))
                    .child(div().text_color(c(theme::FG)).child(root_name.clone()))
                    .on_mouse_down(MouseButton::Left, cx.listener(toggle(REACTOR.to_string())))
                    .into_any_element(),
            );
            if open {
                let lc_open = expanded.contains(REACTOR_LIFECYCLE);
                rows.push(
                    row(1, "mvn-reactor-lc".into())
                        .child(arrow(lc_open))
                        .child(div().text_color(c(theme::FG)).child("Lifecycle (toàn bộ reactor)"))
                        .on_mouse_down(MouseButton::Left, cx.listener(toggle(REACTOR_LIFECYCLE.to_string())))
                        .into_any_element(),
                );
                if lc_open {
                    rows.push(phase_rows(2, "reactor", None, cx));
                }
                module_rows(modules.iter().filter(|m| m.2).collect(), 1, &mut rows, cx);
            }
        }
        module_rows(modules.iter().filter(|m| !m.2).collect(), 0, &mut rows, cx);
        if modules.is_empty() {
            rows.push(div().p_2().text_color(c(theme::FG_DIM)).child("Chưa có mô hình Maven (đang đọc, hoặc không có pom.xml)").into_any_element());
        }

        // ---- toolbar: icon buttons + chips, inputs without separate labels
        let chip = |id: &'static str, active: bool| {
            div()
                .id(id)
                .h(px(28.))
                .px_2p5()
                .flex()
                .flex_row()
                .items_center()
                .gap_1p5()
                .rounded_md()
                .whitespace_nowrap()
                .border_1()
                .border_color(c(if active { theme::ACCENT } else { theme::BORDER }))
                .when(active, |d| d.bg(c(theme::ROW_ACTIVE)))
                .hover(|s| s.bg(c(theme::ROW_HOVER)))
        };
        // header: title, the everyday toggles as chips / icons
        let toolbar = div()
            .w_full()
            .flex()
            .flex_col()
            .border_b_1()
            .border_color(c(theme::BORDER))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .h(px(36.))
                    .pl_3()
                    .pr_1()
                    .child(icon(Ico::Maven, 15., c(theme::ACCENT)))
                    .child(div().font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(theme::FG)).child("Maven"))
                    .child(div().flex_1())
                    .child(crate::git_style::icon_btn("mvn-new", Ico::Plus, "Module Spring Boot mới").on_mouse_down(MouseButton::Left, cx.listener(|ws, _, window, cx| ws.open_new_module(window, cx))))
                    .child(crate::git_style::icon_btn("mvn-reload", Ico::Refresh, "Reload: đọc lại pom, tải jar thiếu (Ctrl+Shift+O)").on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.reload_model(cx))))
                    .child(crate::git_style::icon_btn("mvn-hide", Ico::Minimize, "Ẩn Maven, quay lại cây project").on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| {
                        ws.maven_side = false;
                        cx.notify()
                    }))),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .pb_2()
                    .child(
                        chip("mvn-skip", skip)
                            .tooltip(tip("Bỏ qua test khi chạy Maven"))
                            .child(icon(if skip { Ico::Check } else { Ico::Minus }, 12., c(if skip { theme::ACCENT } else { theme::FG_DIM })))
                            .child("Bỏ qua test")
                            .on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| {
                                let s = !ws.skip_tests;
                                ws.save_maven_prefs_with(None, Some(s));
                                cx.notify()
                            })),
                    )
                    .child(
                        chip("mvn-jdk", false)
                            .tooltip(tip("JDK chạy Maven"))
                            .child(jdk_label.clone())
                            .child(icon(Ico::ChevronDown, 11., c(theme::FG_DIM)))
                            .on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| {
                                if let Some(u) = ws.maven_ui.as_mut() {
                                    u.jdk_menu = !u.jdk_menu;
                                }
                                cx.notify()
                            })),
                    ),
            )
            .child(div().w_full().flex().flex_row().px_3().pb_2().child(div().debug_selector(|| "mvn-profiles".into()).flex_1().min_w_0().child(profiles)))
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .pb_2()
                    .child(div().flex_1().min_w_0().child(add_module))
                    .child(
                        chip("mvn-add", false)
                            .tooltip(tip("Thêm module vào project"))
                            .child(icon(Ico::Plus, 12., c(theme::ACCENT)))
                            .on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.add_module_from_box(cx))),
                    ),
            );

        let goal_bar = div()
            .w_full()
            .flex()
            .flex_col()
            .gap_1()
            .px_3()
            .py_2()
            .border_t_1()
            .border_color(c(theme::BORDER))
            .child(div().truncate().text_size(px(11.)).text_color(c(theme::FG_DIM)).child(format!("Goal cho {}", selected_name.unwrap_or_else(|| "(chọn một module ở trên)".into()))))
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().min_w_0().child(goal))
                    .child(
                        chip("mvn-goal", false)
                            .child(icon(Ico::Play, 12., c(theme::RUN_GREEN)))
                            .child("Chạy")
                            .on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.run_goal_box(cx))),
                    ),
            );

        let tree = div()
            .flex()
            .flex_col()
            .h_full()
            .child(toolbar)
            .child(div().id("mvn-tree").flex_1().overflow_y_scroll().children(rows))
            .child(goal_bar);

        let dep = self.render_dep_tree(cx);
        let jdk_popup = jdk_menu.then(|| self.render_maven_jdk_menu(cx));
        // docked in the project tree's place: modules, or (after "Dependencies") the dependency
        // tree of one module in the same column, with its own close button to come back
        let body = match dep {
            Some(d) => d,
            None => tree.into_any_element(),
        };
        div()
            .debug_selector(|| "maven-window".into())
            .flex()
            .flex_col()
            .w(px(self.sizes.tree))
            .h_full()
            .rounded(px(theme::ISLAND_RADIUS))
            .overflow_hidden()
            .bg(c(theme::BG))
            .border_1()
            .border_color(c(theme::BORDER))
            .relative()
            .child(div().flex_1().min_h_0().child(body))
            .children(jdk_popup)
            .into_any_element()
    }

    fn render_maven_jdk_menu(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let current = runner::config::maven_jdk(&self.root_str());
        let mut items: Vec<(String, Option<String>)> = vec![("Theo JDK mặc định của app".into(), None)];
        items.extend(toolchain::find_jdks().into_iter().map(|j| (j.label(), Some(j.path.display().to_string()))));
        let rows: Vec<AnyElement> = items
            .into_iter()
            .enumerate()
            .map(|(i, (label, path))| {
                let active = path == current;
                div()
                    .id(("mvn-jdk-item", i))
                    .px_2()
                    .whitespace_nowrap()
                    .mx_1()
                    .rounded_md()
                    .when(active, |d| d.bg(c(theme::ROW_ACTIVE)))
                    .hover(|s| s.bg(c(theme::ROW_HOVER)))
                    .child(label)
                    .on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, cx| {
                        let root = ws.root_str();
                        ws.message = Some(match runner::config::set_maven_jdk(&root, path.as_deref()) {
                            Ok(()) => "Đã đổi JDK cho Maven (dùng chung maven-jdk.json với nvim)".into(),
                            Err(e) => format!("Không lưu được: {e}"),
                        });
                        if let Some(u) = ws.maven_ui.as_mut() {
                            u.jdk_menu = false;
                        }
                        cx.notify()
                    }))
                    .into_any_element()
            })
            .collect();
        deferred(
            anchored().child(
                div()
                    .mt(px(70.))
                    .ml(px(100.))
                    .py_1()
                    .bg(c(theme::POPUP_BG))
                    .border_1()
                    .border_color(c(theme::BORDER))
                    .shadow_lg().occlude().rounded_lg()
                    .children(rows),
            ),
        )
        .into_any_element()
    }

    fn render_dep_tree(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dep = self.maven_ui.as_ref()?.dep.as_ref()?;
        let q = dep.filter.read(cx).content.clone();
        let (kept, matches) = runner::maven::filter_tree(&dep.lines, &q);
        let lines: Vec<(SharedString, TreeMark, bool)> = kept
            .iter()
            .map(|l| (SharedString::from((*l).clone()), runner::maven::tree_mark(l), !q.is_empty() && l.to_lowercase().contains(&q.to_lowercase())))
            .collect();
        let conflicts = dep.lines.iter().filter(|l| runner::maven::tree_mark(l) == TreeMark::Conflict).count();
        let status = if dep.loading {
            "đang chạy mvn dependency:tree -Dverbose…".to_string()
        } else if let Some(e) = &dep.error {
            format!("Lỗi: {e}")
        } else if q.is_empty() {
            format!("{} dòng · {} xung đột phiên bản (đỏ)", dep.lines.len(), conflicts)
        } else {
            format!("{matches} dòng khớp")
        };
        let dep_failed = dep.error.is_some();
        let (name, module, filter) = (dep.name.clone(), dep.module.clone(), dep.filter.clone());
        let n = lines.len();
        let (log_font, log_size) = (SharedString::from(self.settings.log_font_family.clone()), self.settings.log_font_px());
        Some(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .size_full()
                // header row 1: back, module, actions. Row 2: the filter on a line of its own
                .child(
                    div()
                        .w_full()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_1p5()
                        .h(px(36.))
                        .pl_1()
                        .pr_1()
                        .child(
                            div()
                                .id("dep-back")
                                .p_1()
                                .rounded_sm()
                                .hover(|s| s.bg(c(theme::ROW_HOVER)))
                                .tooltip(tip("Quay lại danh sách module"))
                                .child(icon(Ico::ChevronLeft, 14., c(theme::FG_DIM)))
                                .on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| {
                                    if let Some(u) = ws.maven_ui.as_mut() {
                                        u.dep = None;
                                    }
                                    cx.notify()
                                })),
                        )
                        .child(div().flex_1().min_w_0().truncate().font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(theme::FG)).child(SharedString::from(format!("{name} · Dependencies"))))
                        .child(div().id("dep-reload").p_1().rounded_sm().hover(|s| s.bg(c(theme::ROW_HOVER))).tooltip(tip("Chạy lại dependency:tree")).child(icon(Ico::Refresh, 13., c(theme::FG_DIM))).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |ws, _, _, cx| ws.open_dependency_tree(module.clone(), cx)),
                        )),
                )
                .child(div().w_full().flex().flex_row().px_3().pb_1().child(div().flex_1().min_w_0().child(filter)))
                .child(div().px_3().pb_1().text_size(px(11.)).text_color(c(if dep_failed { theme::ERROR } else { theme::FG_DIM })).child(status))
                .child(
                    uniform_list("dep-lines", n, move |range, _, _| {
                        range
                            .map(|i| {
                                let (text, mark, hit) = lines[i].clone();
                                let base = div()
                                    .id(("dep", i))
                                    .w_full()
                                    .overflow_hidden()
                                    .h(px(24.))
                                    .px_2()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_1p5()
                                    .whitespace_nowrap()
                                    .hover(|s| s.bg(c(theme::ROW_HOVER)))
                                    .when(hit, |d| d.bg(c(theme::FIND_MATCH)));
                                let Some(r) = parse_dep_line(&text) else {
                                    // not a coordinate line: show as is
                                    return base.font_family(log_font.clone()).text_size(px(log_size)).text_color(c(theme::FG_DIM)).child(text);
                                };
                                let (name_color, dim) = match mark {
                                    TreeMark::Conflict => (theme::ERROR, false),
                                    TreeMark::Duplicate => (theme::FG_DIM, true),
                                    TreeMark::Normal => (theme::FG, false),
                                };
                                let badge = |label: String, color: gpui::Hsla| div().px_1p5().rounded_full().bg(color.opacity(0.14)).text_size(px(10.5)).text_color(color).child(label);
                                // one thin guide per level, like the project tree
                                let guides = (0..r.depth).map(|_| div().w(px(14.)).h_full().flex_none().border_l_1().border_color(c(theme::BORDER)));
                                base.children(guides)
                                    .child(div().size(px(6.)).flex_none().rounded_full().bg(c(if mark == TreeMark::Conflict { theme::ERROR } else if dim { theme::BORDER } else { theme::ACCENT })))
                                    .child(div().flex_none().text_color(c(name_color)).font_weight(if r.depth == 0 { gpui::FontWeight::SEMIBOLD } else { gpui::FontWeight::NORMAL }).child(r.artifact))
                                    // version and scope before the group: they stay visible in a narrow column
                                    .child(div().flex_none().child(badge(r.version, c(if mark == TreeMark::Conflict { theme::ERROR } else { theme::ACCENT }))))
                                    .when(!r.scope.is_empty() && r.scope != "compile", |d| d.child(div().flex_none().child(badge(r.scope.clone(), c(theme::WARNING)))))
                                    .when(mark == TreeMark::Conflict, |d| d.child(div().flex_none().child(badge("conflict".into(), c(theme::ERROR)))))
                                    .child(div().flex_shrink().min_w_0().truncate().text_size(px(11.)).text_color(c(theme::FG_DIM)).child(r.group))
                            })
                            .collect()
                    })
                    .flex_1(),
                )
                .into_any_element(),
        )
    }
}

/// One `mvn dependency:tree -Dverbose` line split into pieces for display.
struct DepRow {
    depth: usize,
    group: String,
    artifact: String,
    version: String,
    scope: String,
}

fn parse_dep_line(line: &str) -> Option<DepRow> {
    let body = line.trim_start_matches(|ch| matches!(ch, '|' | ' ' | '+' | '-' | '\\'));
    let prefix = line.len() - body.len();
    let body = body.strip_prefix('(').unwrap_or(body);
    let (coords, _) = body.split_once(' ').unwrap_or((body, ""));
    let parts: Vec<&str> = coords.split(':').collect();
    // group:artifact:type:version[:scope]  or  group:artifact:type:classifier:version:scope
    let (group, artifact, version, scope) = match parts.len() {
        4 => (parts[0], parts[1], parts[3], ""),
        5 => (parts[0], parts[1], parts[3], parts[4]),
        6 => (parts[0], parts[1], parts[4], parts[5]),
        _ => return None,
    };
    Some(DepRow {
        depth: if prefix == 0 { 0 } else { prefix / 3 },
        group: group.into(),
        artifact: artifact.into(),
        version: version.into(),
        scope: scope.into(),
    })
}
