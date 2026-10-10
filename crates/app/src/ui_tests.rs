//! UI flows driven like a user (mouse on real elements), headless via gpui's test platform.
use crate::settings::Settings;
use crate::tree::NodeId;
use crate::workspace::Workspace;
use gpui::{Modifiers, MouseButton, TestAppContext};
use std::path::PathBuf;

fn sel(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn fake_module(path: &PathBuf, jars: Vec<String>) -> project_model::Module {
    project_model::Module {
        path: path.display().to_string(),
        group_id: Some("vn.demo".into()),
        artifact_id: Some("demo".into()),
        version: Some("1".into()),
        packaging: "jar".into(),
        content_root: path.display().to_string(),
        source_roots: vec![],
        dependencies: vec![],
        profiles: vec![],
        in_reactor: true,
        classpath_jars: Some(jars),
    }
}

/// Right-click a jar under "Dependencies" → "Xóa khỏi ~/.m2" → confirm: the version folder
/// must be gone from the (test) local repository.
#[gpui::test]
fn delete_jar_via_tree_context_menu(cx: &mut TestAppContext) {
    let base = std::env::temp_dir().join(format!("ui-jar-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let repo = base.join("repository");
    let ver = repo.join("org/demo/lib/1.0");
    std::fs::create_dir_all(&ver).unwrap();
    let jar = ver.join("lib-1.0.jar");
    std::fs::write(&jar, "jar").unwrap();
    let root = base.join("proj");
    std::fs::create_dir_all(&root).unwrap();
    // SAFETY: only this test reads the variable
    unsafe { std::env::set_var("CHODILAMIDE_M2_REPO", &repo) };

    let settings = Settings { jdtls_enabled: false, ..Settings::default() };
    let (ws, cx) = cx.add_window_view(|_, cx| Workspace::new(root.clone(), settings, None, cx));
    cx.run_until_parked();
    ws.update(cx, |w, cx| {
        w.project = Some(project_model::Project { root: root.display().to_string(), modules: vec![fake_module(&root, vec![jar.display().to_string()])] });
        w.tree.expand(NodeId::Module(root.clone()));
        w.tree.expand(NodeId::Deps(root.clone()));
        cx.notify();
    });
    cx.run_until_parked();

    let row = cx.debug_bounds(sel("tree:lib-1.0.jar".into())).expect("jar row in the tree");
    cx.simulate_mouse_down(row.center(), MouseButton::Right, Modifiers::none());
    cx.simulate_mouse_up(row.center(), MouseButton::Right, Modifiers::none());
    cx.run_until_parked();

    let item = cx.debug_bounds(sel("menu:Xóa khỏi ~/.m2 (tải lại khi Reload)…".into())).expect("delete item in the context menu");
    cx.simulate_click(item.center(), Modifiers::none());
    cx.run_until_parked();

    let ok = cx.debug_bounds("button:cf-ok").expect("confirm dialog");
    cx.simulate_click(ok.center(), Modifiers::none());
    cx.run_until_parked();

    assert!(!ver.exists(), "jar version folder still there: {}", ver.display());
    let msg = ws.read_with(cx, |w, _| w.message.clone());
    assert!(msg.unwrap_or_default().starts_with("Đã xóa"));
    std::fs::remove_dir_all(&base).unwrap();
}

/// Right-click a jar under "Dependencies" → "Xóa khỏi ~/.m2" → confirm: the version folder
/// must be gone from the (test) local repository.
#[gpui::test]
fn delete_jar_confirm_with_enter_key(cx: &mut TestAppContext) {
    // the same key bindings main() installs
    cx.update(|cx| {
        crate::actions::bind_editor_keys(cx);
        crate::actions::bind_workspace_keys(cx);
        crate::input::bind_keys(cx);
        crate::picker::bind_keys(cx);
        crate::lsp_editor::bind_keys(cx);
        crate::dialogs::bind_keys(cx);
    });
    let base = std::env::temp_dir().join(format!("ui-jar-enter-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let repo = base.join("repository");
    let ver = repo.join("org/demo/lib/1.0");
    std::fs::create_dir_all(&ver).unwrap();
    let jar = ver.join("lib-1.0.jar");
    std::fs::write(&jar, "jar").unwrap();
    let root = base.join("proj");
    std::fs::create_dir_all(&root).unwrap();
    // SAFETY: only this test reads the variable
    unsafe { std::env::set_var("CHODILAMIDE_M2_REPO", &repo) };

    let settings = Settings { jdtls_enabled: false, ..Settings::default() };
    let (ws, cx) = cx.add_window_view(|_, cx| Workspace::new(root.clone(), settings, None, cx));
    cx.run_until_parked();
    ws.update(cx, |w, cx| {
        w.project = Some(project_model::Project { root: root.display().to_string(), modules: vec![fake_module(&root, vec![jar.display().to_string()])] });
        w.tree.expand(NodeId::Module(root.clone()));
        w.tree.expand(NodeId::Deps(root.clone()));
        cx.notify();
    });
    cx.run_until_parked();

    let row = cx.debug_bounds(sel("tree:lib-1.0.jar".into())).expect("jar row in the tree");
    cx.simulate_mouse_down(row.center(), MouseButton::Right, Modifiers::none());
    cx.simulate_mouse_up(row.center(), MouseButton::Right, Modifiers::none());
    cx.run_until_parked();

    let item = cx.debug_bounds(sel("menu:Xóa khỏi ~/.m2 (tải lại khi Reload)…".into())).expect("delete item in the context menu");
    cx.simulate_click(item.center(), Modifiers::none());
    cx.run_until_parked();

    assert!(cx.debug_bounds("button:cf-ok").is_some(), "confirm dialog");

    // the button says "Xóa (Enter)": the Enter key must work too
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();

    assert!(!ver.exists(), "jar version folder still there: {}", ver.display());
    let msg = ws.read_with(cx, |w, _| w.message.clone());
    assert!(msg.unwrap_or_default().starts_with("Đã xóa"));
    std::fs::remove_dir_all(&base).unwrap();
}

/// A file open in a tab and changed outside the IDE (git pull, nvim) reloads by itself.
#[gpui::test]
fn open_tab_follows_external_change(cx: &mut TestAppContext) {
    let root = std::env::temp_dir().join(format!("ui-watch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("ghi-chu.txt");
    std::fs::write(&file, "cũ\n").unwrap();
    let settings = Settings { jdtls_enabled: false, ..Settings::default() };
    let (ws, cx) = cx.add_window_view(|_, cx| Workspace::new(root.clone(), settings, None, cx));
    cx.run_until_parked();
    ws.update_in(cx, |w, window, cx| w.open_file(file.clone(), window, cx));
    cx.run_until_parked();

    std::fs::write(&file, "mới từ bên ngoài\n").unwrap();
    let mut text = String::new();
    for _ in 0..40 {
        std::thread::sleep(std::time::Duration::from_millis(100)); // let inotify deliver
        cx.executor().advance_clock(std::time::Duration::from_millis(400)); // the 300 ms batch timer
        cx.run_until_parked();
        text = ws.read_with(cx, |w, cx| w.tabs[0].read(cx).editor.buffer.text());
        if text.starts_with("mới") {
            break;
        }
    }
    assert_eq!(text, "mới từ bên ngoài\n");
    let dirty = ws.read_with(cx, |w, cx| w.tabs[0].read(cx).is_dirty());
    assert!(!dirty);
    std::fs::remove_dir_all(&root).unwrap();
}

/// Ctrl+Space (and the alternatives) reach the editor's completion command.
#[gpui::test]
fn completion_keys_reach_the_editor(cx: &mut TestAppContext) {
    cx.update(|cx| {
        crate::actions::bind_editor_keys(cx);
        crate::actions::bind_workspace_keys(cx);
        crate::input::bind_keys(cx);
        crate::lsp_editor::bind_keys(cx);
        crate::dialogs::bind_keys(cx);
    });
    let root = std::env::temp_dir().join(format!("ui-comp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("A.java");
    std::fs::write(&file, "class A { void f() { } }\n").unwrap();
    let settings = Settings { jdtls_enabled: false, ..Settings::default() };
    let (ws, cx) = cx.add_window_view(|_, cx| Workspace::new(root.clone(), settings, None, cx));
    cx.run_until_parked();
    ws.update_in(cx, |w, window, cx| w.open_file(file.clone(), window, cx));
    cx.run_until_parked();
    for keys in ["ctrl-space", "ctrl-shift-space", "alt-/", "ctrl-p"] {
        ws.update(cx, |w, cx| w.tabs[0].update(cx, |v, _| v.message = None));
        cx.simulate_keystrokes(keys);
        cx.run_until_parked();
        let msg = ws.read_with(cx, |w, cx| w.tabs[0].read(cx).message.clone());
        assert_eq!(msg.as_deref(), Some("jdtls chưa sẵn sàng cho file này"), "{keys} did not reach the editor");
    }
    std::fs::remove_dir_all(&root).unwrap();
}

/// Ctrl+B in an .xhtml page opens the included page and the composite component (no jdtls needed).
#[gpui::test]
fn jsf_ctrl_b_on_include_and_composite(cx: &mut TestAppContext) {
    cx.update(|cx| {
        crate::actions::bind_editor_keys(cx);
        crate::actions::bind_workspace_keys(cx);
        crate::input::bind_keys(cx);
        crate::lsp_editor::bind_keys(cx);
        crate::dialogs::bind_keys(cx);
    });
    let root = std::env::temp_dir().join(format!("ui-jsf-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let web = root.join("m/src/main/webapp");
    std::fs::create_dir_all(web.join("WEB-INF/inc")).unwrap();
    std::fs::create_dir_all(web.join("resources/comp")).unwrap();
    std::fs::write(web.join("WEB-INF/inc/header.xhtml"), "<div/>\n").unwrap();
    std::fs::write(web.join("resources/comp/greeting.xhtml"), "<cc:interface/>\n").unwrap();
    let page = web.join("page.xhtml");
    let text = "<html xmlns:comp=\"jakarta.faces.composite/comp\">\n<ui:include src=\"/WEB-INF/inc/header.xhtml\"/>\n<comp:greeting who=\"x\"/>\n</html>\n";
    std::fs::write(&page, text).unwrap();
    let settings = Settings { jdtls_enabled: false, ..Settings::default() };
    let (ws, cx) = cx.add_window_view(|_, cx| Workspace::new(root.clone(), settings, None, cx));
    cx.run_until_parked();
    let module: project_model::Module = serde_json::from_value(serde_json::json!({ "path": root.join("m"), "content_root": root.join("m") })).unwrap();
    ws.update(cx, |w, _| w.project = Some(project_model::Project { root: root.display().to_string(), modules: vec![module] }));
    let open_and_jump = |cx: &mut gpui::VisualTestContext, needle: &str| -> String {
        ws.update_in(cx, |w, window, cx| w.open_file(page.clone(), window, cx));
        cx.run_until_parked();
        let at = text.find(needle).unwrap() + 2;
        ws.update(cx, |w, cx| {
            let i = w.active.unwrap();
            w.tabs[i].update(cx, |v, _| v.editor.set_cursor(at, false))
        });
        cx.simulate_keystrokes("ctrl-b");
        cx.run_until_parked();
        ws.read_with(cx, |w, cx| w.tabs[w.active.unwrap()].read(cx).path().unwrap().display().to_string())
    };
    assert!(open_and_jump(cx, "WEB-INF/inc").ends_with("WEB-INF/inc/header.xhtml"));
    assert!(open_and_jump(cx, "greeting").ends_with("resources/comp/greeting.xhtml"));
    std::fs::remove_dir_all(&root).unwrap();
}

/// A long line: End scrolls the editor sideways to the caret, Home scrolls back.
#[gpui::test]
fn editor_scrolls_horizontally_with_caret(cx: &mut TestAppContext) {
    cx.update(|cx| {
        crate::actions::bind_editor_keys(cx);
        crate::actions::bind_workspace_keys(cx);
        crate::input::bind_keys(cx);
        crate::lsp_editor::bind_keys(cx);
        crate::dialogs::bind_keys(cx);
    });
    let root = std::env::temp_dir().join(format!("ui-hscroll-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("Long.java");
    std::fs::write(&file, format!("class Long {{ String s = \"{}\"; }}\nshort\n", "x".repeat(600))).unwrap();
    let settings = Settings { jdtls_enabled: false, ..Settings::default() };
    let (ws, cx) = cx.add_window_view(|_, cx| Workspace::new(root.clone(), settings, None, cx));
    cx.run_until_parked();
    ws.update_in(cx, |w, window, cx| w.open_file(file.clone(), window, cx));
    cx.run_until_parked();
    let scroll_x = |cx: &mut gpui::VisualTestContext| ws.read_with(cx, |w, cx| f32::from(w.tabs[0].read(cx).scroll_x));
    assert_eq!(scroll_x(cx), 0.);
    cx.simulate_keystrokes("end");
    cx.run_until_parked();
    assert!(scroll_x(cx) > 1000., "End did not scroll: {}", scroll_x(cx));
    cx.simulate_keystrokes("home");
    cx.run_until_parked();
    assert_eq!(scroll_x(cx), 0.);
    std::fs::remove_dir_all(&root).unwrap();
}

/// Find Action (Ctrl+Shift+A): pick "Nhân đôi dòng" and run it in the editor; then give it a new
/// shortcut with Ctrl+Enter and use that shortcut.
#[gpui::test]
fn find_action_runs_and_rebinds(cx: &mut TestAppContext) {
    let root = std::env::temp_dir().join(format!("ui-actions-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    // keymap.toml goes to a scratch config dir, never the user's
    let old_cfg = std::env::var_os("XDG_CONFIG_HOME");
    unsafe { std::env::set_var("XDG_CONFIG_HOME", root.join("cfg")) };
    cx.update(|cx| {
        crate::actions::bind_editor_keys(cx);
        crate::actions::bind_workspace_keys(cx);
        crate::input::bind_keys(cx);
        crate::picker::bind_keys(cx);
        crate::lsp_editor::bind_keys(cx);
        crate::dialogs::bind_keys(cx);
        crate::picker::install_capture(cx);
        crate::keymap::apply(cx);
    });
    let file = root.join("a.txt");
    std::fs::write(&file, "một\n").unwrap();
    let settings = Settings { jdtls_enabled: false, ..Settings::default() };
    let (ws, cx) = cx.add_window_view(|_, cx| Workspace::new(root.clone(), settings, None, cx));
    cx.run_until_parked();
    ws.update_in(cx, |w, window, cx| w.open_file(file.clone(), window, cx));
    cx.run_until_parked();
    let text = |cx: &mut gpui::VisualTestContext| ws.read_with(cx, |w, cx| w.tabs[0].read(cx).editor.buffer.text());

    cx.simulate_keystrokes("ctrl-shift-a");
    cx.run_until_parked();
    assert!(ws.read_with(cx, |w, _| w.picker.is_some()), "Find Action did not open");
    cx.simulate_input("nhân đôi");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(text(cx), "một\nmột\n");

    // new shortcut: Ctrl+Enter, then press Ctrl+Alt+K
    cx.simulate_keystrokes("ctrl-shift-a");
    cx.run_until_parked();
    cx.simulate_input("nhân đôi");
    cx.run_until_parked();
    cx.simulate_keystrokes("ctrl-enter");
    cx.run_until_parked();
    cx.simulate_keystrokes("ctrl-alt-k");
    cx.run_until_parked();
    let saved = std::fs::read_to_string(root.join("cfg/chodilamide/keymap.toml")).unwrap_or_default();
    assert!(saved.contains("editor::DuplicateLine") && saved.contains("ctrl-alt-k"), "{saved}");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.simulate_keystrokes("ctrl-alt-k");
    cx.run_until_parked();
    assert_eq!(text(cx), "một\nmột\nmột\n");
    // the old Ctrl+D no longer duplicates
    cx.simulate_keystrokes("ctrl-d");
    cx.run_until_parked();
    assert_eq!(text(cx), "một\nmột\nmột\n");

    unsafe {
        match old_cfg {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
    }
    std::fs::remove_dir_all(&root).unwrap();
}

/// Wheel over a popup (Find Action here) scrolls the popup only, never the editor under it.
#[gpui::test]
fn wheel_on_popup_does_not_scroll_editor(cx: &mut TestAppContext) {
    cx.update(|cx| {
        crate::actions::bind_editor_keys(cx);
        crate::actions::bind_workspace_keys(cx);
        crate::input::bind_keys(cx);
        crate::picker::bind_keys(cx);
        crate::lsp_editor::bind_keys(cx);
        crate::dialogs::bind_keys(cx);
    });
    let root = std::env::temp_dir().join(format!("ui-wheel-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("dai.txt");
    std::fs::write(&file, (0..500).map(|i| format!("dòng {i}\n")).collect::<String>()).unwrap();
    let settings = Settings { jdtls_enabled: false, ..Settings::default() };
    let (ws, cx) = cx.add_window_view(|_, cx| Workspace::new(root.clone(), settings, None, cx));
    cx.run_until_parked();
    ws.update_in(cx, |w, window, cx| w.open_file(file.clone(), window, cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("ctrl-shift-a");
    cx.run_until_parked();
    let b = cx.debug_bounds("picker").expect("picker not painted");
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: b.center(),
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-400.))),
        ..Default::default()
    });
    cx.run_until_parked();
    let y = ws.read_with(cx, |w, cx| f32::from(w.tabs[0].read(cx).scroll_y));
    assert_eq!(y, 0., "the editor under the popup scrolled");
    std::fs::remove_dir_all(&root).unwrap();
}

/// Every tree side / panel side / theme renders, and the tree can be hidden.
#[gpui::test]
fn layouts_and_themes_render(cx: &mut TestAppContext) {
    cx.update(|cx| {
        crate::actions::bind_editor_keys(cx);
        crate::actions::bind_workspace_keys(cx);
        crate::input::bind_keys(cx);
        crate::lsp_editor::bind_keys(cx);
    });
    let root = std::env::temp_dir().join(format!("ui-layout-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("A.java");
    std::fs::write(&file, "class A {}\n").unwrap();
    let settings = Settings { jdtls_enabled: false, ..Settings::default() };
    let (ws, cx) = cx.add_window_view(|_, cx| Workspace::new(root.clone(), settings, None, cx));
    cx.run_until_parked();
    ws.update_in(cx, |w, window, cx| w.open_file(file.clone(), window, cx));
    for (tree_side, visible, panel_side) in [("left", true, "bottom"), ("right", true, "right"), ("left", false, "left"), ("right", false, "bottom")] {
        ws.update(cx, |w, cx| {
            let mut s = w.settings.clone();
            s.tree_side = tree_side.into();
            s.tree_visible = visible;
            s.panel_side = panel_side.into();
            w.panel_open = true;
            w.apply_settings(s, cx);
        });
        cx.run_until_parked();
        let e = cx.debug_bounds("editor-area").expect("editor not painted");
        if visible {
            // the tree is on the chosen side of the editor
            let t = cx.debug_bounds("project-tree").expect("tree not painted");
            assert_eq!(t.left() < e.left(), tree_side == "left", "{tree_side} {panel_side}");
        } else if panel_side == "bottom" {
            // hidden tree: only the tool window stripe (40px) or the island gap is left of the editor
            assert!(f32::from(e.left()) <= 40. + crate::theme::ISLAND_GAP, "{tree_side} {panel_side}: {:?}", e.left());
        }
    }
    for t in crate::theme::THEMES {
        crate::theme::set_theme(t);
        crate::theme::set_editor_theme(t);
        ws.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
    }
    crate::theme::set_theme("Dark");
    crate::theme::set_editor_theme(crate::theme::EDITOR_FOLLOW);
    std::fs::remove_dir_all(&root).unwrap();
}

/// Spring Boot: typing a key in application.properties completes from the metadata json
/// (here the module's own target/classes one), and Ctrl+B on a key opens the @Value using it.
#[gpui::test]
fn spring_properties_completion_and_ctrl_b(cx: &mut TestAppContext) {
    cx.update(|cx| {
        crate::actions::bind_editor_keys(cx);
        crate::actions::bind_workspace_keys(cx);
        crate::input::bind_keys(cx);
        crate::lsp_editor::bind_keys(cx);
        crate::dialogs::bind_keys(cx);
    });
    let root = std::env::temp_dir().join(format!("ui-spring-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let meta = root.join("target/classes/META-INF");
    std::fs::create_dir_all(&meta).unwrap();
    std::fs::write(meta.join("spring-configuration-metadata.json"), r#"{"properties":[{"name":"server.port","type":"java.lang.Integer","description":"Server HTTP port."},{"name":"server.servlet.context-path","type":"java.lang.String"}]}"#).unwrap();
    let res = root.join("src/main/resources");
    std::fs::create_dir_all(&res).unwrap();
    let props = res.join("application.properties");
    std::fs::write(&props, "app.mail-host=smtp\nserver.p").unwrap();
    let java = root.join("src/main/java/demo");
    std::fs::create_dir_all(&java).unwrap();
    std::fs::write(java.join("Mail.java"), "class Mail {\n  @Value(\"${app.mailHost}\") String host;\n}\n").unwrap();

    let settings = Settings { jdtls_enabled: false, ..Settings::default() };
    let (ws, cx) = cx.add_window_view(|_, cx| Workspace::new(root.clone(), settings, None, cx));
    cx.run_until_parked();
    ws.update_in(cx, |w, window, cx| {
        let mut m = fake_module(&root, vec![]);
        m.source_roots = vec![project_model::SourceRoot { path: root.join("src/main/java").display().to_string(), kind: "main".into(), lang: "java".into() }];
        w.project = Some(project_model::Project { root: root.display().to_string(), modules: vec![m] });
        w.open_file(props.clone(), window, cx);
    });
    cx.run_until_parked();
    ws.update(cx, |w, cx| w.tabs[0].update(cx, |v, _| {
        let end = v.editor.buffer.len();
        v.editor.set_cursor(end, false);
    }));
    cx.simulate_keystrokes("o");
    cx.run_until_parked();
    let labels = ws.read_with(cx, |w, cx| {
        let v = w.tabs[0].read(cx);
        v.completion.as_ref().map(|c| c.visible.iter().map(|i| c.items[*i]["label"].as_str().unwrap_or("").to_string()).collect::<Vec<_>>()).unwrap_or_default()
    });
    assert_eq!(labels, ["server.port"], "completion after 'server.po'");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let text = ws.read_with(cx, |w, cx| w.tabs[0].read(cx).editor.buffer.text());
    assert!(text.ends_with("server.port="), "{text}");

    // Ctrl+B on the first line's key (relaxed name app.mail-host ↔ app.mailHost) → Mail.java
    ws.update(cx, |w, cx| w.tabs[0].update(cx, |v, _| v.editor.set_cursor(2, false)));
    cx.simulate_keystrokes("ctrl-b");
    cx.run_until_parked();
    let active = ws.read_with(cx, |w, cx| w.active.and_then(|i| w.tabs.get(i)).and_then(|t| t.read(cx).path().cloned()));
    assert_eq!(active, Some(java.join("Mail.java")));
    std::fs::remove_dir_all(&root).unwrap();
}

/// Renaming a bean property in Java updates the EL in the pages - an open page in its buffer,
/// a closed one on disk - and the open page gets its JSF inspections (ajax id).
#[gpui::test]
fn jsf_rename_updates_pages_and_inspections(cx: &mut TestAppContext) {
    cx.update(|cx| {
        crate::actions::bind_editor_keys(cx);
        crate::actions::bind_workspace_keys(cx);
        crate::input::bind_keys(cx);
        crate::lsp_editor::bind_keys(cx);
        crate::dialogs::bind_keys(cx);
    });
    let root = std::env::temp_dir().join(format!("ui-jsfren-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let src = root.join("m/src/main/java/demo");
    let web = root.join("m/src/main/webapp");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&web).unwrap();
    let bean = src.join("UserBean.java");
    std::fs::write(&bean, "package demo;\n@Named\n@ViewScoped\npublic class UserBean {\n  private String fullName;\n  public String getFullName() { return fullName; }\n}\n").unwrap();
    let open_page = web.join("a.xhtml");
    let closed_page = web.join("b.xhtml");
    std::fs::write(&open_page, "<h:form id=\"f\"><h:outputText value=\"#{userBean.name}\"/><p:commandButton update=\"nope\"/></h:form>\n").unwrap();
    std::fs::write(&closed_page, "<p>#{userBean['name']} #{userBean.names}</p>\n").unwrap();

    let settings = Settings { jdtls_enabled: false, ..Settings::default() };
    let (ws, cx) = cx.add_window_view(|_, cx| Workspace::new(root.clone(), settings, None, cx));
    cx.run_until_parked();
    let m = root.join("m");
    let module: project_model::Module = serde_json::from_value(serde_json::json!({
        "path": m, "content_root": m,
        "source_roots": [{ "path": m.join("src/main/java"), "kind": "main", "lang": "java" }]
    }))
    .unwrap();
    ws.update(cx, |w, _| w.project = Some(project_model::Project { root: root.display().to_string(), modules: vec![module] }));
    ws.update_in(cx, |w, window, cx| w.open_file(open_page.clone(), window, cx));
    cx.run_until_parked();
    // inspections of the open page: the unknown ajax id is underlined
    let diags = ws.read_with(cx, |w, cx| w.tabs[0].read(cx).diagnostics.iter().map(|d| d.message.clone()).collect::<Vec<_>>());
    assert!(diags.iter().any(|m| m.contains("'nope'")), "{diags:?}");

    // the Java side was renamed name → fullName (jdtls part skipped: no jdtls here)
    ws.update_in(cx, |w, window, cx| w.open_file(bean.clone(), window, cx));
    cx.run_until_parked();
    ws.update(cx, |w, cx| {
        let t = w.tabs[1].clone();
        w.jsf_after_rename(t, "name".into(), "fullName".into(), cx)
    });
    cx.run_until_parked();
    let open_text = ws.read_with(cx, |w, cx| w.tabs[0].read(cx).editor.buffer.text());
    assert!(open_text.contains("#{userBean.fullName}"), "{open_text}");
    let closed = std::fs::read_to_string(&closed_page).unwrap();
    assert_eq!(closed, "<p>#{userBean['fullName']} #{userBean.names}</p>\n");
    // "◇ N xhtml" on the getter (pages on disk: b.xhtml uses userBean['fullName'])
    ws.update(cx, |w, cx| {
        let t = w.tabs[1].clone();
        w.jsf_lens(t, cx)
    });
    cx.run_until_parked();
    let lens = ws.read_with(cx, |w, cx| w.tabs[1].read(cx).lens.clone());
    assert_eq!(lens, [(5, "◇ 1 xhtml".to_string())]);
    std::fs::remove_dir_all(&root).unwrap();
}

/// Daily-editing keys reach their commands: Ctrl+/ comments, Ctrl+R + Alt+A replaces all,
/// Ctrl+E lists the previous file, Shift Shift opens Search Everywhere.
#[gpui::test]
fn editing_and_navigation_keys(cx: &mut TestAppContext) {
    cx.update(|cx| {
        crate::actions::bind_editor_keys(cx);
        crate::actions::bind_workspace_keys(cx);
        crate::input::bind_keys(cx);
        crate::lsp_editor::bind_keys(cx);
        crate::dialogs::bind_keys(cx);
    });
    let root = std::env::temp_dir().join(format!("ui-keys-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let a = root.join("A.java");
    let b = root.join("app.yml");
    std::fs::write(&a, "class A {\n    int foo = foo + 1;\n}\n").unwrap();
    std::fs::write(&b, "server:\n  port: 1\n").unwrap();
    let settings = Settings { jdtls_enabled: false, ..Settings::default() };
    let (ws, cx) = cx.add_window_view(|_, cx| Workspace::new(root.clone(), settings, None, cx));
    cx.run_until_parked();
    ws.update_in(cx, |w, window, cx| w.open_file(a.clone(), window, cx));
    cx.run_until_parked();
    let text = |cx: &mut gpui::VisualTestContext| ws.read_with(cx, |w, cx| w.tabs[w.active.unwrap()].read(cx).editor.buffer.text());

    ws.update(cx, |w, cx| w.tabs[0].update(cx, |v, _| v.editor.set_cursor(14, false)));
    cx.simulate_keystrokes("ctrl-/");
    cx.run_until_parked();
    assert_eq!(text(cx), "class A {\n    // int foo = foo + 1;\n}\n");
    cx.simulate_keystrokes("ctrl-z");

    // Ctrl+R: find "foo", replace with "bar", Alt+A = all
    cx.simulate_keystrokes("ctrl-r");
    cx.run_until_parked();
    cx.simulate_input("foo");
    cx.run_until_parked();
    ws.update_in(cx, |w, window, cx| {
        let t = w.tabs[0].clone();
        t.update(cx, |v, cx| {
            let rep = v.replace_input.clone().expect("replace field open");
            rep.update(cx, |i, cx| i.set_text("bar", cx));
            v.replace_all(cx);
            let _ = window;
        })
    });
    cx.run_until_parked();
    assert_eq!(text(cx), "class A {\n    int bar = bar + 1;\n}\n");

    // Ctrl+E: the other file, most recent first
    ws.update_in(cx, |w, window, cx| w.open_file(b.clone(), window, cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("ctrl-e");
    cx.run_until_parked();
    assert!(ws.read_with(cx, |w, _| w.picker.is_some()), "Ctrl+E opens Recent Files");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    ws.update(cx, |w, _| w.picker = None);

    // Shift, Shift
    for _ in 0..2 {
        cx.simulate_modifiers_change(Modifiers::shift());
        cx.simulate_modifiers_change(Modifiers::none());
    }
    cx.run_until_parked();
    assert!(ws.read_with(cx, |w, _| w.picker.is_some()), "Shift Shift opens Search Everywhere");
    std::fs::remove_dir_all(&root).unwrap();
}

/// Ctrl+- collapses the method body: its lines are no longer laid out; Ctrl+= brings them back.
#[gpui::test]
fn folding_keys_hide_lines(cx: &mut TestAppContext) {
    cx.update(|cx| {
        crate::actions::bind_editor_keys(cx);
        crate::actions::bind_workspace_keys(cx);
        crate::input::bind_keys(cx);
        crate::lsp_editor::bind_keys(cx);
        crate::dialogs::bind_keys(cx);
    });
    let root = std::env::temp_dir().join(format!("ui-fold-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let a = root.join("A.java");
    std::fs::write(&a, "class A {\n    void f() {\n        x();\n        y();\n    }\n    int z;\n}\n").unwrap();
    let settings = Settings { jdtls_enabled: false, ..Settings::default() };
    let (ws, cx) = cx.add_window_view(|_, cx| Workspace::new(root.clone(), settings, None, cx));
    cx.run_until_parked();
    ws.update_in(cx, |w, window, cx| w.open_file(a.clone(), window, cx));
    cx.run_until_parked();
    let shown = |cx: &mut gpui::VisualTestContext| ws.read_with(cx, |w, cx| w.tabs[0].read(cx).laid_out_lines());
    ws.update(cx, |w, cx| w.tabs[0].update(cx, |v, _| v.editor.set_cursor(40, false))); // in x();
    cx.simulate_keystrokes("ctrl--");
    cx.run_until_parked();
    assert_eq!(shown(cx), [0, 1, 4, 5, 6, 7]);
    cx.simulate_keystrokes("ctrl-=");
    cx.run_until_parked();
    assert_eq!(shown(cx), [0, 1, 2, 3, 4, 5, 6, 7]);
    std::fs::remove_dir_all(&root).unwrap();
}

/// Git: a changed line gets a gutter mark, Ctrl+Alt+Z rolls it back, the tree/status know the
/// file, Ctrl+K opens the commit dialog and committing clears the status.
#[gpui::test]
fn git_gutter_rollback_and_commit(cx: &mut TestAppContext) {
    cx.update(|cx| {
        crate::actions::bind_editor_keys(cx);
        crate::actions::bind_workspace_keys(cx);
        crate::input::bind_keys(cx);
        crate::lsp_editor::bind_keys(cx);
        crate::dialogs::bind_keys(cx);
    });
    let root = std::env::temp_dir().join(format!("ui-git-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let git = |args: &[&str]| std::process::Command::new("git").arg("-C").arg(&root).args(args).output().map(|o| o.status.success()).unwrap_or(false);
    if !git(&["init", "-q", "-b", "main"]) {
        return; // no git on this machine
    }
    git(&["config", "user.email", "t@t"]);
    git(&["config", "user.name", "T"]);
    let a = root.join("A.java");
    std::fs::write(&a, "class A {\n    int x = 1;\n}\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-qm", "init"]);

    let settings = Settings { jdtls_enabled: false, ..Settings::default() };
    let (ws, cx) = cx.add_window_view(|_, cx| Workspace::new(root.clone(), settings, None, cx));
    cx.run_until_parked();
    // git runs on background threads: wait for the first status
    for _ in 0..50 {
        if ws.read_with(cx, |w, _| w.git.branch.is_some()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        cx.run_until_parked();
    }
    assert_eq!(ws.read_with(cx, |w, _| w.git.branch.clone()).as_deref(), Some("main"));
    ws.update_in(cx, |w, window, cx| w.open_file(a.clone(), window, cx));
    for _ in 0..50 {
        if ws.read_with(cx, |w, cx| w.tabs[0].read(cx).git_base.is_some()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        cx.run_until_parked();
    }
    // edit line 2 → a "modified" mark on it after the next paint
    ws.update(cx, |w, cx| w.tabs[0].update(cx, |v, cx| {
        v.editor.set_cursor(22, false); // after "1"
        v.editor.insert("0");
        cx.notify()
    }));
    cx.run_until_parked();
    let marks = ws.read_with(cx, |w, cx| w.tabs[0].read(cx).git_marks.clone());
    assert_eq!(marks, [(1, vcs::LineMark::Modified)]);
    cx.simulate_keystrokes("ctrl-alt-z");
    cx.run_until_parked();
    assert_eq!(ws.read_with(cx, |w, cx| w.tabs[0].read(cx).editor.buffer.text()), "class A {\n    int x = 1;\n}\n");

    // change on disk, commit through the dialog
    ws.update(cx, |w, cx| w.tabs[0].update(cx, |v, cx| {
        v.editor.set_cursor(0, false);
        v.editor.insert("// c\n");
        cx.notify()
    }));
    cx.simulate_keystrokes("ctrl-k");
    cx.run_until_parked();
    assert!(ws.read_with(cx, |w, _| w.log_view.as_ref().is_some_and(|v| v.file.is_none() && v.working) && w.git_ui.is_some()), "Ctrl+K opens the Git window on the working tree");
    // nothing staged: Commit takes every tracked change (git status arrives in the background)
    for _ in 0..50 {
        if ws.read_with(cx, |w, _| !w.git.tree.changes.is_empty()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        cx.run_until_parked();
    }
    ws.update(cx, |w, cx| w.commit_for_test("add comment", cx));
    let last = || {
        let out = std::process::Command::new("git").arg("-C").arg(&root).args(["log", "--format=%s", "-n1"]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    for _ in 0..100 {
        cx.run_until_parked();
        if last() == "add comment" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let out = std::process::Command::new("git").arg("-C").arg(&root).args(["log", "--format=%s", "-n1"]).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "add comment");
    std::fs::remove_dir_all(&root).unwrap();
}

/// HTTP Client: Ctrl+Enter on a request of an .http file runs it through curl ({{host}} from
/// http-client.env.json) and prints status + pretty JSON in a Run tab.
#[gpui::test]
fn http_client_runs_request(cx: &mut TestAppContext) {
    cx.update(|cx| {
        crate::actions::bind_editor_keys(cx);
        crate::actions::bind_workspace_keys(cx);
        crate::input::bind_keys(cx);
        crate::lsp_editor::bind_keys(cx);
        crate::dialogs::bind_keys(cx);
    });
    let root = std::env::temp_dir().join(format!("ui-http-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    // a one-shot JSON server
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        if let Ok((mut s, _)) = listener.accept() {
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf);
            let body = "{\"ok\":true,\"items\":[1,2]}";
            let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        }
    });
    std::fs::write(root.join("http-client.env.json"), format!("{{\"dev\": {{\"host\": \"http://127.0.0.1:{port}\"}}}}")).unwrap();
    let f = root.join("api.http");
    std::fs::write(&f, "### items\nGET {{host}}/api/items\nAccept: application/json\n").unwrap();
    let settings = Settings { jdtls_enabled: false, ..Settings::default() };
    let (ws, cx) = cx.add_window_view(|_, cx| Workspace::new(root.clone(), settings, None, cx));
    cx.run_until_parked();
    ws.update_in(cx, |w, window, cx| w.open_file(f.clone(), window, cx));
    cx.run_until_parked();
    ws.update(cx, |w, cx| w.tabs[0].update(cx, |v, _| v.editor.set_cursor(12, false)));
    cx.simulate_keystrokes("ctrl-enter");
    let console = |cx: &mut gpui::VisualTestContext| ws.read_with(cx, |w, _| w.sessions.last().map(|s| s.console.iter().map(|(l, _)| l.clone()).collect::<Vec<_>>().join("\n")).unwrap_or_default());
    for _ in 0..200 {
        cx.run_until_parked();
        if console(cx).contains("ms)") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let out = console(cx);
    assert!(out.contains(&format!("GET http://127.0.0.1:{port}/api/items")), "{out}");
    assert!(out.contains("HTTP/1.1 200 OK"), "{out}");
    assert!(out.contains("\"items\": ["), "pretty JSON: {out}");
    std::fs::remove_dir_all(&root).unwrap();
}

/// Alt+J twice then typing edits both occurrences; Esc goes back to one caret.
#[gpui::test]
fn multiple_carets_through_keys(cx: &mut TestAppContext) {
    cx.update(|cx| {
        crate::actions::bind_editor_keys(cx);
        crate::actions::bind_workspace_keys(cx);
        crate::input::bind_keys(cx);
        crate::lsp_editor::bind_keys(cx);
        crate::dialogs::bind_keys(cx);
    });
    let root = std::env::temp_dir().join(format!("ui-multi-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let a = root.join("A.java");
    std::fs::write(&a, "int foo = foo * 2;\n").unwrap();
    let settings = Settings { jdtls_enabled: false, ..Settings::default() };
    let (ws, cx) = cx.add_window_view(|_, cx| Workspace::new(root.clone(), settings, None, cx));
    cx.run_until_parked();
    ws.update_in(cx, |w, window, cx| w.open_file(a.clone(), window, cx));
    cx.run_until_parked();
    ws.update(cx, |w, cx| w.tabs[0].update(cx, |v, _| v.editor.set_cursor(5, false)));
    cx.simulate_keystrokes("alt-j alt-j");
    cx.run_until_parked();
    cx.simulate_input("bar");
    cx.run_until_parked();
    assert_eq!(ws.read_with(cx, |w, cx| w.tabs[0].read(cx).editor.buffer.text()), "int bar = bar * 2;\n");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(ws.read_with(cx, |w, cx| w.tabs[0].read(cx).editor.extra.is_empty()));
    std::fs::remove_dir_all(&root).unwrap();
}

/// .xhtml completion without jdtls: beans after "#{", tags after "<p:" (PrimeFaces taglib read
/// from its jar) and "<h:", attributes inside a tag (inserted as name="" with the caret inside).
#[gpui::test]
fn jsf_tag_and_bean_completion(cx: &mut TestAppContext) {
    cx.update(|cx| {
        crate::actions::bind_editor_keys(cx);
        crate::actions::bind_workspace_keys(cx);
        crate::input::bind_keys(cx);
        crate::lsp_editor::bind_keys(cx);
        crate::dialogs::bind_keys(cx);
    });
    let root = std::env::temp_dir().join(format!("ui-jsfc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let src = root.join("m/src/main/java/demo");
    let web = root.join("m/src/main/webapp");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&web).unwrap();
    std::fs::write(src.join("HelloBean.java"), "package demo;\n@Named\n@ViewScoped\npublic class HelloBean {\n  public String getName() { return null; }\n}\n").unwrap();
    let page = web.join("a.xhtml");
    let head = "<ui:composition xmlns:h=\"http://xmlns.jcp.org/jsf/html\" xmlns:p=\"http://primefaces.org/ui\">\n";
    std::fs::write(&page, format!("{head}\n</ui:composition>\n")).unwrap();
    let pf: Vec<String> = std::fs::read_dir(PathBuf::from(std::env::var("HOME").unwrap()).join(".m2/repository/org/primefaces/primefaces"))
        .into_iter()
        .flatten()
        .flatten()
        .flat_map(|d| std::fs::read_dir(d.path()).into_iter().flatten().flatten().map(|f| f.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "jar") && !p.to_string_lossy().contains("-sources"))
        .map(|p| p.display().to_string())
        .take(1)
        .collect();
    let settings = Settings { jdtls_enabled: false, ..Settings::default() };
    let (ws, cx) = cx.add_window_view(|_, cx| Workspace::new(root.clone(), settings, None, cx));
    cx.run_until_parked();
    let m = root.join("m");
    let module: project_model::Module = serde_json::from_value(serde_json::json!({
        "path": m, "content_root": m, "_classpath_jars": pf,
        "source_roots": [{ "path": m.join("src/main/java"), "kind": "main", "lang": "java" }]
    })).unwrap();
    ws.update(cx, |w, _| w.project = Some(project_model::Project { root: root.display().to_string(), modules: vec![module] }));
    ws.update_in(cx, |w, window, cx| w.open_file(page.clone(), window, cx));
    cx.run_until_parked();
    let labels = |cx: &mut gpui::VisualTestContext| {
        for _ in 0..100 {
            cx.run_until_parked();
            let l = ws.read_with(cx, |w, cx| w.tabs[0].read(cx).completion.as_ref().map(|c| c.visible.iter().map(|i| c.items[*i]["label"].as_str().unwrap_or("").to_string()).collect::<Vec<_>>()));
            if l.as_ref().is_some_and(|l| !l.is_empty()) {
                return l.unwrap();
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        Vec::new()
    };
    ws.update(cx, |w, cx| w.tabs[0].update(cx, |v, _| v.editor.set_cursor(head.len(), false)));
    cx.simulate_input("<h:inputT");
    assert!(labels(cx).contains(&"inputText".to_string()), "h: tags");
    cx.simulate_keystrokes("enter");
    cx.simulate_input(" valu");
    assert!(labels(cx).contains(&"value".to_string()), "attributes");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("#{hel");
    assert_eq!(labels(cx).first().map(String::as_str), Some("helloBean"));
    let text = ws.read_with(cx, |w, cx| w.tabs[0].read(cx).editor.buffer.text());
    assert!(text.contains("<h:inputText value=\"#{hel"), "{text}");
    if !pf.is_empty() {
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        let still_open = ws.read_with(cx, |w, cx| w.tabs[0].read(cx).completion.is_some());
        assert!(!still_open, "Esc closes the completion popup");
        ws.update(cx, |w, cx| w.tabs[0].update(cx, |v, _| v.editor.set_cursor(head.len(), false)));
        cx.simulate_input("\n<p:dataT");
        let l = labels(cx);
        assert!(l.contains(&"dataTable".to_string()), "PrimeFaces tags from the jar {pf:?}: {l:?}");
    }
    std::fs::remove_dir_all(&root).unwrap();
}

#[gpui::test]
#[ignore]
fn jsf_member_completion_real_jdtls(cx: &mut TestAppContext) {
    cx.update(|cx| {
        crate::actions::bind_editor_keys(cx);
        crate::actions::bind_workspace_keys(cx);
        crate::input::bind_keys(cx);
        crate::lsp_editor::bind_keys(cx);
        crate::dialogs::bind_keys(cx);
    });
    let root = PathBuf::from(std::env::var("HOME").unwrap()).join("Git-projects/java-debug-model/test-fixtures/sample-project");
    let page = root.join("module-a/src/main/webapp/hello.xhtml");
    let settings = Settings { jdtls_enabled: true, ..Settings::default() };
    let (ws, cx) = cx.add_window_view(|_, cx| Workspace::new(root.clone(), settings, None, cx));
    ws.update(cx, |w, cx| { w.visible = true; w.start_lsp(cx) });
    for i in 0..600 {
        cx.run_until_parked();
        if ws.read_with(cx, |w, _| matches!(w.lsp_state, crate::lsp_workspace::LspState::Ready)) { eprintln!("ready after {i}"); break; }
        if i % 50 == 0 { ws.update(cx, |w, cx| { if w.project.is_some() && matches!(w.lsp_state, crate::lsp_workspace::LspState::Off) { w.start_lsp(cx) } }); }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    eprintln!("state {}", ws.read_with(cx, |w, _| w.lsp_status()));
    ws.update_in(cx, |w, window, cx| w.open_file(page.clone(), window, cx));
    cx.run_until_parked();
    ws.update(cx, |w, cx| w.tabs[0].update(cx, |v, _| {
        let t = v.editor.buffer.text();
        let at = t.find("value=\"#{helloBean.name}").unwrap() + 7;
        v.editor.set_cursor(at, false);
    }));
    cx.simulate_input("#{helloBean.");
    let mut labels = None;
    for _ in 0..100 {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(100));
        labels = ws.read_with(cx, |w, cx| w.tabs[0].read(cx).completion.as_ref().map(|c| c.visible.iter().map(|i| c.items[*i]["label"].as_str().unwrap_or("").to_string()).collect::<Vec<_>>()));
        if labels.as_ref().is_some_and(|l| !l.is_empty()) { break; }
    }
    eprintln!("MEMBERS {labels:?} msg={:?}", ws.read_with(cx, |w, cx| w.tabs[0].read(cx).message.clone()));
    ws.update(cx, |w, _| { let _ = w.shutdown_lsp(); });
}

/// Vim keymap end to end: Normal-mode commands through real keystrokes (no IME involved),
/// Insert typing, Esc, undo, the ':' command line, a vimrc mapping, and an IDE key (Ctrl+E)
/// still reaching the IDE.
#[gpui::test]
fn vim_mode_keys(cx: &mut TestAppContext) {
    cx.update(|cx| {
        crate::actions::bind_editor_keys(cx);
        crate::actions::bind_workspace_keys(cx);
        crate::input::bind_keys(cx);
        crate::lsp_editor::bind_keys(cx);
        crate::dialogs::bind_keys(cx);
        crate::vim_mode::install(cx);
    });
    let root = std::env::temp_dir().join(format!("ui-vim-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("cfg/chodilamide")).unwrap();
    std::fs::write(root.join("cfg/chodilamide/vimrc"), "inoremap jk <Esc>\n").unwrap();
    // SAFETY: tests run one at a time (--test-threads=1)
    unsafe { std::env::set_var("XDG_CONFIG_HOME", root.join("cfg")) };
    let a = root.join("A.java");
    std::fs::write(&a, "int foo = 1;\nint bar = 2;\nint baz = 3;\n").unwrap();
    let settings = Settings { jdtls_enabled: false, keymap_mode: "Vim".into(), ..Settings::default() };
    let (ws, cx) = cx.add_window_view(|_, cx| Workspace::new(root.clone(), settings, None, cx));
    cx.run_until_parked();
    ws.update_in(cx, |w, window, cx| w.open_file(a.clone(), window, cx));
    cx.run_until_parked();
    let text = |cx: &mut gpui::VisualTestContext| ws.read_with(cx, |w, cx| w.tabs[0].read(cx).editor.buffer.text());
    let status = |cx: &mut gpui::VisualTestContext| ws.read_with(cx, |w, cx| w.tabs[0].read(cx).vim_status());
    assert_eq!(status(cx).as_deref(), Some("-- NORMAL --"));

    cx.simulate_keystrokes("j d d");
    cx.run_until_parked();
    assert_eq!(text(cx), "int foo = 1;\nint baz = 3;\n");
    cx.simulate_keystrokes("u");
    cx.run_until_parked();
    assert_eq!(text(cx), "int foo = 1;\nint bar = 2;\nint baz = 3;\n");
    cx.simulate_keystrokes("ctrl-r");
    cx.run_until_parked();
    assert_eq!(text(cx), "int foo = 1;\nint baz = 3;\n", "Ctrl+R = Vim redo");

    // ciw + typing + the vimrc's "jk" = Esc
    cx.simulate_keystrokes("g g w c i w");
    cx.run_until_parked();
    assert_eq!(status(cx).as_deref(), Some("-- INSERT --"));
    cx.simulate_input("qux");
    cx.simulate_keystrokes("j k");
    cx.run_until_parked();
    assert_eq!(text(cx), "int qux = 1;\nint baz = 3;\n");
    assert_eq!(status(cx).as_deref(), Some("-- NORMAL --"));

    // :%s via the command line
    cx.simulate_keystrokes(":");
    cx.run_until_parked();
    cx.simulate_input("%s/int/long/g");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(text(cx), "long qux = 1;\nlong baz = 3;\n");

    // Visual Block + I: a column typed on both lines; a macro replays it
    cx.simulate_keystrokes("g g ctrl-v j shift-i");
    cx.run_until_parked();
    cx.simulate_input("x");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(text(cx), "xlong qux = 1;\nxlong baz = 3;\n");
    cx.simulate_keystrokes("q a shift-a");
    cx.simulate_input(";");
    cx.simulate_keystrokes("escape j q");
    cx.run_until_parked();
    cx.simulate_keystrokes("k @ a");
    cx.run_until_parked();
    assert_eq!(text(cx), "xlong qux = 1;;;\nxlong baz = 3;\n", "@a replays A; on the first line again");

    // IDE keys not given to Vim still work: Ctrl+E (Recent Files) opens the picker
    cx.simulate_keystrokes("ctrl-e");
    cx.run_until_parked();
    let msg_or_picker = ws.read_with(cx, |w, _| w.picker.is_some() || w.message.is_some());
    assert!(msg_or_picker, "Ctrl+E reached the IDE");
    unsafe { std::env::remove_var("XDG_CONFIG_HOME") };
    std::fs::remove_dir_all(&root).unwrap();
}

/// A message becomes a balloon (bottom right), errors marked, the same text not repeated.
#[gpui::test]
fn messages_become_balloons(cx: &mut TestAppContext) {
    let root = std::env::temp_dir().join(format!("ui-toast-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let settings = Settings { jdtls_enabled: false, ..Settings::default() };
    let (ws, cx) = cx.add_window_view(|_, cx| Workspace::new(root.clone(), settings, None, cx));
    cx.run_until_parked();
    ws.update(cx, |w, cx| {
        w.message = Some("Đã commit 2 file".into());
        cx.notify()
    });
    cx.run_until_parked();
    ws.update(cx, |w, cx| {
        w.message = Some("Push lỗi: rejected".into());
        cx.notify()
    });
    cx.run_until_parked();
    let t = ws.read_with(cx, |w, _| w.toasts.iter().map(|(t, _, e)| (t.clone(), *e)).collect::<Vec<_>>());
    assert_eq!(t, [("Đã commit 2 file".to_string(), false), ("Push lỗi: rejected".to_string(), true)]);
    std::fs::remove_dir_all(&root).unwrap();
}

/// Settings → a dropdown (every installed font): the mouse wheel scrolls its list.
#[gpui::test]
fn settings_select_scrolls(cx: &mut TestAppContext) {
    cx.update(|cx| {
        crate::actions::bind_editor_keys(cx);
        crate::actions::bind_workspace_keys(cx);
        crate::input::bind_keys(cx);
        crate::dialogs::bind_keys(cx);
    });
    let (shell, cx) = cx.add_window_view(|window, cx| crate::shell::Shell::new(Settings::default(), None, Vec::new(), None, window, cx));
    cx.run_until_parked();
    shell.update_in(cx, |s, window, cx| {
        s.open_settings(window, cx);
        s.debug_open_font_select(window, cx);
    });
    cx.run_until_parked();
    let list = cx.debug_bounds("select-list:sel-editor-font").expect("font list shown");
    let before = shell.read_with(cx, |s, _| s.font_select_offset()).unwrap();
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: list.center(),
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-120.))),
        ..Default::default()
    });
    cx.run_until_parked();
    let after = shell.read_with(cx, |s, _| s.font_select_offset()).unwrap();
    assert!(after.y < before.y, "list did not scroll: {before:?} → {after:?}");
}

/// Debug/Run of a test after editing it: the unsaved edit is saved and a stale .class is
/// recompiled, so the test JVM runs the new code (bug: it ran the old class file).
#[gpui::test]
fn run_uses_new_test_code(cx: &mut TestAppContext) {
    let root = std::env::temp_dir().join(format!("ui-stale-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let src = root.join("m/src/test/java/a");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(root.join("m/target/test-classes/a")).unwrap();
    let f = src.join("T.java");
    std::fs::write(&f, "package a; public class T { public static String v() { return \"old\"; } }\n").unwrap();
    // a stale class file from an earlier build
    let class = root.join("m/target/test-classes/a/T.class");
    std::fs::write(&class, "stale").unwrap();
    let settings = Settings { jdtls_enabled: false, ..Settings::default() };
    let (ws, cx) = cx.add_window_view(|_, cx| Workspace::new(root.clone(), settings, None, cx));
    cx.run_until_parked();
    let m = root.join("m");
    let module: project_model::Module = serde_json::from_value(serde_json::json!({
        "path": m, "content_root": m,
        "source_roots": [{ "path": m.join("src/test/java"), "kind": "test", "lang": "java" }]
    }))
    .unwrap();
    ws.update(cx, |w, _| w.project = Some(project_model::Project { root: root.display().to_string(), modules: vec![module.clone()] }));
    ws.update_in(cx, |w, window, cx| w.open_file(f.clone(), window, cx));
    cx.run_until_parked();
    // edit without saving
    ws.update(cx, |w, cx| w.tabs[0].update(cx, |v, _| {
        let t = v.editor.buffer.text();
        let at = t.find("old").unwrap();
        v.editor.anchor = at;
        v.editor.head = at + 3;
        v.editor.insert("new");
    }));
    let task = ws.update(cx, |w, cx| w.ensure_built(&module, &[], true, vec!["a.T".into()], None, cx));
    let res = cx.background_executor.block_test(task);
    assert!(std::fs::read_to_string(&f).unwrap().contains("\"new\""), "saved before building");
    let bytes = std::fs::read(&class).unwrap();
    assert!(bytes.starts_with(&[0xCA, 0xFE, 0xBA, 0xBE]), "recompiled by javac ({res:?})");
    assert!(bytes.windows(3).any(|w| w == b"new"), "the class holds the new code");
    std::fs::remove_dir_all(&root).unwrap();
}

/// Git window on a real merge conflict: the conflicted file is listed, the resolver takes the
/// other branch's side, "resolved" + Continue end the merge; the diff view pairs the lines.
#[gpui::test]
fn git_conflict_and_diff(cx: &mut TestAppContext) {
    let root = std::env::temp_dir().join(format!("ui-gitcf-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let git = |args: &[&str]| std::process::Command::new("git").arg("-C").arg(&root).args(args).output().map(|o| o.status.success()).unwrap_or(false);
    if !git(&["init", "-q", "-b", "main"]) {
        return;
    }
    git(&["config", "user.email", "t@t"]);
    git(&["config", "user.name", "T"]);
    let f = root.join("A.java");
    std::fs::write(&f, "class A {\n    int x = 1;\n}\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-qm", "init"]);
    git(&["checkout", "-q", "-b", "feat"]);
    std::fs::write(&f, "class A {\n    int x = 2;\n}\n").unwrap();
    git(&["commit", "-qam", "feat"]);
    git(&["checkout", "-q", "main"]);
    std::fs::write(&f, "class A {\n    int x = 3;\n}\n").unwrap();
    git(&["commit", "-qam", "main"]);
    git(&["merge", "feat"]); // conflict

    let settings = Settings { jdtls_enabled: false, ..Settings::default() };
    let (ws, cx) = cx.add_window_view(|_, cx| Workspace::new(root.clone(), settings, None, cx));
    let wait = |cx: &mut gpui::VisualTestContext, f: &dyn Fn(&Workspace) -> bool| {
        for _ in 0..100 {
            cx.run_until_parked();
            if ws.read_with(cx, |w, _| f(w)) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    };
    ws.update(cx, |w, cx| w.open_git_tab(cx));
    wait(cx, &|w| w.git.tree.changes.iter().any(|c| c.conflicted()));
    assert_eq!(ws.read_with(cx, |w, _| w.git.tree.state), Some(vcs::RepoState::Merging));
    ws.update(cx, |w, cx| {
        w.open_conflicts(f.clone(), cx);
        assert_eq!(w.conflict_view.as_ref().unwrap().conflicts.len(), 1);
        w.pick_conflict(0, vcs::Pick::Theirs, cx);
        w.finish_conflicts(cx);
    });
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "class A {\n    int x = 2;\n}\n");
    wait(cx, &|w| !w.git.tree.changes.iter().any(|c| c.conflicted()));
    // Continue from the panel's state action
    let r = vcs::state_action(&root, vcs::RepoState::Merging, "continue");
    assert!(r.is_ok(), "{r:?}");
    // diff view: an edit against HEAD
    std::fs::write(&f, "class A {\n    int x = 9;\n    int y;\n}\n").unwrap();
    ws.update_in(cx, |w, window, cx| w.open_diff(f.clone(), None, window, cx));
    wait(cx, &|w| w.diff_view.is_some());
    let kinds = ws.read_with(cx, |w, _| w.diff_view.as_ref().unwrap().rows.iter().map(|r| r.kind).collect::<Vec<_>>());
    assert_eq!(kinds, [vcs::RowKind::Same, vcs::RowKind::Changed, vcs::RowKind::Added, vcs::RowKind::Same]);
    std::fs::remove_dir_all(&root).unwrap();
}

/// Git window, SmartGit style: stage one block of two from the diff, commit only that (the
/// other stays in the work tree), the log graph lists it.
#[gpui::test]
fn git_hunk_stage_commit_and_log(cx: &mut TestAppContext) {
    let root = std::env::temp_dir().join(format!("ui-githunk-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let git = |args: &[&str]| std::process::Command::new("git").arg("-C").arg(&root).args(args).output().map(|o| o.status.success()).unwrap_or(false);
    if !git(&["init", "-q", "-b", "main"]) {
        return;
    }
    git(&["config", "user.email", "t@t"]);
    git(&["config", "user.name", "T"]);
    let f = root.join("A.java");
    std::fs::write(&f, "1\n2\n3\n4\n5\n6\n7\n8\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-qm", "init"]);
    std::fs::write(&f, "ONE\n2\n3\n4\n5\n6\n7\nEIGHT\n").unwrap();

    let settings = Settings { jdtls_enabled: false, ..Settings::default() };
    let (ws, cx) = cx.add_window_view(|_, cx| Workspace::new(root.clone(), settings, None, cx));
    let wait = |cx: &mut gpui::VisualTestContext, f: &dyn Fn(&Workspace) -> bool| {
        for _ in 0..150 {
            cx.run_until_parked();
            if ws.read_with(cx, |w, _| f(w)) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        false
    };
    ws.update(cx, |w, cx| w.open_git_tab(cx));
    assert!(wait(cx, &|w| w.git.tree.changes.iter().any(|c| c.unstaged())));
    ws.update(cx, |w, cx| w.load_diff(f.clone(), crate::git_panel::DiffMode::WorkTree, None, cx));
    assert!(wait(cx, &|w| w.diff_view.as_ref().is_some_and(|v| v.blocks.len() == 2)), "two blocks against the index");
    ws.update(cx, |w, cx| w.diff_block_action(true, cx));
    assert!(wait(cx, &|w| w.diff_view.as_ref().is_some_and(|v| v.blocks.len() == 1) && w.git.busy.is_none()), "first block staged, one left");
    assert!(wait(cx, &|w| w.git.tree.changes.iter().any(|c| c.staged() && c.unstaged())), "file is partially staged");
    ws.update(cx, |w, cx| {
        w.diff_view = None;
        w.commit_for_test("first block", cx)
    });
    let head = || std::process::Command::new("git").arg("-C").arg(&root).args(["show", "HEAD:A.java"]).output().map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default();
    assert!(wait(cx, &|_| head() == "ONE\n2\n3\n4\n5\n6\n7\n8\n"), "only the staged block committed: {:?}", head());
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "ONE\n2\n3\n4\n5\n6\n7\nEIGHT\n", "work tree keeps the other block");
    // the log: two commits, newest selected, its file listed
    ws.update_in(cx, |w, window, cx| w.git_log(None, window, cx));
    // the other block is still uncommitted: the window opens on the working tree, with branches loaded
    assert!(wait(cx, &|w| w.log_view.as_ref().is_some_and(|v| v.commits.len() == 2 && v.working && v.refs.locals.iter().any(|b| b.name == "main" && b.current))));
    ws.update(cx, |w, cx| w.log_goto(0, cx));
    assert!(wait(cx, &|w| w.log_view.as_ref().is_some_and(|v| !v.working && v.files.len() == 1)));
    ws.read_with(cx, |w, _| {
        let v = w.log_view.as_ref().unwrap();
        assert_eq!(v.commits[0].subject, "first block");
        assert!(v.commits[0].refs.iter().any(|r| r == "HEAD -> main"));
        assert_eq!(v.rows[0].col, 0);
    });
    std::fs::remove_dir_all(&root).unwrap();
}

/// The Git window's branch list is virtual: with 600 remote branches only the rows in view
/// exist (the first is built, the last is not), and ticking a branch switches the graph from
/// "all branches" to the ticked ones.
#[gpui::test]
fn git_window_branch_list_is_virtual_and_ticks_filter(cx: &mut TestAppContext) {
    let root = std::env::temp_dir().join(format!("ui-gitside-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let git = |args: &[&str]| std::process::Command::new("git").arg("-C").arg(&root).args(args).output().map(|o| o.status.success()).unwrap_or(false);
    if !git(&["init", "-q", "-b", "main"]) {
        return;
    }
    git(&["config", "user.email", "t@t"]);
    git(&["config", "user.name", "T"]);
    std::fs::write(root.join("A.txt"), "1\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-qm", "init"]);

    let settings = Settings { jdtls_enabled: false, ..Settings::default() };
    let (ws, cx) = cx.add_window_view(|_, cx| Workspace::new(root.clone(), settings, None, cx));
    cx.run_until_parked();
    ws.update_in(cx, |w, window, cx| w.git_log(None, window, cx));
    for _ in 0..100 {
        cx.run_until_parked();
        if ws.read_with(cx, |w, _| w.log_view.as_ref().is_some_and(|v| !v.loading && !v.commits.is_empty())) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    ws.update(cx, |w, cx| {
        let v = w.log_view.as_mut().unwrap();
        v.refs.remotes = vec![("origin".to_string(), (0..600).map(|i| format!("b{i:03}")).collect())];
        v.flipped.insert("remote-origin".to_string());
        cx.notify()
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds(sel("side-remote:origin/b000".into())).is_some(), "first remote branch is on screen");
    assert!(cx.debug_bounds(sel("side-remote:origin/b599".into())).is_none(), "the last one is not built");

    // nothing ticked: every branch; one ticked: that branch (+ HEAD)
    assert!(ws.read_with(cx, |w, _| w.log_view.as_ref().unwrap().all));
    ws.update(cx, |w, cx| w.pick_branches(Some(Some("origin/b000".to_string())), cx));
    assert!(ws.read_with(cx, |w, _| {
        let v = w.log_view.as_ref().unwrap();
        !v.all && v.branches == ["origin/b000".to_string()]
    }));
    ws.update(cx, |w, cx| w.pick_branches(Some(Some("origin/b000".to_string())), cx));
    assert!(ws.read_with(cx, |w, _| w.log_view.as_ref().unwrap().all), "unticking the last one shows every branch again");
    std::fs::remove_dir_all(&root).unwrap();
}

/// The Maven tool window takes the project tree's place (and gives it back).
#[gpui::test]
fn maven_window_replaces_project_tree(cx: &mut TestAppContext) {
    let root = std::env::temp_dir().join(format!("ui-mvnside-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let settings = Settings { jdtls_enabled: false, ..Settings::default() };
    let (ws, cx) = cx.add_window_view(|_, cx| Workspace::new(root.clone(), settings, None, cx));
    cx.run_until_parked();
    assert!(cx.debug_bounds("project-tree").is_some(), "the project tree is shown first");
    assert!(cx.debug_bounds("maven-window").is_none());
    ws.update(cx, |w, cx| {
        w.maven_side = true;
        cx.notify()
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("maven-window").is_some(), "Maven is in the tree's place");
    // the Profiles field once collapsed to a few pixels: it must fill the column
    let field = cx.debug_bounds("mvn-profiles").expect("profiles field");
    assert!(field.size.width > gpui::px(150.), "profiles field is {:?} wide", field.size.width);
    assert!(ws.read_with(cx, |w, _| !w.panel_open), "the bottom panel is not used for Maven");
    ws.update(cx, |w, cx| {
        w.maven_side = false;
        cx.notify()
    });
    cx.run_until_parked();
    assert!(ws.read_with(cx, |w, _| !w.maven_side), "closing Maven brings the tree back");
    std::fs::remove_dir_all(&root).unwrap();
}

/// Undo last commit: HEAD goes back, the changes are staged, the message returns to the commit box.
#[gpui::test]
fn git_undo_last_commit_restores_message_and_stages(cx: &mut TestAppContext) {
    let root = std::env::temp_dir().join(format!("ui-gitundo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let git = |args: &[&str]| std::process::Command::new("git").arg("-C").arg(&root).args(args).output().map(|o| o.status.success()).unwrap_or(false);
    if !git(&["init", "-q", "-b", "main"]) {
        return;
    }
    git(&["config", "user.email", "t@t"]);
    git(&["config", "user.name", "T"]);
    std::fs::write(root.join("A.txt"), "1\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-qm", "first"]);
    std::fs::write(root.join("A.txt"), "2\n").unwrap();
    git(&["commit", "-qam", "second commit message"]);

    let settings = Settings { jdtls_enabled: false, ..Settings::default() };
    let (ws, cx) = cx.add_window_view(|_, cx| Workspace::new(root.clone(), settings, None, cx));
    cx.run_until_parked();
    ws.update_in(cx, |w, window, cx| w.git_log(None, window, cx));
    ws.update_in(cx, |w, window, cx| w.git_undo_last_commit(window, cx));
    let last = || {
        let out = std::process::Command::new("git").arg("-C").arg(&root).args(["log", "--format=%s", "-n1"]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    for _ in 0..150 {
        cx.run_until_parked();
        if last() == "first" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(last(), "first", "HEAD went back one commit");
    for _ in 0..50 {
        cx.run_until_parked();
        if ws.read_with(cx, |w, cx| w.git_ui.as_ref().is_some_and(|u| u.msg.text(cx) == "second commit message")) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(ws.read_with(cx, |w, cx| w.git_ui.as_ref().unwrap().msg.text(cx) == "second commit message"), "the message is back in the commit box");
    assert_eq!(std::fs::read_to_string(root.join("A.txt")).unwrap(), "2\n", "work tree untouched");
    std::fs::remove_dir_all(&root).unwrap();
}

/// Clone from the project chooser: a repository (here a local one) is cloned into the chosen
/// folder and opened as a project.
#[gpui::test]
fn clone_from_chooser_opens_the_project(cx: &mut TestAppContext) {
    let base = std::env::temp_dir().join(format!("ui-clone-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let src = base.join("source-repo");
    std::fs::create_dir_all(&src).unwrap();
    let git = |args: &[&str]| std::process::Command::new("git").arg("-C").arg(&src).args(args).output().map(|o| o.status.success()).unwrap_or(false);
    if !git(&["init", "-q", "-b", "main"]) {
        return;
    }
    git(&["config", "user.email", "t@t"]);
    git(&["config", "user.name", "T"]);
    std::fs::write(src.join("A.txt"), "1\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-qm", "init"]);

    // opening a project writes the "open" / "recent" lists: keep them out of the real state dir
    let old_state = std::env::var_os("XDG_STATE_HOME");
    unsafe { std::env::set_var("XDG_STATE_HOME", base.join("state")) };
    let settings = Settings { jdtls_enabled: false, ..Settings::default() };
    let (shell, cx) = cx.add_window_view(|window, cx| crate::shell::Shell::new(settings, None, Vec::new(), None, window, cx));
    cx.run_until_parked();
    shell.update_in(cx, |s, window, cx| s.open_clone(window, cx));
    let parent = base.join("projects");
    shell.update(cx, |s, cx| {
        let f = s.clone.as_ref().expect("clone dialog open");
        f.set_url(src.to_str().unwrap(), cx);
        f.set_parent(parent.to_str().unwrap(), cx);
    });
    cx.run_until_parked();
    assert_eq!(shell.read_with(cx, |s, cx| s.clone.as_ref().unwrap().folder_text(cx)), "source-repo", "the folder name comes from the address");
    shell.update_in(cx, |s, window, cx| s.start_clone(window, cx));
    for _ in 0..200 {
        cx.run_until_parked();
        if shell.read_with(cx, |s, _| s.clone.is_none()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(parent.join("source-repo/A.txt").is_file(), "cloned into the chosen folder");
    assert!(shell.read_with(cx, |s, _| s.clone.is_none() && s.chooser.is_none()), "the dialog and the chooser are gone");
    assert!(shell.read_with(cx, |s, _| s.has_project(&parent.join("source-repo"))), "the new project is open");
    match old_state {
        Some(v) => unsafe { std::env::set_var("XDG_STATE_HOME", v) },
        None => unsafe { std::env::remove_var("XDG_STATE_HOME") },
    }
    std::fs::remove_dir_all(&base).unwrap();
}
