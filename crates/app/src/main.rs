//! ChoDiLamIDE (Rust) - `chodilamide-ide [project-dir]`. Without an argument it restores the
//! projects that were open (projects.json, shared with the WezTerm shell) or shows the chooser.
mod actions;
mod bp_dialog;
mod config_dialog;
mod context_menu;
mod debug;
mod debug_values;
mod dialogs;
mod editor_view;
mod icons;
mod ui_icons;
mod input;
mod keymap;
mod jsf_support;
mod spring_support;
mod git_support;
mod git_panel;
mod clone_panel;
mod new_project;
mod git_log;
mod git_window;
mod git_style;
mod db_view;
mod code_tools;
mod http_client;
mod coverage;
mod vim_mode;
mod replace_dialog;
mod layout;
mod lsp_editor;
mod lsp_workspace;
mod maven_panel;
mod picker;
mod projects;
mod run_panel;
mod search;
mod settings;
mod settings_dialog;
mod terminal;
mod shell;
mod theme;
mod tree;
mod watcher;
#[cfg(test)]
mod ui_tests;
mod workspace;

use gpui::{App, Application, Bounds, Focusable, TitlebarOptions, WindowBounds, WindowOptions, prelude::*, px, size};
use std::path::PathBuf;

fn main() {
    let arg = std::env::args().nth(1).map(PathBuf::from).map(|p| std::path::absolute(&p).unwrap_or(p));
    let mut open: Vec<PathBuf> = projects::read_open().into_iter().map(|p| PathBuf::from(p.cwd)).filter(|p| p.is_dir()).collect();
    if let Some(a) = &arg {
        if !open.contains(a) {
            open.push(a.clone());
        }
    }
    let (settings, settings_error) = settings::Settings::load();

    Application::new().run(move |cx: &mut App| {
        actions::bind_editor_keys(cx);
        actions::bind_workspace_keys(cx);
        input::bind_keys(cx);
        picker::bind_keys(cx);
        lsp_editor::bind_keys(cx);
        shell::bind_keys(cx);
        dialogs::bind_keys(cx);
        db_view::bind_keys(cx);
        workspace::install_esc(cx);
        picker::install_capture(cx);
        vim_mode::install(cx);
        // user shortcuts (keymap.toml) replace the defaults bound above
        for e in keymap::apply(cx) {
            eprintln!("keymap.toml: {e}");
        }
        terminal::install_key_interceptor(cx);
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        // Windows has no JetBrains Mono (the default editor / UI font): ship it in the exe
        #[cfg(windows)]
        {
            let fonts: Vec<std::borrow::Cow<'static, [u8]>> = vec![
                std::borrow::Cow::Borrowed(include_bytes!("../assets/fonts/JetBrainsMono-Light.ttf")),
                std::borrow::Cow::Borrowed(include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf")),
                std::borrow::Cow::Borrowed(include_bytes!("../assets/fonts/JetBrainsMono-Medium.ttf")),
                std::borrow::Cow::Borrowed(include_bytes!("../assets/fonts/JetBrainsMono-SemiBold.ttf")),
                std::borrow::Cow::Borrowed(include_bytes!("../assets/fonts/JetBrainsMono-Bold.ttf")),
                std::borrow::Cow::Borrowed(include_bytes!("../assets/fonts/JetBrainsMono-Italic.ttf")),
            ];
            if let Err(e) = cx.text_system().add_fonts(fonts) {
                eprintln!("JetBrains Mono: {e}");
            }
        }
        let bounds = Bounds::centered(None, size(px(1400.), px(900.)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions { title: Some("ChoDiLamIDE".into()), ..Default::default() }),
                    app_id: Some("chodilamide-ide".into()),
                    // always see-through: with transparency off the theme paints opaque colors anyway
                    window_background: gpui::WindowBackgroundAppearance::Transparent,
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| shell::Shell::new(settings.clone(), settings_error.clone(), open.clone(), arg.clone(), window, cx)),
            )
            .expect("open window");
        window
            .update(cx, |shell, window, cx| {
                cx.activate(true);
                // closing the window asks about unsaved files in every project first
                let entity = cx.entity();
                window.on_window_should_close(cx, move |window, cx| {
                    entity.update(cx, |s, cx| s.quit(window, cx));
                    false
                });
                let _ = shell.focus_handle(cx);
            })
            .ok();
    });
}
