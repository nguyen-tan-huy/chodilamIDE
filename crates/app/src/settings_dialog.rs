//! Settings dialog (Ctrl+Alt+S): theme, fonts, sizes, jdtls memory. Applied live to every
//! open project and saved to ~/.config/chodilamide/settings.toml.
use crate::dialogs::{self, DialogCancel, DialogConfirm, DialogNext, DialogPrev, Field};
use crate::settings::Settings;
use crate::shell::Shell;
use crate::theme::{self, c};
use gpui::{AnyElement, Context, Focusable, SharedString, Window, div, prelude::*};

const WEIGHTS: &[&str] = &["Light", "Normal", "Medium", "SemiBold", "Bold"];

pub struct SettingsEditor {
    theme: usize,
    theme_select: dialogs::Select,
    /// 0 = follow the app theme, i = THEMES[i - 1]
    editor_theme: usize,
    editor_theme_select: dialogs::Select,
    font_weight: usize,
    ui_font_weight: usize,
    /// 0 left, 1 right, 2 hidden
    tree_pos: usize,
    /// 0 bottom, 1 right, 2 left
    panel_pos: usize,
    /// every installed font (not only monospace)
    fonts: Vec<String>,
    font_select: dialogs::Select,
    ui_font_select: dialogs::Select,
    log_font_select: dialogs::Select,
    log_font: Field,
    log_font_size: Field,
    /// default JDK of the active project (maven-jdk.json, shared with nvim)
    project: Option<String>,
    jdks: Vec<(String, Option<String>)>,
    jdk: usize,
    jdk_select: dialogs::Select,
    font: Field,
    font_size: Field,
    ui_font: Field,
    ui_font_size: Field,
    line_height: Field,
    tab_width: Field,
    tree_width: Field,
    panel_height: Field,
    xmx: Field,
    xms: Field,
    jdtls_on: usize,
    /// 0 = transparent background on
    glass_on: usize,
    /// Java & Maven for the whole app: (label, home) with None = automatic
    app_jdks: Vec<(String, Option<String>)>,
    app_jdk: usize,
    app_jdk_select: dialogs::Select,
    mavens: Vec<(String, Option<String>)>,
    maven: usize,
    maven_select: dialogs::Select,
    use_wrapper: usize,
    auto_download: usize,
    /// 0 IntelliJ, 1 Vim
    keymap: usize,
    vim_keys: Vec<String>,
    error: Option<String>,
    /// the category shown (index into PAGES)
    page: usize,
    /// the page area: keeps Enter / Esc inside the dialog on pages without a text field
    focus: gpui::FocusHandle,
}

/// Categories of the dialog (IntelliJ Settings: list on the left, the page on the right).
const PAGES: &[(&str, crate::ui_icons::Ico, &str)] = &[
    ("Giao diện", crate::ui_icons::Ico::Eye, "Theme, font và cỡ chữ của app"),
    ("Editor", crate::ui_icons::Ico::Commit, "Theme màu code, font, dòng, tab"),
    ("Bố cục", crate::ui_icons::Ico::Folder, "Vị trí và kích thước cây project, panel"),
    ("Console", crate::ui_icons::Ico::Terminal, "Log của Run / Debug / Test"),
    ("Phím tắt", crate::ui_icons::Ico::Search, "Kiểu phím IntelliJ / Vim, vimrc, phím tắt app"),
    ("Java & Maven", crate::ui_icons::Ico::Maven, "JDK, Maven, tự tải công cụ"),
    ("jdtls", crate::ui_icons::Ico::Gear, "Máy chủ ngôn ngữ Java: bật/tắt, bộ nhớ"),
];

const AUTO_JDK: &str = "Tự động (JDK app tải, không có thì của OS)";
const AUTO_MAVEN: &str = "Tự động (mvn trong PATH, không có thì Maven của app)";

/// (label, home) lists for the Java & Maven section; a saved path that is gone stays listed.
fn app_jdk_list(current: &str) -> Vec<(String, Option<String>)> {
    let mut v = vec![(AUTO_JDK.to_string(), None)];
    v.extend(toolchain::find_jdks().into_iter().map(|j| (j.label(), Some(j.path.display().to_string()))));
    if !current.is_empty() && !v.iter().any(|(_, p)| p.as_deref() == Some(current)) {
        v.push((format!("{current}  (không thấy)"), Some(current.to_string())));
    }
    v
}

fn maven_list(current: &str) -> Vec<(String, Option<String>)> {
    let mut v = vec![(AUTO_MAVEN.to_string(), None)];
    v.extend(toolchain::managed::find_mavens().into_iter().map(|m| (m.label(), Some(m.home.display().to_string()))));
    if !current.is_empty() && !v.iter().any(|(_, p)| p.as_deref() == Some(current)) {
        v.push((format!("{current}  (không thấy)"), Some(current.to_string())));
    }
    v
}

fn index_of(list: &[(String, Option<String>)], current: &str) -> usize {
    list.iter().position(|(_, p)| p.as_deref().unwrap_or("") == current).unwrap_or(0)
}

impl SettingsEditor {
    /// Tab order (font names are picked from the dropdowns).
    fn fields(&self) -> Vec<&Field> {
        // only the fields of the page shown can take the focus
        match self.page {
            0 => vec![&self.ui_font_size],
            1 => vec![&self.font_size, &self.line_height, &self.tab_width],
            2 => vec![&self.tree_width, &self.panel_height],
            3 => vec![&self.log_font_size],
            6 => vec![&self.xmx, &self.xms],
            _ => vec![],
        }
    }

    /// Form → Settings, validating numbers and heap sizes.
    fn read(&self, base: &Settings, cx: &gpui::App) -> Result<Settings, String> {
        let num = |f: &Field, min: f32, max: f32| -> Result<f32, String> {
            let t = f.text(cx).replace(',', ".");
            let v: f32 = t.parse().map_err(|_| format!("{}: '{t}' không phải số", f.label))?;
            if v < min || v > max {
                return Err(format!("{}: cần trong khoảng {min}–{max}", f.label));
            }
            Ok(v)
        };
        let heap = |f: &Field| -> Result<String, String> {
            let t = f.text(cx);
            let ok = t.len() >= 2 && t[..t.len() - 1].chars().all(|c| c.is_ascii_digit()) && t.ends_with(['m', 'M', 'g', 'G']);
            if ok { Ok(t) } else { Err(format!("{}: dạng 512m hoặc 2G", f.label)) }
        };
        let font = |f: &Field| -> Result<String, String> {
            let t = f.text(cx);
            if t.is_empty() { Err(format!("{}: không được trống", f.label)) } else { Ok(t) }
        };
        Ok(Settings {
            theme: theme::THEMES[self.theme].to_string(),
            font_family: font(&self.font)?,
            font_size: num(&self.font_size, 6., 40.)?,
            ui_font_family: font(&self.ui_font)?,
            ui_font_size: num(&self.ui_font_size, 6., 32.)?,
            line_height: num(&self.line_height, 1., 3.)?,
            tab_width: num(&self.tab_width, 1., 16.)? as usize,
            tree_width: num(&self.tree_width, 120., 800.)?,
            panel_height: num(&self.panel_height, 100., 1200.)?,
            jdtls_xmx: heap(&self.xmx)?,
            jdtls_xms: heap(&self.xms)?,
            jdtls_enabled: self.jdtls_on == 0,
            glass: self.glass_on == 0,
            editor_theme: if self.editor_theme == 0 { theme::EDITOR_FOLLOW.to_string() } else { theme::THEMES[self.editor_theme - 1].to_string() },
            font_weight: WEIGHTS[self.font_weight].to_string(),
            ui_font_weight: WEIGHTS[self.ui_font_weight].to_string(),
            tree_side: if self.tree_pos == 1 { "right".into() } else { "left".into() },
            tree_visible: self.tree_pos != 2,
            panel_side: ["bottom", "right", "left"][self.panel_pos].to_string(),
            log_font_family: font(&self.log_font)?,
            log_font_size: num(&self.log_font_size, 6., 32.)?,
            jdk_home: self.app_jdks.get(self.app_jdk).and_then(|(_, p)| p.clone()).unwrap_or_default(),
            maven_home: self.mavens.get(self.maven).and_then(|(_, p)| p.clone()).unwrap_or_default(),
            maven_use_wrapper: self.use_wrapper == 0,
            auto_download_tools: self.auto_download == 0,
            keymap_mode: if self.keymap == 1 { "Vim".into() } else { "IntelliJ".into() },
            vim_ctrl_keys: self.vim_keys.clone(),
            ..base.clone()
        })
    }
}

impl Shell {
    pub(crate) fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let s = self.settings().clone();
        let mut fonts: Vec<String> = window.text_system().all_font_names().into_iter().filter(|n| !n.starts_with('.')).collect();
        fonts.sort_by_key(|f| f.to_lowercase());
        fonts.dedup();
        let project = self.active_root(cx);
        let current_jdk = project.as_deref().and_then(runner::config::maven_jdk);
        let mut jdks: Vec<(String, Option<String>)> = vec![("Theo JDK mặc định của app".into(), None)];
        jdks.extend(toolchain::find_jdks().into_iter().map(|j| (j.label(), Some(j.path.display().to_string()))));
        if let Some(cur) = current_jdk.as_ref().filter(|c| !jdks.iter().any(|(_, p)| p.as_ref() == Some(*c))) {
            jdks.push((cur.clone(), Some(cur.clone())));
        }
        let jdk = jdks.iter().position(|(_, p)| *p == current_jdk).unwrap_or(0);
        let app_jdks = app_jdk_list(&s.jdk_home);
        let mavens = maven_list(&s.maven_home);
        let ed = SettingsEditor {
            app_jdk: index_of(&app_jdks, &s.jdk_home),
            app_jdks,
            app_jdk_select: dialogs::Select::new(cx),
            maven: index_of(&mavens, &s.maven_home),
            mavens,
            maven_select: dialogs::Select::new(cx),
            use_wrapper: if s.maven_use_wrapper { 0 } else { 1 },
            keymap: if s.keymap_mode == "Vim" { 1 } else { 0 },
            vim_keys: s.vim_ctrl_keys.clone(),
            auto_download: if s.auto_download_tools { 0 } else { 1 },
            theme: theme::theme_index(&s.theme),
            theme_select: dialogs::Select::new(cx),
            editor_theme: if s.editor_theme == theme::EDITOR_FOLLOW { 0 } else { theme::theme_index(&s.editor_theme) + 1 },
            editor_theme_select: dialogs::Select::new(cx),
            font_weight: WEIGHTS.iter().position(|w| *w == s.font_weight).unwrap_or(1),
            ui_font_weight: WEIGHTS.iter().position(|w| *w == s.ui_font_weight).unwrap_or(1),
            tree_pos: if !s.tree_visible { 2 } else if s.tree_side == "right" { 1 } else { 0 },
            panel_pos: match s.panel_side.as_str() { "right" => 1, "left" => 2, _ => 0 },
            fonts,
            font_select: dialogs::Select::new(cx),
            ui_font_select: dialogs::Select::new(cx),
            log_font_select: dialogs::Select::new(cx),
            log_font: Field::new("Font", "", &s.log_font_family, cx),
            log_font_size: Field::new("Cỡ chữ (pt)", "10.5", &s.log_font_size.to_string(), cx).narrow(90.),
            project,
            jdks,
            jdk,
            jdk_select: dialogs::Select::new(cx),
            font: Field::new("Font", "", &s.font_family, cx),
            font_size: Field::new("Cỡ chữ (pt)", "12", &s.font_size.to_string(), cx).narrow(90.),
            ui_font: Field::new("Font", "", &s.ui_font_family, cx),
            ui_font_size: Field::new("Cỡ chữ (pt)", "10.5", &s.ui_font_size.to_string(), cx).narrow(90.),
            line_height: Field::new("Khoảng cách dòng", "1.45", &s.line_height.to_string(), cx).narrow(90.),
            tab_width: Field::new("Độ rộng Tab", "4", &s.tab_width.to_string(), cx).narrow(90.),
            tree_width: Field::new("Độ rộng cây project (px)", "280", &s.tree_width.to_string(), cx).narrow(90.),
            panel_height: Field::new("Chiều cao panel dưới (px)", "300", &s.panel_height.to_string(), cx).narrow(90.),
            xmx: Field::new("RAM tối đa (-Xmx)", "2G", &s.jdtls_xmx, cx).narrow(90.),
            xms: Field::new("RAM ban đầu (-Xms)", "256m", &s.jdtls_xms, cx).narrow(90.),
            jdtls_on: if s.jdtls_enabled { 0 } else { 1 },
            glass_on: if s.glass { 0 } else { 1 },
            error: None,
            page: 0,
            focus: cx.focus_handle(),
        };
        // typing in a font filter re-renders the list
        for f in [ed.font_select.filter.clone(), ed.ui_font_select.filter.clone(), ed.log_font_select.filter.clone(), ed.jdk_select.filter.clone(), ed.app_jdk_select.filter.clone(), ed.maven_select.filter.clone(), ed.theme_select.filter.clone(), ed.editor_theme_select.filter.clone()] {
            cx.subscribe(&f, |_, _, _: &crate::input::InputEvent, cx| cx.notify()).detach();
        }
        crate::dialogs::focus_later(ed.ui_font_size.input.read(cx).focus_handle(cx), window, cx);
        self.settings_editor = Some(ed);
        cx.notify();
    }

    fn close_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings_editor = None;
        self.refocus(window, cx);
        cx.notify();
    }

    fn save_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let base = self.settings().clone();
        let Some(ed) = self.settings_editor.as_mut() else { return };
        let new = match ed.read(&base, cx) {
            Ok(s) => s,
            Err(e) => {
                ed.error = Some(e);
                cx.notify();
                return;
            }
        };
        if let Some(root) = ed.project.clone() {
            let path = ed.jdks.get(ed.jdk).and_then(|(_, p)| p.clone());
            if runner::config::maven_jdk(&root) != path {
                if let Err(e) = runner::config::set_maven_jdk(&root, path.as_deref()) {
                    ed.error = Some(format!("Không ghi được maven-jdk.json: {e}"));
                    cx.notify();
                    return;
                }
            }
        }
        if let Err(e) = new.save() {
            ed.error = Some(format!("Không ghi được settings.toml: {e}"));
            cx.notify();
            return;
        }
        let memory_changed = new.jdtls_xmx != self.settings().jdtls_xmx || new.jdtls_xms != self.settings().jdtls_xms;
        self.apply_settings(new, cx);
        self.settings_editor = None;
        self.refocus(window, cx);
        if memory_changed {
            self.set_message("Đã lưu. RAM jdtls mới áp dụng cho project mở sau (hoặc mở lại app).", cx);
        }
        cx.notify();
    }

    pub(crate) fn render_settings(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let ed = self.settings_editor.as_ref()?;
        let themes: Vec<String> = theme::THEMES.iter().map(|t| t.to_string()).collect();
        let (theme_i, jdtls_on, error) = (ed.theme, ed.jdtls_on, ed.error.clone());
        let fonts = ed.fonts.clone();
        let (editor_font, ui_font) = (ed.font.text(cx), ed.ui_font.text(cx));
        let ui_font_row = dialogs::select_row(
            "Font",
            "sel-ui-font",
            &ui_font,
            &ed.ui_font_select,
            &fonts,
            true,
            cx,
            |s: &mut Shell, window, cx| s.toggle_select(Which::Ui, window, cx),
            |s: &mut Shell, name, window, cx| s.pick_select(Which::Ui, name, window, cx),
        );
        let editor_font_row = dialogs::select_row(
            "Font",
            "sel-editor-font",
            &editor_font,
            &ed.font_select,
            &fonts,
            true,
            cx,
            |s: &mut Shell, window, cx| s.toggle_select(Which::Editor, window, cx),
            |s: &mut Shell, name, window, cx| s.pick_select(Which::Editor, name, window, cx),
        );
        let log_font = ed.log_font.text(cx);
        let log_font_row = dialogs::select_row(
            "Font",
            "sel-log-font",
            &log_font,
            &ed.log_font_select,
            &fonts,
            true,
            cx,
            |s: &mut Shell, window, cx| s.toggle_select(Which::Log, window, cx),
            |s: &mut Shell, name, window, cx| s.pick_select(Which::Log, name, window, cx),
        );
        let jdk_labels: Vec<String> = ed.jdks.iter().map(|(l, _)| l.clone()).collect();
        let project_name = ed.project.as_deref().map(|r| crate::tree::file_name(std::path::Path::new(r)));
        let jdk_row = project_name.as_ref().map(|_| {
            dialogs::select_row(
                "JDK mặc định",
                "sel-jdk",
                &jdk_labels[ed.jdk],
                &ed.jdk_select,
                &jdk_labels,
                false,
                cx,
                |s: &mut Shell, window, cx| s.toggle_select(Which::Jdk, window, cx),
                |s: &mut Shell, name, window, cx| s.pick_select(Which::Jdk, name, window, cx),
            )
        });
        let app_jdk_labels: Vec<String> = ed.app_jdks.iter().map(|(l, _)| l.clone()).collect();
        let app_jdk_row = dialogs::select_row(
            "JDK mặc định",
            "sel-app-jdk",
            &app_jdk_labels[ed.app_jdk],
            &ed.app_jdk_select,
            &app_jdk_labels,
            false,
            cx,
            |s: &mut Shell, window, cx| s.toggle_select(Which::AppJdk, window, cx),
            |s: &mut Shell, name, window, cx| s.pick_select(Which::AppJdk, name, window, cx),
        );
        let maven_labels: Vec<String> = ed.mavens.iter().map(|(l, _)| l.clone()).collect();
        let maven_row = dialogs::select_row(
            "Maven",
            "sel-maven",
            &maven_labels[ed.maven],
            &ed.maven_select,
            &maven_labels,
            false,
            cx,
            |s: &mut Shell, window, cx| s.toggle_select(Which::Maven, window, cx),
            |s: &mut Shell, name, window, cx| s.pick_select(Which::Maven, name, window, cx),
        );
        // "21 ✓" = already installed (clicking it again just selects it)
        let installed: Vec<u32> = toolchain::find_jdks().iter().map(|j| j.major).collect();
        let jdk_downloads: Vec<String> =
            toolchain::managed::JDK_CHOICES.iter().map(|m| if installed.contains(m) { format!("{m} ✓") } else { m.to_string() }).collect();
        let has_managed_maven = toolchain::managed::find_mavens().iter().any(|m| m.managed && m.version == toolchain::managed::MAVEN_VERSION);
        let maven_download = vec![format!("{}{}", toolchain::managed::MAVEN_VERSION, if has_managed_maven { " ✓" } else { "" })];
        let progress = toolchain::managed::progress();
        let ctrl_chips: Vec<String> = crate::vim_mode::CTRL_KEYS
            .iter()
            .map(|(k, what)| {
                let on = ed.vim_keys.iter().any(|x| x == k);
                format!("{} {} ({})", if on { "✓" } else { "·" }, crate::keymap::pretty(k), what)
            })
            .collect();
        let weights: Vec<String> = WEIGHTS.iter().map(|w| w.to_string()).collect();
        let theme_row = dialogs::select_row(
            "Theme",
            "sel-theme",
            &themes[theme_i],
            &ed.theme_select,
            &themes,
            false,
            cx,
            |s: &mut Shell, window, cx| s.toggle_select(Which::Theme, window, cx),
            |s: &mut Shell, name, window, cx| s.pick_select(Which::Theme, name, window, cx),
        );
        let mut editor_themes = vec![theme::EDITOR_FOLLOW.to_string()];
        editor_themes.extend(themes.iter().cloned());
        let editor_theme_row = dialogs::select_row(
            "Theme editor",
            "sel-editor-theme",
            &editor_themes[ed.editor_theme],
            &ed.editor_theme_select,
            &editor_themes,
            false,
            cx,
            |s: &mut Shell, window, cx| s.toggle_select(Which::EditorTheme, window, cx),
            |s: &mut Shell, name, window, cx| s.pick_select(Which::EditorTheme, name, window, cx),
        );
        let row = |f: &Field| f.row().into_any_element();
        let hint = |t: String| div().pl_2().text_size(gpui::px(11.)).text_color(c(theme::FG_DIM)).child(SharedString::from(t)).into_any_element();
        let page = ed.page;
        let mut items: Vec<AnyElement> = Vec::new();
        match page {
            0 => {
                items.push(theme_row.into_any_element());
                items.push(dialogs::choice_row("Nền trong suốt", &["Bật".to_string(), "Tắt".to_string()], ed.glass_on, "set-glass", cx, |s: &mut Shell, i, _| {
                    if let Some(ed) = s.settings_editor.as_mut() {
                        ed.glass_on = i;
                    }
                }).into_any_element());
                items.push(ui_font_row.into_any_element());
                items.push(row(&ed.ui_font_size));
                items.push(dialogs::choice_row("Độ đậm chữ", &weights, ed.ui_font_weight, "set-ui-weight", cx, |s: &mut Shell, i, _| {
                    if let Some(ed) = s.settings_editor.as_mut() {
                        ed.ui_font_weight = i;
                    }
                }).into_any_element());
            }
            1 => {
                items.push(editor_theme_row.into_any_element());
                items.push(editor_font_row.into_any_element());
                items.push(row(&ed.font_size));
                items.push(dialogs::choice_row("Độ đậm chữ", &weights, ed.font_weight, "set-weight", cx, |s: &mut Shell, i, _| {
                    if let Some(ed) = s.settings_editor.as_mut() {
                        ed.font_weight = i;
                    }
                }).into_any_element());
                items.push(row(&ed.line_height));
                items.push(row(&ed.tab_width));
            }
            2 => {
                items.push(dialogs::choice_row("Cây project", &["Trái".to_string(), "Phải".to_string(), "Ẩn".to_string()], ed.tree_pos, "set-tree-pos", cx, |s: &mut Shell, i, _| {
                    if let Some(ed) = s.settings_editor.as_mut() {
                        ed.tree_pos = i;
                    }
                }).into_any_element());
                items.push(row(&ed.tree_width));
                items.push(dialogs::choice_row("Panel Run/Debug/Git", &["Dưới".to_string(), "Phải".to_string(), "Trái".to_string()], ed.panel_pos, "set-panel-pos", cx, |s: &mut Shell, i, _| {
                    if let Some(ed) = s.settings_editor.as_mut() {
                        ed.panel_pos = i;
                    }
                }).into_any_element());
                items.push(row(&ed.panel_height));
            }
            3 => {
                items.push(log_font_row.into_any_element());
                items.push(row(&ed.log_font_size));
            }
            4 => {
                items.push(dialogs::choice_row("Kiểu phím", &["IntelliJ".to_string(), "Vim".to_string()], ed.keymap, "set-keymap", cx, |s: &mut Shell, i, _| {
                    if let Some(ed) = s.settings_editor.as_mut() {
                        ed.keymap = i;
                    }
                }).into_any_element());
                if ed.keymap == 1 {
                    items.push(dialogs::choice_row("Phím Ctrl cho Vim", &ctrl_chips, usize::MAX, "set-vimkeys", cx, |s: &mut Shell, i, _| {
                        if let Some(ed) = s.settings_editor.as_mut() {
                            let k = crate::vim_mode::CTRL_KEYS[i].0.to_string();
                            match ed.vim_keys.iter().position(|x| *x == k) {
                                Some(p) => {
                                    ed.vim_keys.remove(p);
                                }
                                None => ed.vim_keys.push(k),
                            }
                        }
                    }).into_any_element());
                    items.push(hint("✓ = phím đó thuộc Vim, còn lại là phím tắt của IDE. Alt/F-key luôn thuộc IDE (Ctrl+B, Alt+Enter, Shift+F6… vẫn chạy).".into()));
                }
                items.push(
                    div()
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .gap_2()
                        .pt_2()
                        .child(dialogs::button("set-vimrc", "Sửa vimrc (map, leader)…", false, cx, |s: &mut Shell, window, cx| s.edit_vimrc(window, cx)))
                        .child(dialogs::button("set-appkeys", "Phím tắt app…", false, cx, |s: &mut Shell, window, cx| s.edit_app_keys(window, cx)))
                        .into_any_element(),
                );
                items.push(hint("Phím tắt app: tìm lệnh trong Find Action, Ctrl+Enter để đổi phím.".into()));
            }
            5 => {
                items.push(dialogs::section("Toàn app").into_any_element());
                items.push(app_jdk_row.into_any_element());
                items.push(dialogs::choice_row("Tải JDK (Temurin)", &jdk_downloads, usize::MAX, "dl-jdk", cx, |s: &mut Shell, i, cx| {
                    s.download_tool(Download::Jdk(toolchain::managed::JDK_CHOICES[i]), cx)
                }).into_any_element());
                items.push(maven_row.into_any_element());
                items.push(dialogs::choice_row("Tải Maven", &maven_download, usize::MAX, "dl-maven", cx, |s: &mut Shell, _, cx| s.download_tool(Download::Maven, cx)).into_any_element());
                items.push(dialogs::choice_row("Maven wrapper (mvnw)", &["Dùng khi project có".to_string(), "Bỏ qua".to_string()], ed.use_wrapper, "set-wrapper", cx, |s: &mut Shell, i, _| {
                    if let Some(ed) = s.settings_editor.as_mut() {
                        ed.use_wrapper = i;
                    }
                }).into_any_element());
                items.push(dialogs::choice_row("Tự tải khi thiếu", &["Bật".to_string(), "Tắt".to_string()], ed.auto_download, "set-autodl", cx, |s: &mut Shell, i, _| {
                    if let Some(ed) = s.settings_editor.as_mut() {
                        ed.auto_download = i;
                    }
                }).into_any_element());
                items.push(
                    div().pl_2().text_size(gpui::px(11.)).text_color(c(if progress.is_some() { theme::ACCENT } else { theme::FG_DIM })).child(SharedString::from(
                        progress.unwrap_or_else(|| format!("Tải về {}. Mở project khi máy chưa có JDK 21+ / Maven thì app tự tải.", toolchain::tools_dir().display())),
                    )).into_any_element(),
                );
                if let Some(name) = project_name {
                    items.push(dialogs::section_owned(format!("Project {name}")).into_any_element());
                    items.extend(jdk_row.map(|r| r.into_any_element()));
                    items.push(hint("Dùng cho Maven, build và Run/Debug/Test khi cấu hình chạy không chọn JDK riêng (chung file với nvim)".into()));
                }
            }
            _ => {
                items.push(dialogs::choice_row("jdtls", &["Bật".to_string(), "Tắt".to_string()], jdtls_on, "set-jdtls", cx, |s: &mut Shell, i, _| {
                    if let Some(ed) = s.settings_editor.as_mut() {
                        ed.jdtls_on = i;
                    }
                }).into_any_element());
                items.push(row(&ed.xmx));
                items.push(row(&ed.xms));
                items.push(hint("Xmx lớn hơn giúp project nhiều module nhanh hơn; đổi xong mở lại project để áp dụng.".into()));
            }
        }
        // left: the categories; right: the page with its title and description
        let nav = div().flex().flex_col().flex_none().w(gpui::px(200.)).gap_0p5().pr_3().border_r_1().border_color(c(theme::BORDER)).children(PAGES.iter().enumerate().map(|(i, (name, ico, _))| {
            let on = i == page;
            div()
                .id(("set-page", i))
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .h(gpui::px(30.))
                .px_2()
                .rounded_md()
                .text_color(c(if on { theme::FG } else { theme::FG_DIM }))
                .when(on, |d| d.bg(c(theme::ROW_ACTIVE)))
                .hover(|s| s.bg(c(theme::ROW_HOVER)).text_color(c(theme::FG)))
                .child(crate::ui_icons::icon(*ico, 15., c(if on { theme::ACCENT } else { theme::FG_DIM })))
                .child(*name)
                .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |s, _, window, cx| {
                    if let Some(ed) = s.settings_editor.as_mut() {
                        ed.page = i;
                        // the old page's field is gone: focus this page's first one
                        let f = ed.fields().first().map(|f| f.input.read(cx).focus_handle(cx)).unwrap_or_else(|| ed.focus.clone());
                        crate::dialogs::focus_later(f, window, cx);
                        // a dropdown of the previous page must not stay open
                        for w in [Which::Theme, Which::EditorTheme, Which::Ui, Which::Editor, Which::Log, Which::Jdk, Which::AppJdk, Which::Maven] {
                            ed.select(w).open = false;
                        }
                    }
                    cx.notify()
                }))
        }));
        let (title, desc) = (PAGES[page].0, PAGES[page].2);
        let content = div()
            .track_focus(&ed.focus)
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .gap_1()
            .pl_4()
            .child(div().text_size(gpui::px(16.)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(theme::FG)).child(title))
            .child(div().pb_2().text_color(c(theme::FG_DIM)).child(desc))
            .children(items);
        let body = div().flex().flex_row().min_h(gpui::px(440.)).child(nav).child(content);
        let footer = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .w_full()
            .child(match error {
                Some(e) => div().flex_1().text_color(c(theme::ERROR)).child(SharedString::from(e)),
                None => div().flex_1().whitespace_nowrap().overflow_hidden().text_color(c(theme::FG_DIM)).child("Lưu vào ~/.config/chodilamide/settings.toml"),
            })
            .child(dialogs::button("set-cancel", "Hủy (Esc)", false, cx, |s: &mut Shell, window, cx| s.cancel_settings(window, cx)))
            .child(dialogs::button("set-save", "Lưu (Enter)", true, cx, |s: &mut Shell, window, cx| s.save_settings(window, cx)));
        let dialog = dialogs::modal("settings-body", "Cài đặt", 980., body, footer)
            .on_action(cx.listener(|s, _: &DialogConfirm, window, cx| s.save_settings(window, cx)))
            .on_action(cx.listener(|s, _: &DialogCancel, window, cx| s.cancel_settings(window, cx)))
            .on_action(cx.listener(|s, _: &DialogNext, window, cx| {
                if let Some(ed) = s.settings_editor.as_ref() {
                    dialogs::focus_step(&ed.fields(), true, window, cx)
                }
            }))
            .on_action(cx.listener(|s, _: &DialogPrev, window, cx| {
                if let Some(ed) = s.settings_editor.as_ref() {
                    dialogs::focus_step(&ed.fields(), false, window, cx)
                }
            }));
        Some(dialog.into_any_element())
    }

    fn cancel_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let saved = self.settings().theme.clone();
        theme::set_theme(&saved);
        let saved_editor = self.settings().editor_theme.clone();
        theme::set_editor_theme(&saved_editor);
        cx.refresh_windows();
        self.close_settings(window, cx)
    }

    #[cfg(test)]
    pub(crate) fn font_select_offset(&self) -> Option<gpui::Point<gpui::Pixels>> {
        self.settings_editor.as_ref().map(|e| e.font_select.scroll.0.borrow().base_handle.offset())
    }

    /// Screenshot checks: open the dialog on a category.
    pub(crate) fn debug_settings_page(&mut self, page: usize) {
        if let Some(ed) = self.settings_editor.as_mut() {
            ed.page = page;
        }
    }

    pub(crate) fn debug_open_font_select(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ed) = self.settings_editor.as_mut() {
            ed.page = 1;
        }
        self.toggle_select(Which::Editor, window, cx);
    }

    fn toggle_select(&mut self, which: Which, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.settings_editor.as_mut() else { return };
        let was = ed.select(which).open;
        for w in [Which::Editor, Which::Ui, Which::Log, Which::Jdk, Which::AppJdk, Which::Maven, Which::Theme, Which::EditorTheme] {
            ed.select(w).open = false;
        }
        let this = ed.select(which);
        this.open = !was;
        if this.open {
            this.filter.update(cx, |i, cx| i.set_value("", cx));
            crate::dialogs::focus_later(this.filter.read(cx).focus_handle(cx), window, cx);
        }
    }

    fn pick_select(&mut self, which: Which, name: String, _: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.settings_editor.as_mut() else { return };
        match which {
            Which::Editor => ed.font.set(&name, cx),
            Which::Ui => ed.ui_font.set(&name, cx),
            Which::Log => ed.log_font.set(&name, cx),
            Which::Jdk => ed.jdk = ed.jdks.iter().position(|(l, _)| *l == name).unwrap_or(ed.jdk),
            Which::AppJdk => ed.app_jdk = ed.app_jdks.iter().position(|(l, _)| *l == name).unwrap_or(ed.app_jdk),
            Which::Maven => ed.maven = ed.mavens.iter().position(|(l, _)| *l == name).unwrap_or(ed.maven),
            // themes preview immediately; Cancel restores the saved ones
            Which::Theme => {
                ed.theme = theme::THEMES.iter().position(|t| *t == name).unwrap_or(ed.theme);
                theme::set_theme(theme::THEMES[ed.theme]);
                cx.refresh_windows();
            }
            Which::EditorTheme => {
                ed.editor_theme = theme::THEMES.iter().position(|t| *t == name).map(|i| i + 1).unwrap_or(0);
                theme::set_editor_theme(&name);
                cx.refresh_windows();
            }
        }
        ed.select(which).open = false;
    }
}

#[derive(Clone, Copy)]
enum Which {
    Editor,
    Ui,
    Log,
    Jdk,
    AppJdk,
    Maven,
    Theme,
    EditorTheme,
}

impl Shell {
    /// Creates the vimrc from the template when missing, opens it in the active project.
    fn edit_vimrc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = crate::vim_mode::vimrc_path();
        if !path.exists() {
            if let Some(d) = path.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let _ = std::fs::write(&path, crate::vim_mode::VIMRC_TEMPLATE);
        }
        self.settings_editor = None;
        if let Some(ws) = self.active_workspace() {
            ws.update(cx, |w, cx| w.open_file(path, window, cx));
        }
        cx.notify();
    }

    /// App shortcuts are changed in Find Action (Ctrl+Enter on an action).
    fn edit_app_keys(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings_editor = None;
        self.refocus(window, cx);
        window.defer(cx, |window, cx| window.dispatch_action(Box::new(crate::actions::FindAction), cx));
        cx.notify();
    }
}

#[derive(Clone, Copy)]
enum Download {
    Jdk(u32),
    Maven,
}

impl Shell {
    /// Downloads in the background, then refreshes the lists and selects what was installed.
    fn download_tool(&mut self, what: Download, cx: &mut Context<Self>) {
        if toolchain::managed::progress().is_some() {
            return;
        }
        let task = cx.background_executor().spawn(async move {
            match what {
                Download::Jdk(m) => toolchain::managed::install_jdk(m).map(|p| toolchain::canonical(&p).unwrap_or(p)),
                Download::Maven => toolchain::managed::install_maven(toolchain::managed::MAVEN_VERSION).map(|m| m.home),
            }
        });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |s, cx| {
                if let Some(ed) = s.settings_editor.as_mut() {
                    let cur_jdk = ed.app_jdks.get(ed.app_jdk).and_then(|(_, p)| p.clone()).unwrap_or_default();
                    let cur_mvn = ed.mavens.get(ed.maven).and_then(|(_, p)| p.clone()).unwrap_or_default();
                    ed.app_jdks = app_jdk_list(&cur_jdk);
                    ed.mavens = maven_list(&cur_mvn);
                    ed.app_jdk = index_of(&ed.app_jdks, &cur_jdk);
                    ed.maven = index_of(&ed.mavens, &cur_mvn);
                    match (&res, what) {
                        (Ok(home), Download::Jdk(_)) => {
                            let home = home.display().to_string();
                            ed.app_jdk = index_of(&ed.app_jdks, &home);
                            let cur = ed.jdks.get(ed.jdk).and_then(|(_, p)| p.clone());
                            ed.jdks.truncate(1);
                            ed.jdks.extend(toolchain::find_jdks().into_iter().map(|j| (j.label(), Some(j.path.display().to_string()))));
                            ed.jdk = ed.jdks.iter().position(|(_, p)| p.as_deref() == Some(home.as_str())).or_else(|| ed.jdks.iter().position(|(_, p)| *p == cur)).unwrap_or(0);
                        }
                        (Ok(home), Download::Maven) => ed.maven = index_of(&ed.mavens, &home.display().to_string()),
                        (Err(e), _) => ed.error = Some(format!("Tải thất bại: {e:#}")),
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

impl SettingsEditor {
    fn select(&mut self, w: Which) -> &mut dialogs::Select {
        match w {
            Which::Editor => &mut self.font_select,
            Which::Ui => &mut self.ui_font_select,
            Which::Log => &mut self.log_font_select,
            Which::Jdk => &mut self.jdk_select,
            Which::AppJdk => &mut self.app_jdk_select,
            Which::Maven => &mut self.maven_select,
            Which::Theme => &mut self.theme_select,
            Which::EditorTheme => &mut self.editor_theme_select,
        }
    }
}
