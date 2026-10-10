//! Run/Debug Configurations editor (like IntelliJ's): list on the left, form on the right.
//! Saves to the shared .nvim/java-debug-model/debug-configs.json (java-debug-model reads
//! the same file; fields it wrote that we don't know are kept by RunConfig::extra).
use crate::dialogs::{self, DialogCancel, DialogConfirm, DialogNext, DialogPrev, Field};
use crate::theme::{self, c};
use crate::workspace::Workspace;
use gpui::{AnyElement, Context, Focusable, MouseButton, SharedString, Window, div, prelude::*, px};
use runner::config::RunConfig;
use std::collections::BTreeMap;

pub struct ConfigEditor {
    configs: Vec<RunConfig>,
    selected: usize,
    name: Field,
    main_class: Field,
    workdir: Field,
    vm_args: Field,
    program_args: Field,
    env: Field,
    profiles: Field,
    modules: Vec<(String, String)>, // (path, artifactId)
    module: usize,
    jdks: Vec<(String, Option<String>)>, // (label, jdk_path)
    jdk: usize,
    /// profiles found in application-*.properties/yml, toggled in VM options
    spring_profiles: Vec<String>,
    error: Option<String>,
}

const ACTIVE: &str = "-Dspring.profiles.active=";

/// Active Spring profiles written in VM options (`-Dspring.profiles.active=a,b`).
pub fn active_profiles(vm: &str) -> Vec<String> {
    vm.split_whitespace().find_map(|a| a.strip_prefix(ACTIVE)).map(|v| v.trim_matches('"').split(',').filter(|p| !p.is_empty()).map(String::from).collect()).unwrap_or_default()
}

/// Turns profile `p` on/off in the VM options, keeping every other option as typed.
pub fn toggle_profile(vm: &str, p: &str) -> String {
    let mut active = active_profiles(vm);
    match active.iter().position(|a| a == p) {
        Some(i) => {
            active.remove(i);
        }
        None => active.push(p.to_string()),
    }
    let mut args: Vec<String> = vm.split_whitespace().filter(|a| !a.starts_with(ACTIVE)).map(String::from).collect();
    if !active.is_empty() {
        args.push(format!("{ACTIVE}{}", active.join(",")));
    }
    args.join(" ")
}

/// "K=V,K2=V2" - the same text format as java-debug-model's config form.
pub fn format_env(env: &BTreeMap<String, String>) -> String {
    env.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(",")
}

pub fn parse_env(text: &str) -> BTreeMap<String, String> {
    text.split(',')
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .filter(|(k, _)| !k.is_empty() && k.chars().all(|c| c.is_alphanumeric() || c == '_'))
        .collect()
}

impl ConfigEditor {
    fn fields(&self) -> [&Field; 7] {
        [&self.name, &self.main_class, &self.workdir, &self.vm_args, &self.program_args, &self.env, &self.profiles]
    }

    /// Writes the form into `configs[selected]`.
    fn commit(&mut self, cx: &gpui::App) {
        let Some(cfg) = self.configs.get_mut(self.selected) else { return };
        cfg.name = self.name.text(cx);
        cfg.main_class = self.main_class.text(cx);
        cfg.working_directory = self.workdir.text(cx);
        cfg.vm_args = self.vm_args.text(cx);
        cfg.program_args = self.program_args.text(cx);
        cfg.env_vars = parse_env(&self.env.text(cx));
        cfg.maven_profiles = self.profiles.text(cx).split(',').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect();
        if let Some((path, _)) = self.modules.get(self.module) {
            cfg.module_path = path.clone();
        }
        cfg.jdk_path = self.jdks.get(self.jdk).and_then(|(_, p)| p.clone());
    }

    /// Loads `configs[selected]` into the form.
    fn load(&mut self, cx: &mut gpui::App) {
        let Some(cfg) = self.configs.get(self.selected).cloned() else { return };
        self.name.set(&cfg.name, cx);
        self.main_class.set(&cfg.main_class, cx);
        self.workdir.set(&cfg.working_directory, cx);
        self.vm_args.set(&cfg.vm_args, cx);
        self.program_args.set(&cfg.program_args, cx);
        self.env.set(&format_env(&cfg.env_vars), cx);
        self.profiles.set(&cfg.maven_profiles.join(","), cx);
        self.module = self.modules.iter().position(|(p, _)| *p == cfg.module_path).unwrap_or(0);
        self.jdk = cfg.jdk_path.as_ref().and_then(|j| self.jdks.iter().position(|(_, p)| p.as_ref() == Some(j))).unwrap_or(0);
        self.error = None;
    }

    fn validate(&self) -> Result<(), String> {
        let mut names = std::collections::HashSet::new();
        for c in &self.configs {
            if c.name.trim().is_empty() {
                return Err("Tên cấu hình không được trống".into());
            }
            if c.main_class.trim().is_empty() {
                return Err(format!("'{}': thiếu main class", c.name));
            }
            if !names.insert(c.name.clone()) {
                return Err(format!("Trùng tên cấu hình '{}'", c.name));
            }
        }
        Ok(())
    }
}

impl Workspace {
    pub(crate) fn open_config_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.config_menu_open = false;
        let root = self.root_str();
        let configs = runner::config::load(&root);
        let active = runner::config::active(&root);
        let modules: Vec<(String, String)> = self
            .project
            .as_ref()
            .map(|p| p.modules.iter().map(|m| (m.path.clone(), m.artifact_id.clone().unwrap_or_else(|| m.path.clone()))).collect())
            .unwrap_or_default();
        let mut jdks: Vec<(String, Option<String>)> = vec![("Mặc định".into(), None)];
        jdks.extend(toolchain::find_jdks().into_iter().map(|j| (j.label(), Some(j.path.display().to_string()))));
        let spring_profiles = self.project.as_ref().map(spring::config::profiles).unwrap_or_default();
        let mut ed = ConfigEditor {
            spring_profiles,
            selected: active.as_ref().and_then(|a| configs.iter().position(|c| &c.name == a)).unwrap_or(0),
            configs,
            name: Field::new("Tên", "vd: ProductWebApplication", "", cx),
            main_class: Field::new("Main class", "vd: vn.longvan.productweb.ProductWebApplication", "", cx),
            workdir: Field::new("Thư mục chạy", "mặc định: thư mục module", "", cx),
            vm_args: Field::new("VM options", "vd: -Xmx1g -Dspring.profiles.active=dev", "", cx),
            program_args: Field::new("Program arguments", "", "", cx),
            env: Field::new("Biến môi trường", "KEY=giá trị,KEY2=giá trị (như bản nvim)", "", cx),
            profiles: Field::new("Maven profiles", "vd: dev,local", "", cx),
            modules,
            module: 0,
            jdks,
            jdk: 0,
            error: None,
        };
        if ed.configs.is_empty() {
            ed.configs.push(RunConfig { name: "Cấu hình mới".into(), ..Default::default() });
        }
        ed.load(cx);
        crate::dialogs::focus_later(ed.name.input.read(cx).focus_handle(cx), window, cx);
        self.config_editor = Some(ed);
        cx.notify();
    }

    fn close_config_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.config_editor = None;
        if let Some(i) = self.active {
            self.activate(i, window, cx);
        }
        cx.notify();
    }

    /// Saves every config; returns false (and shows the error) when something is invalid.
    fn save_config_editor(&mut self, cx: &mut Context<Self>) -> bool {
        let root = self.root_str();
        let Some(ed) = self.config_editor.as_mut() else { return false };
        ed.commit(cx);
        if let Err(e) = ed.validate() {
            ed.error = Some(e);
            cx.notify();
            return false;
        }
        match runner::config::save(&root, &ed.configs) {
            Ok(()) => {
                if let Some(c) = ed.configs.get(ed.selected) {
                    let _ = runner::config::set_active(&root, &c.name);
                }
                self.message = Some("Đã lưu cấu hình chạy".into());
                true
            }
            Err(e) => {
                ed.error = Some(format!("Không lưu được: {e}"));
                cx.notify();
                false
            }
        }
    }

    fn select_config(&mut self, i: usize, cx: &mut Context<Self>) {
        if let Some(ed) = self.config_editor.as_mut() {
            ed.commit(cx);
            ed.selected = i.min(ed.configs.len().saturating_sub(1));
            ed.load(cx);
        }
        cx.notify();
    }

    fn add_config(&mut self, duplicate: bool, cx: &mut Context<Self>) {
        let Some(ed) = self.config_editor.as_mut() else { return };
        ed.commit(cx);
        let mut cfg = if duplicate {
            ed.configs.get(ed.selected).cloned().unwrap_or_default()
        } else {
            let module = ed.modules.first().map(|(p, _)| p.clone()).unwrap_or_default();
            RunConfig { module_path: module.clone(), working_directory: module, ..Default::default() }
        };
        let base = if duplicate { format!("{} (bản sao)", cfg.name) } else { "Cấu hình mới".into() };
        let mut name = base.clone();
        let mut n = 2;
        while ed.configs.iter().any(|c| c.name == name) {
            name = format!("{base} {n}");
            n += 1;
        }
        cfg.name = name;
        ed.configs.push(cfg);
        ed.selected = ed.configs.len() - 1;
        ed.load(cx);
        cx.notify();
    }

    fn delete_config(&mut self, cx: &mut Context<Self>) {
        let Some(ed) = self.config_editor.as_mut() else { return };
        if ed.selected < ed.configs.len() {
            ed.configs.remove(ed.selected);
        }
        ed.selected = ed.selected.min(ed.configs.len().saturating_sub(1));
        ed.load(cx);
        cx.notify();
    }

    pub(crate) fn render_config_editor(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        // the Maven model may arrive after the dialog opened: fill the module list then
        if self.config_editor.as_ref().is_some_and(|e| e.modules.is_empty()) {
            if let Some(p) = self.project.as_ref() {
                let modules: Vec<(String, String)> = p.modules.iter().map(|m| (m.path.clone(), m.artifact_id.clone().unwrap_or_else(|| m.path.clone()))).collect();
                if let Some(ed) = self.config_editor.as_mut() {
                    let current = ed.configs.get(ed.selected).map(|c| c.module_path.clone()).unwrap_or_default();
                    ed.module = modules.iter().position(|(p, _)| *p == current).unwrap_or(0);
                    ed.modules = modules;
                }
            }
        }
        let ed = self.config_editor.as_ref()?;
        let list = ed.configs.iter().enumerate().map(|(i, cfg)| {
            div()
                .id(("cfg-item", i))
                .px_2()
                .whitespace_nowrap()
                .mx_1()
                .rounded_md()
                .when(i == ed.selected, |d| d.bg(c(theme::ROW_ACTIVE)))
                .hover(|s| s.bg(c(theme::ROW_HOVER)))
                .child(div().text_color(c(theme::FG)).child(if cfg.name.is_empty() { "(chưa đặt tên)".to_string() } else { cfg.name.clone() }))
                .on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, cx| ws.select_config(i, cx)))
                .into_any_element()
        });
        let list: Vec<AnyElement> = list.collect();
        let mut module_names: Vec<String> = ed.modules.iter().map(|(_, a)| a.clone()).collect();
        if module_names.is_empty() {
            module_names.push("(đang đọc mô hình Maven…)".into());
        }
        let jdk_names: Vec<String> = ed.jdks.iter().map(|(l, _)| l.clone()).collect();
        let (module, jdk, error) = (ed.module, ed.jdk, ed.error.clone());
        // "✓ dev" = active in VM options; click toggles it
        let on = active_profiles(&ed.vm_args.text(cx));
        let spring_chips: Vec<String> = ed.spring_profiles.iter().map(|p| if on.contains(p) { format!("✓ {p}") } else { p.clone() }).collect();
        let rows: Vec<AnyElement> = ed.fields().iter().map(|f| f.row().into_any_element()).collect();
        let form = div()
            .flex_1()
            .flex()
            .flex_col()
            .gap_2()
            .children(rows)
            .child(dialogs::choice_row("Module", &module_names, module, "cfg-module", cx, |ws: &mut Workspace, i, _| {
                if let Some(ed) = ws.config_editor.as_mut() {
                    ed.module = i;
                }
            }))
            .child(dialogs::choice_row("JDK chạy app", &jdk_names, jdk, "cfg-jdk", cx, |ws: &mut Workspace, i, _| {
                if let Some(ed) = ws.config_editor.as_mut() {
                    ed.jdk = i;
                }
            }))
            .when(!spring_chips.is_empty(), |d| {
                d.child(dialogs::choice_row("Spring profiles", &spring_chips, usize::MAX, "cfg-spring", cx, |ws: &mut Workspace, i, cx| {
                    if let Some(ed) = ws.config_editor.as_mut() {
                        if let Some(p) = ed.spring_profiles.get(i).cloned() {
                            let vm = toggle_profile(&ed.vm_args.text(cx), &p);
                            ed.vm_args.set(&vm, cx);
                        }
                    }
                }))
            })
            .when_some(error, |d, e| d.child(div().text_color(c(theme::ERROR)).child(SharedString::from(e))));

        let body = div().flex().flex_col().gap_2()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_3()
                    .child(
                        div()
                            .w(px(230.))
                            .flex_none()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .border_r_1()
                            .border_color(c(theme::BORDER))
                            .pr_2()
                            .child(
                                div()
                                    .flex()
                                    .gap_1()
                                    .child(dialogs::button("cfg-add", "+ Mới", false, cx, |ws: &mut Workspace, _, cx| ws.add_config(false, cx)))
                                    .child(dialogs::button("cfg-dup", "Nhân bản", false, cx, |ws: &mut Workspace, _, cx| ws.add_config(true, cx)))
                                    .child(dialogs::button("cfg-del", "Xóa", false, cx, |ws: &mut Workspace, _, cx| ws.delete_config(cx))),
                            )
                            .children(list),
                    )
                    .child(form),
            );
        let footer = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .w_full()
            .child(div().flex_1().whitespace_nowrap().overflow_hidden().text_color(c(theme::FG_DIM)).child("Tab: ô tiếp · Enter: lưu · Esc: đóng"))
            .child(dialogs::button("cfg-cancel", "Đóng", false, cx, |ws: &mut Workspace, window, cx| ws.close_config_editor(window, cx)))
            .child(dialogs::button("cfg-run", "Lưu & ▶ Run", false, cx, |ws: &mut Workspace, window, cx| {
                if ws.save_config_editor(cx) {
                    ws.close_config_editor(window, cx);
                    ws.run_active(false, window, cx)
                }
            }))
            .child(dialogs::button("cfg-debug", "Lưu & 🐞 Debug", false, cx, |ws: &mut Workspace, window, cx| {
                if ws.save_config_editor(cx) {
                    ws.close_config_editor(window, cx);
                    ws.run_active(true, window, cx)
                }
            }))
            .child(dialogs::button("cfg-save", "Lưu", true, cx, |ws: &mut Workspace, window, cx| {
                if ws.save_config_editor(cx) {
                    ws.close_config_editor(window, cx)
                }
            }));
        let dialog = dialogs::modal("cfg-body", "Cấu hình Run/Debug", 1000., body, footer)
            .on_action(cx.listener(|ws, _: &DialogConfirm, window, cx| {
                if ws.save_config_editor(cx) {
                    ws.close_config_editor(window, cx)
                }
            }))
            .on_action(cx.listener(|ws, _: &DialogCancel, window, cx| ws.close_config_editor(window, cx)))
            .on_action(cx.listener(|ws, _: &DialogNext, window, cx| {
                if let Some(ed) = ws.config_editor.as_ref() {
                    dialogs::focus_step(&ed.fields(), true, window, cx)
                }
            }))
            .on_action(cx.listener(|ws, _: &DialogPrev, window, cx| {
                if let Some(ed) = ws.config_editor.as_ref() {
                    dialogs::focus_step(&ed.fields(), false, window, cx)
                }
            }));
        Some(dialog.into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spring_profile_toggle() {
        let vm = toggle_profile("-Xmx1g", "dev");
        assert_eq!(vm, "-Xmx1g -Dspring.profiles.active=dev");
        let vm = toggle_profile(&vm, "local");
        assert_eq!(active_profiles(&vm), ["dev", "local"]);
        assert_eq!(toggle_profile(&toggle_profile(&vm, "dev"), "local"), "-Xmx1g");
    }

    #[test]
    fn env_text_roundtrip_like_nvim_form() {
        let env = parse_env(" TZ = Asia/Ho_Chi_Minh ,SPRING_PROFILES_ACTIVE=dev,bad key=x,=y");
        assert_eq!(env.len(), 2);
        assert_eq!(env["TZ"], "Asia/Ho_Chi_Minh");
        assert_eq!(format_env(&env), "SPRING_PROFILES_ACTIVE=dev,TZ=Asia/Ho_Chi_Minh");
        assert!(parse_env("").is_empty());
    }
}
