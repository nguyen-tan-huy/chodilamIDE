//! `~/.config/chodilamide/settings.toml`. Sizes are in points like wezterm_user.lua
//! (`font_size = 12` there), converted to pixels at 96 dpi.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// Dark | Light
    pub theme: String,
    /// transparent: no background behind panels / buttons, they float on the wallpaper
    pub glass: bool,
    pub font_family: String,
    pub font_size: f32,
    pub ui_font_family: String,
    pub ui_font_size: f32,
    pub line_height: f32,
    pub tab_width: usize,
    pub tree_width: f32,
    /// jdtls heap cap (phase 0: without -Xmx jdtls may take 1/4 of RAM)
    pub jdtls_xmx: String,
    pub jdtls_xms: String,
    /// set false to open projects without starting jdtls
    pub jdtls_enabled: bool,
    /// height of the Run/Debug/Maven tool window
    pub panel_height: f32,
    /// Run/Debug console and Maven output
    pub log_font_family: String,
    pub log_font_size: f32,
    /// editor color scheme; "Theo theme app" = same as `theme`
    pub editor_theme: String,
    /// Normal | Medium | SemiBold | Bold
    pub font_weight: String,
    pub ui_font_weight: String,
    /// project tree: "left" | "right", and whether it is shown
    pub tree_side: String,
    pub tree_visible: bool,
    /// Run/Debug/Maven/Terminal panel: "bottom" | "right" | "left"
    pub panel_side: String,
    /// Java & Maven (whole app, like IntelliJ's SDK + Maven home): "" = automatic
    pub jdk_home: String,
    pub maven_home: String,
    pub maven_use_wrapper: bool,
    /// download a JDK / Maven when the machine has none
    pub auto_download_tools: bool,
    /// "IntelliJ" | "Vim"
    pub keymap_mode: String,
    /// Ctrl keys handled by Vim (the others stay IDE shortcuts)
    pub vim_ctrl_keys: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "Dark".into(),
            glass: true,
            font_family: "JetBrains Mono".into(),
            font_size: 12.0,
            ui_font_family: "JetBrains Mono".into(),
            ui_font_size: 10.5,
            line_height: 1.45,
            tab_width: 4,
            tree_width: 340.0,
            jdtls_xmx: "2G".into(),
            jdtls_xms: "256m".into(),
            jdtls_enabled: true,
            panel_height: 300.0,
            log_font_family: "JetBrains Mono".into(),
            log_font_size: 10.5,
            editor_theme: crate::theme::EDITOR_FOLLOW.into(),
            font_weight: "Normal".into(),
            ui_font_weight: "Normal".into(),
            tree_side: "left".into(),
            tree_visible: true,
            panel_side: "bottom".into(),
            jdk_home: String::new(),
            maven_home: String::new(),
            maven_use_wrapper: true,
            auto_download_tools: true,
            keymap_mode: "IntelliJ".into(),
            vim_ctrl_keys: crate::vim_mode::default_vim_keys(),
        }
    }
}

impl Settings {
    pub fn path() -> PathBuf {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).unwrap_or_default()).join(".config"));
        base.join("chodilamide/settings.toml")
    }

    pub fn load() -> (Self, Option<String>) {
        let path = Self::path();
        match std::fs::read_to_string(&path) {
            Err(_) => (Self::default(), None),
            Ok(s) => match toml::from_str(&s) {
                Ok(v) => (v, None),
                Err(e) => (Self::default(), Some(format!("{}: {e}", path.display()))),
            },
        }
    }

    /// Writes settings.toml (atomic), so the next start and other projects use it too.
    pub fn save(&self) -> anyhow::Result<()> {
        let text = format!("# ChoDiLamIDE - sửa trong app: Ctrl+Alt+S\n{}", toml::to_string_pretty(self)?);
        project_model::cache::write_atomic(&Self::path(), text.as_bytes())
    }

    /// Pushes the Java & Maven choice to toolchain (used by every Maven/java launch).
    pub fn apply_toolchain(&self) {
        let path = |s: &str| (!s.is_empty()).then(|| PathBuf::from(s));
        toolchain::managed::configure(toolchain::managed::Selection {
            jdk_home: path(&self.jdk_home),
            maven_home: path(&self.maven_home),
            use_wrapper: self.maven_use_wrapper,
            auto_download: self.auto_download_tools,
        });
    }

    pub fn font_px(&self) -> f32 {
        self.font_size * 4.0 / 3.0
    }

    pub fn weight(name: &str) -> gpui::FontWeight {
        match name {
            "Light" => gpui::FontWeight::LIGHT,
            "Medium" => gpui::FontWeight::MEDIUM,
            "SemiBold" => gpui::FontWeight::SEMIBOLD,
            "Bold" => gpui::FontWeight::BOLD,
            _ => gpui::FontWeight::NORMAL,
        }
    }

    pub fn log_font_px(&self) -> f32 {
        self.log_font_size * 4.0 / 3.0
    }

    pub fn ui_font_px(&self) -> f32 {
        self.ui_font_size * 4.0 / 3.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_roundtrip_and_old_files() {
        let s = Settings { theme: "Tokyo Night".into(), font_size: 13.5, jdtls_xmx: "3G".into(), ..Settings::default() };
        let text = toml::to_string_pretty(&s).unwrap();
        assert_eq!(toml::from_str::<Settings>(&text).unwrap(), s);
        // a file written before `theme` existed still loads, with defaults for the rest
        let old: Settings = toml::from_str("font_size = 12\n").unwrap();
        assert_eq!((old.theme.as_str(), old.font_size, old.tab_width), ("Dark", 12.0, 4));
    }
}
