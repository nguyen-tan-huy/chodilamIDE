//! Workspace side of the language server: starts jdtls once the Maven model is known,
//! routes server messages (status, diagnostics, applyEdit), opens definition targets
//! (incl. jdt:// class files), navigation history, references and Problems lists.
use crate::editor_view::{EditorEvent, EditorView};
use crate::picker::Picker;
use crate::search::Hit;
use crate::workspace::Workspace;
use futures::StreamExt;
use gpui::{AppContext, Context, Entity, Window};
use lsp_client::{Client, ServerMessage, jdtls, path_to_uri, uri_to_path};
use serde_json::{Value, json};
use std::path::PathBuf;

/// LSP FileChangeType.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FileChange {
    Created = 1,
    Changed = 2,
    Deleted = 3,
}

/// One jdtls background job.
#[derive(Debug, Clone)]
pub struct Job {
    pub task: String,
    pub status: String,
    /// done / total, when jdtls knows the total
    pub fraction: Option<f32>,
    pub at: std::time::Instant,
}

impl Job {
    /// Import / build jobs make the classes a run needs; indexing ("Searching…") and
    /// diagnostics don't, so those never hold a run back.
    pub fn blocks_run(&self) -> bool {
        const BLOCKING: &[&str] = &["Building", "Importing", "Update Maven", "Refreshing", "Synchroniz", "Initializ", "Compil"];
        BLOCKING.iter().any(|b| self.task.contains(b))
    }
}

/// A run that waits for jdtls to finish importing/building.
pub enum PendingRun {
    Launch { cfg: runner::config::RunConfig, debug: bool },
    Tests { module: String, targets: Vec<runner::test::TestTarget>, debug: bool },
}

impl PendingRun {
    pub fn label(&self) -> String {
        match self {
            PendingRun::Launch { cfg, debug } => format!("{} {}", if *debug { "Debug" } else { "Run" }, cfg.name),
            PendingRun::Tests { targets, debug, .. } => {
                let what = targets.first().map(|t| t.class.rsplit('.').next().unwrap_or(&t.class).to_string()).unwrap_or_default();
                format!("{} test {what}", if *debug { "Debug" } else { "Run" })
            }
        }
    }
}

pub enum LspState {
    Off,
    Starting(String),
    Ready,
    Failed(String),
}

/// Where the cursor was, for Back/Forward (Ctrl+Alt+←/→).
#[derive(Clone)]
pub struct NavEntry {
    tab_key: String,
    at: usize,
}

impl Workspace {
    pub(crate) fn start_lsp(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.lsp_state, LspState::Off) {
            return;
        }
        let Some(project) = self.project.as_ref() else { return };
        let has_java = project.modules.iter().any(|m| m.source_roots.iter().any(|s| s.lang == "java"));
        if !has_java {
            return;
        }
        let root = self.root.clone();
        let modules: Vec<PathBuf> = project.modules.iter().map(|m| PathBuf::from(&m.path)).collect();
        let mem = jdtls::Memory { xmx: self.settings.jdtls_xmx.clone(), xms: self.settings.jdtls_xms.clone() };
        self.lsp_state = LspState::Starting("đang khởi động jdtls…".into());
        cx.notify();

        let spawn = cx.background_executor().spawn(async move {
            // jdtls runs on 21+: download Temurin 21 when the machine has none
            toolchain::managed::ensure_jdk(21)?;
            toolchain::managed::ensure_jdtls()?;
            let tools = toolchain::find_tools();
            let jdks = toolchain::find_jdks();
            let cfg = jdtls::config(&root, &modules, &tools, &jdks, &mem)?;
            let (client, events) = Client::spawn(&cfg.program, &cfg.args, &root, &cfg.env, cfg.settings.clone())?;
            let init = client.request("initialize", jdtls::initialize_params(&root, cfg.init_options.clone()));
            anyhow::Ok((client, events, init, tools.notes))
        });
        cx.spawn(async move |this, cx| {
            let (client, mut events, init, notes) = match spawn.await {
                Ok(v) => v,
                Err(e) => {
                    let _ = this.update(cx, |ws, cx| {
                        ws.lsp_state = LspState::Failed(format!("{e:#}"));
                        cx.notify();
                    });
                    return;
                }
            };
            match init.response().await {
                Ok(res) => {
                    *client.capabilities.lock().unwrap() = res["capabilities"].clone();
                    client.notify("initialized", json!({}));
                }
                Err(e) => {
                    let _ = this.update(cx, |ws, cx| {
                        ws.lsp_state = LspState::Failed(format!("initialize: {e:#}"));
                        cx.notify();
                    });
                    return;
                }
            }
            let ok = this
                .update(cx, |ws, cx| {
                    ws.lsp = Some(client.clone());
                    ws.lsp_state = LspState::Starting("jdtls đang import project…".into());
                    if let Some(n) = notes.first() {
                        ws.message = Some(n.clone());
                    }
                    for t in ws.tabs.clone() {
                        ws.attach_tab(&t, cx);
                    }
                    cx.notify();
                })
                .is_ok();
            if !ok {
                return;
            }
            while let Some(msg) = events.next().await {
                if this.update(cx, |ws, cx| ws.on_server_message(msg, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn on_server_message(&mut self, msg: ServerMessage, cx: &mut Context<Self>) {
        match msg {
            ServerMessage::Notification { method, params } => match method.as_str() {
                // jdtls background jobs: importing, building, refreshing… (progressReportProvider)
                "language/progressReport" => {
                    let id = params["id"].as_str().unwrap_or("").to_string();
                    if params["complete"].as_bool().unwrap_or(false) {
                        self.lsp_jobs.remove(&id);
                    } else {
                        let total = params["totalWork"].as_u64().unwrap_or(0);
                        let done = params["workDone"].as_u64().unwrap_or(0);
                        self.lsp_jobs.insert(
                            id,
                            Job {
                                task: params["task"].as_str().unwrap_or("").to_string(),
                                status: params["status"].as_str().or(params["subTask"].as_str()).unwrap_or("").to_string(),
                                fraction: (total > 0).then(|| (done as f32 / total as f32).clamp(0., 1.)),
                                at: std::time::Instant::now(),
                            },
                        );
                    }
                }
                "language/status" => {
                    let ty = params["type"].as_str().unwrap_or("");
                    let text = params["message"].as_str().unwrap_or("").to_string();
                    match ty {
                        "ServiceReady" => {
                            self.lsp_state = LspState::Ready;
                            self.jsf_check_all(cx);
                        }
                        "Error" => self.message = Some(format!("jdtls: {text}")),
                        _ if !matches!(self.lsp_state, LspState::Ready) => self.lsp_state = LspState::Starting(text),
                        _ => {}
                    }
                }
                "textDocument/publishDiagnostics" => {
                    let uri = params["uri"].as_str().unwrap_or("").to_string();
                    let diags = params["diagnostics"].as_array().cloned().unwrap_or_default();
                    for t in &self.tabs {
                        if t.read(cx).lsp_uri() == Some(uri.as_str()) {
                            t.update(cx, |v, cx| {
                                v.set_diagnostics(&diags);
                                cx.notify()
                            });
                        }
                    }
                    if diags.is_empty() {
                        self.diagnostics.remove(&uri);
                    } else {
                        self.diagnostics.insert(uri, diags);
                    }
                }
                "window/showMessage" => self.message = params["message"].as_str().map(|s| format!("jdtls: {s}")),
                _ => {}
            },
            ServerMessage::Request { method, params } => {
                if method == "workspace/applyEdit" {
                    self.apply_workspace_edit(&params["edit"], cx);
                }
            }
            ServerMessage::Exited => {
                self.lsp = None;
                self.lsp_state = LspState::Failed("jdtls đã thoát".into());
                for t in self.tabs.clone() {
                    t.update(cx, |v, _| v.detach_lsp());
                }
            }
        }
        cx.notify();
    }

    /// Applies a WorkspaceEdit: open tabs get the edits in their buffer (unsaved, undoable);
    /// files that are not open are edited on disk; rename/create/delete file operations
    /// (e.g. renaming a public class renames its .java file) are carried out too.
    pub(crate) fn apply_workspace_edit(&mut self, edit: &Value, cx: &mut Context<Self>) {
        let mut ops: Vec<Value> = Vec::new();
        if let Some(dc) = edit["documentChanges"].as_array() {
            ops.extend(dc.iter().cloned());
        }
        if let Some(changes) = edit["changes"].as_object() {
            for (uri, edits) in changes {
                ops.push(json!({ "textDocument": { "uri": uri }, "edits": edits }));
            }
        }
        let (mut files, mut errors) = (0usize, Vec::new());
        let mut disk: Vec<(PathBuf, FileChange)> = Vec::new();
        for op in ops {
            match op["kind"].as_str() {
                Some("rename") => {
                    let (Some(old), Some(new)) = (op["oldUri"].as_str().and_then(uri_to_path), op["newUri"].as_str().and_then(uri_to_path)) else { continue };
                    if let Err(e) = std::fs::rename(&old, &new) {
                        errors.push(format!("đổi tên file: {e}"));
                        continue;
                    }
                    disk.push((old.clone(), FileChange::Deleted));
                    disk.push((new.clone(), FileChange::Created));
                    let new_uri = path_to_uri(&new);
                    let client = self.lsp.clone();
                    for t in self.tabs.clone() {
                        if t.read(cx).path() == Some(&old) {
                            t.update(cx, |v, _| {
                                v.detach_lsp();
                                v.editor.buffer.path = Some(new.clone());
                                if let Some(c) = client.clone() {
                                    v.attach_lsp(c, new_uri.clone(), true);
                                }
                            });
                        }
                    }
                    self.tree.refresh();
                }
                Some("create") => {
                    if let Some(p) = op["uri"].as_str().and_then(uri_to_path) {
                        if let Some(d) = p.parent() {
                            let _ = std::fs::create_dir_all(d);
                        }
                        let _ = std::fs::OpenOptions::new().create(true).append(true).open(&p);
                        disk.push((p.clone(), FileChange::Created));
                        self.tree.refresh();
                    }
                }
                Some("delete") => {
                    if let Some(p) = op["uri"].as_str().and_then(uri_to_path) {
                        let _ = std::fs::remove_file(&p);
                        disk.push((p.clone(), FileChange::Deleted));
                        self.tree.refresh();
                    }
                }
                _ => {
                    let uri = op["textDocument"]["uri"].as_str().unwrap_or("").to_string();
                    let edits = op["edits"].as_array().cloned().unwrap_or_default();
                    if edits.is_empty() {
                        continue;
                    }
                    files += 1;
                    if let Some(t) = self.tabs.iter().find(|t| t.read(cx).lsp_uri() == Some(uri.as_str())).cloned() {
                        t.update(cx, |v, cx| {
                            v.apply_text_edits(&edits);
                            cx.notify()
                        });
                    } else if let Some(p) = uri_to_path(&uri) {
                        match std::fs::read_to_string(&p) {
                            Ok(text) => {
                                let new = lsp_client::text::apply_edits(&text, &edits);
                                match std::fs::write(&p, new) {
                                    Ok(()) => disk.push((p.clone(), FileChange::Changed)),
                                    Err(e) => errors.push(format!("{}: {e}", p.display())),
                                }
                            }
                            Err(e) => errors.push(format!("{}: {e}", p.display())),
                        }
                    }
                }
            }
        }
        self.notify_files(&disk);
        self.message = Some(if errors.is_empty() {
            format!("Đã sửa {files} file (file đang mở: nhớ Ctrl+S)")
        } else {
            format!("Lỗi khi sửa: {}", errors.join("; "))
        });
        cx.notify();
    }

    /// Runs a code action or command from the Alt+Enter menu.
    fn run_action(&mut self, action: Value, cx: &mut Context<Self>) {
        let Some(client) = self.lsp.clone() else { return };
        // a bare Command (title/command/arguments)
        if action["command"].is_string() {
            self.execute_command(&action, cx);
            return;
        }
        if action["edit"].is_object() || action["command"].is_object() {
            if action["edit"].is_object() {
                self.apply_workspace_edit(&action["edit"], cx);
            }
            if action["command"].is_object() {
                self.execute_command(&action["command"], cx);
            }
            return;
        }
        // lazily computed action: resolve first
        let req = client.request("codeAction/resolve", action);
        cx.spawn(async move |this, cx| {
            let res = req.response().await;
            let _ = this.update(cx, |ws, cx| match res {
                Ok(a) if a["edit"].is_object() || a["command"].is_object() => ws.run_action(a, cx),
                Ok(_) => ws.message = Some("Hành động không có thay đổi nào".into()),
                Err(e) => ws.message = Some(format!("{e}")),
            });
        })
        .detach();
    }

    /// jdtls' "prompt" commands of Alt+Insert: the IDE answers the prompt itself with every
    /// field / member preselected (IntelliJ's dialog with all checked), then applies the edit.
    fn generate_prompt(&mut self, command: &str, params: Value, cx: &mut Context<Self>) -> bool {
        let Some(client) = self.lsp.clone() else { return false };
        let (check, generate): (&str, &str) = match command {
            "java.action.generateToStringPrompt" => ("java/checkToStringStatus", "java/generateToString"),
            "java.action.hashCodeEqualsPrompt" => ("java/checkHashCodeEqualsStatus", "java/generateHashCodeEquals"),
            "java.action.generateConstructorsPrompt" => ("java/checkConstructorsStatus", "java/generateConstructors"),
            "java.action.generateAccessorsPrompt" => ("java/resolveUnimplementedAccessors", "java/generateAccessors"),
            "java.action.overrideMethodsPrompt" => ("java/listOverridableMethods", "java/addOverridableMethods"),
            _ => return false,
        };
        let command = command.to_string();
        cx.spawn(async move |this, cx| {
            let status = match client.request(check, params.clone()).response().await {
                Ok(s) => s,
                Err(e) => {
                    let _ = this.update(cx, |ws, cx| {
                        ws.message = Some(format!("{e}"));
                        cx.notify()
                    });
                    return;
                }
            };
            let body = match command.as_str() {
                "java.action.generateToStringPrompt" => json!({ "context": params, "fields": status["fields"] }),
                "java.action.hashCodeEqualsPrompt" => json!({ "context": params, "fields": status["fields"], "regenerate": true }),
                "java.action.generateConstructorsPrompt" => {
                    let ctors: Vec<Value> = status["constructors"].as_array().and_then(|a| a.first().cloned()).into_iter().collect();
                    json!({ "context": params, "constructors": ctors, "fields": status["fields"] })
                }
                "java.action.generateAccessorsPrompt" => json!({ "context": params, "accessors": status }),
                // only what still has to be implemented (abstract / interface methods), like IntelliJ's Implement Methods
                _ => {
                    let all = status["methods"].as_array().cloned().unwrap_or_default();
                    let todo: Vec<Value> = all.iter().filter(|m| m["unimplemented"].as_bool() == Some(true)).cloned().collect();
                    json!({ "context": params, "overridableMethods": if todo.is_empty() { all } else { todo } })
                }
            };
            let res = client.request(generate, body).response().await;
            let _ = this.update(cx, |ws, cx| {
                match res {
                    Ok(edit) if edit.is_object() => ws.apply_workspace_edit(&edit, cx),
                    Ok(_) => ws.message = Some("Không có gì để generate".into()),
                    Err(e) => ws.message = Some(format!("{e}")),
                }
                cx.notify()
            });
        })
        .detach();
        true
    }

    fn execute_command(&mut self, cmd: &Value, cx: &mut Context<Self>) {
        let Some(client) = self.lsp.clone() else { return };
        if let Some(c) = cmd["command"].as_str() {
            if self.generate_prompt(c, cmd["arguments"][0].clone(), cx) {
                return;
            }
        }
        // results that change files come back as workspace/applyEdit requests
        let req = client.request("workspace/executeCommand", json!({ "command": cmd["command"], "arguments": cmd["arguments"].clone() }));
        cx.spawn(async move |this, cx| {
            if let Err(e) = req.response().await {
                let _ = this.update(cx, |ws, cx| {
                    ws.message = Some(format!("{e}"));
                    cx.notify()
                });
            }
        })
        .detach();
    }

    /// Hooks a newly opened tab up to the server (didOpen for .java files) and its events.
    pub(crate) fn attach_tab(&mut self, t: &Entity<EditorView>, cx: &mut Context<Self>) {
        let Some(client) = self.lsp.clone() else { return };
        let Some(path) = t.read(cx).path().cloned() else { return };
        if t.read(cx).lsp_uri().is_some() || path.extension().and_then(|e| e.to_str()) != Some("java") {
            return;
        }
        let uri = path_to_uri(&path);
        let diags = self.diagnostics.get(&uri).cloned().unwrap_or_default();
        t.update(cx, |v, _| {
            v.attach_lsp(client, uri, true);
            v.set_diagnostics(&diags);
        });
    }

    pub(crate) fn subscribe_tab(&mut self, t: &Entity<EditorView>, window: &mut Window, cx: &mut Context<Self>) {
        cx.subscribe_in(t, window, |ws, _, ev: &EditorEvent, window, cx| match ev {
            EditorEvent::Goto(locs) => {
                if let Some(loc) = locs.first() {
                    ws.push_nav(cx);
                    ws.open_location(loc.clone(), window, cx);
                }
            }
            EditorEvent::Usages { locs, uri, line, col } => ws.usages_with_xhtml(locs.clone(), uri.clone(), *line, *col, window, cx),
            EditorEvent::JsfGoto(at) => {
                let Some(t) = ws.active.and_then(|i| ws.tabs.get(i)).cloned() else { return };
                ws.jsf_goto(t, *at, window, cx)
            }
            EditorEvent::JsfComplete { at, explicit } => {
                let Some(t) = ws.active.and_then(|i| ws.tabs.get(i)).cloned() else { return };
                ws.jsf_complete(t, *at, *explicit, window, cx)
            }
            EditorEvent::JsfHover(at) => {
                let Some(t) = ws.active.and_then(|i| ws.tabs.get(i)).cloned() else { return };
                ws.jsf_hover(t, *at, window, cx)
            }
            EditorEvent::SpringGoto(at) => {
                let Some(t) = ws.active.and_then(|i| ws.tabs.get(i)).cloned() else { return };
                ws.spring_goto(t, *at, window, cx)
            }
            EditorEvent::SpringComplete { at, explicit } => {
                let Some(t) = ws.active.and_then(|i| ws.tabs.get(i)).cloned() else { return };
                ws.spring_complete(t, *at, *explicit, window, cx)
            }
            EditorEvent::SpringHover(at) => {
                let Some(t) = ws.active.and_then(|i| ws.tabs.get(i)).cloned() else { return };
                ws.spring_hover(t, *at, window, cx)
            }
            EditorEvent::Locations { title, locs } => ws.show_locations(title, locs, window, cx),
            EditorEvent::RunHttp(line) => ws.run_http_request(*line, cx),
            EditorEvent::OpenPath(p) => {
                let here = ws.active.and_then(|i| ws.tabs.get(i)).and_then(|t| t.read(cx).path().and_then(|f| f.parent().map(|d| d.to_path_buf())));
                let cands = [here.map(|d| d.join(p)), Some(ws.root.join(p)), Some(PathBuf::from(p))];
                match cands.into_iter().flatten().find(|c| c.is_file()) {
                    Some(f) => ws.open_file(f, window, cx),
                    None => ws.message = Some(format!("Không có file {p}")),
                }
            }
            EditorEvent::Hierarchy { kind, params } => ws.show_hierarchy(*kind, params.clone(), window, cx),
            EditorEvent::ApplyEdit(edit) => ws.apply_workspace_edit(edit, cx),
            EditorEvent::Renamed { old, new } => {
                let Some(t) = ws.active.and_then(|i| ws.tabs.get(i)).cloned() else { return };
                ws.jsf_after_rename(t, old.clone(), new.clone(), cx)
            }
            EditorEvent::ToggleBreakpoint(line) => {
                let Some(t) = ws.active.and_then(|i| ws.tabs.get(i)).cloned() else { return };
                if let Some(p) = t.read(cx).path().cloned() {
                    ws.toggle_breakpoint(p, *line, cx);
                }
            }
            EditorEvent::ContextMenu { at, line } => ws.editor_menu(*at, *line, cx),
            EditorEvent::DebugHover { expr, at } => ws.show_value_popup(expr.clone(), *at, cx),
            EditorEvent::DismissPopups => {
                if ws.value_popup.take().is_some() {
                    cx.notify();
                }
            }
            EditorEvent::EditBreakpoint(line) => {
                let Some(t) = ws.active.and_then(|i| ws.tabs.get(i)).cloned() else { return };
                if let Some(p) = t.read(cx).path().cloned() {
                    ws.open_bp_dialog(p, *line, window, cx);
                }
            }
            EditorEvent::RunTest { class, method, debug } => {
                let Some(t) = ws.active.and_then(|i| ws.tabs.get(i)).cloned() else { return };
                let Some(p) = t.read(cx).path().cloned() else { return };
                let Some(module) = ws.project.as_ref().and_then(|pr| pr.find_module_for_file(&p.display().to_string())).map(|m| m.path.clone()) else {
                    ws.message = Some("File test không thuộc module Maven nào".into());
                    return;
                };
                ws.remember_test_profile(&p, class, method.as_deref(), *debug);
                let targets = vec![runner::test::TestTarget { class: class.clone(), methods: method.iter().cloned().collect() }];
                ws.run_tests(&module, targets, *debug, cx);
            }
            EditorEvent::RunMain { main_class, debug } => {
                let Some(t) = ws.active.and_then(|i| ws.tabs.get(i)).cloned() else { return };
                if let Some(p) = t.read(cx).path().cloned() {
                    ws.run_main(&p, main_class, *debug, window, cx);
                }
            }
            EditorEvent::RunAction(action) => ws.run_action(action.clone(), cx),
            EditorEvent::Structure(syms) => {
                let Some(t) = ws.active.and_then(|i| ws.tabs.get(i)).cloned() else { return };
                let Some(path) = t.read(cx).path().cloned() else { return };
                let hits = syms
                    .iter()
                    .map(|(name, detail, line, col16, depth)| {
                        let text = t.read(cx).editor.buffer.line_text(*line);
                        let col = lsp_client::text::utf16_col_to_byte(&text, *col16);
                        Hit { path: path.clone(), line: *line, col, text: format!("{}{name}  {detail}", "  ".repeat(*depth)), mark: None }
                    })
                    .collect();
                ws.open_list(format!("Cấu trúc {}", t.read(cx).title()), hits, window, cx);
            }
        })
        .detach();
    }

    fn tab_key(t: &EditorView) -> Option<String> {
        t.lsp_uri().map(String::from).or_else(|| t.path().map(|p| path_to_uri(p)))
    }

    pub(crate) fn push_nav(&mut self, cx: &mut Context<Self>) {
        if let Some(t) = self.active.and_then(|i| self.tabs.get(i)) {
            let v = t.read(cx);
            if let Some(key) = Self::tab_key(v) {
                self.nav_back.push(NavEntry { tab_key: key, at: v.editor.head });
                self.nav_forward.clear();
                if self.nav_back.len() > 100 {
                    self.nav_back.remove(0);
                }
            }
        }
    }

    pub(crate) fn navigate(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        let entry = if back { self.nav_back.pop() } else { self.nav_forward.pop() };
        let Some(entry) = entry else { return };
        // remember where we are for the opposite direction
        if let Some(t) = self.active.and_then(|i| self.tabs.get(i)) {
            let v = t.read(cx);
            if let Some(key) = Self::tab_key(v) {
                let here = NavEntry { tab_key: key, at: v.editor.head };
                if back { self.nav_forward.push(here) } else { self.nav_back.push(here) }
            }
        }
        if let Some(i) = self.tabs.iter().position(|t| Self::tab_key(t.read(cx)).as_deref() == Some(entry.tab_key.as_str())) {
            self.activate(i, window, cx);
        } else if let Some(p) = uri_to_path(&entry.tab_key) {
            self.open_file(p, window, cx);
        } else {
            return;
        }
        if let Some(t) = self.active.and_then(|i| self.tabs.get(i)).cloned() {
            t.update(cx, |v, cx| {
                v.editor.set_cursor(entry.at, false);
                v.request_center();
                cx.notify()
            });
        }
    }

    /// Opens an LSP Location: a file on disk, or a class inside a jar (jdt://…).
    pub(crate) fn open_location(&mut self, loc: Value, window: &mut Window, cx: &mut Context<Self>) {
        let uri = loc["uri"].as_str().or(loc["targetUri"].as_str()).unwrap_or("").to_string();
        let range = if loc["range"].is_object() { loc["range"].clone() } else { loc["targetSelectionRange"].clone() };
        let line = range["start"]["line"].as_u64().unwrap_or(0) as usize;
        let ch = range["start"]["character"].as_u64().unwrap_or(0) as usize;
        let goto = move |t: &Entity<EditorView>, cx: &mut Context<Self>| {
            t.update(cx, |v, cx| {
                let at = v.editor.buffer.byte_of_lsp_position(line, ch);
                v.editor.set_cursor(at, false);
                v.request_center();
                cx.notify()
            })
        };
        if let Some(path) = uri_to_path(&uri) {
            self.open_file(path, window, cx);
            if let Some(t) = self.active.and_then(|i| self.tabs.get(i)).cloned() {
                goto(&t, cx);
            }
            return;
        }
        if !uri.starts_with("jdt://") {
            self.message = Some(format!("Không mở được {uri}"));
            return;
        }
        if let Some(i) = self.tabs.iter().position(|t| t.read(cx).lsp_uri() == Some(uri.as_str())) {
            self.activate(i, window, cx);
            let t = self.tabs[i].clone();
            goto(&t, cx);
            return;
        }
        let Some(client) = self.lsp.clone() else { return };
        let req = client.request("java/classFileContents", json!({ "uri": uri }));
        cx.spawn_in(window, async move |this, cx| {
            let text = req.response().await.ok().and_then(|v| v.as_str().map(String::from));
            let _ = this.update_in(cx, |ws, window, cx| {
                let Some(text) = text.filter(|t| !t.is_empty()) else {
                    ws.message = Some("jdtls không trả nội dung class (thiếu sources jar?)".into());
                    cx.notify();
                    return;
                };
                let title = class_title(&uri);
                let mut buffer = editor_core::Buffer::from_text(&text);
                // a fake path gives Java highlighting; it is never saved (read-only)
                buffer.path = None;
                let tab_width = ws.settings.tab_width;
                let view = cx.new(|cx| {
                    let mut v = EditorView::new_with_lang(buffer, tab_width, syntax::Lang::Java, cx);
                    v.read_only = true;
                    v.title_override = Some(title);
                    v.attach_lsp(client, uri.clone(), false);
                    v
                });
                cx.observe(&view, |_, _, cx| cx.notify()).detach();
                ws.subscribe_tab(&view, window, cx);
                ws.tabs.push(view.clone());
                ws.activate(ws.tabs.len() - 1, window, cx);
                goto(&view, cx);
            });
        })
        .detach();
    }

    pub(crate) fn show_locations(&mut self, title: &str, locs: &[Value], window: &mut Window, cx: &mut Context<Self>) {
        let mut hits = Vec::new();
        for loc in locs {
            let Some(path) = loc["uri"].as_str().and_then(uri_to_path) else { continue };
            let line = loc["range"]["start"]["line"].as_u64().unwrap_or(0) as usize;
            let ch = loc["range"]["start"]["character"].as_u64().unwrap_or(0) as usize;
            let text = self.line_text(&path, line, cx);
            let col = lsp_client::text::utf16_col_to_byte(&text, ch);
            // the referenced identifier at `col` is the part drawn bold
            let len = text[col.min(text.len())..].find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$')).unwrap_or(text.len() - col.min(text.len()));
            hits.push(Hit::snippet(path, line, &text, col, len));
        }
        self.open_list(format!("{title} ({})", hits.len()), hits, window, cx);
    }

    /// Problems (Alt+6): every error/warning jdtls reported, errors first.
    pub(crate) fn show_problems(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut items: Vec<(u8, Hit)> = Vec::new();
        for (uri, diags) in &self.diagnostics {
            let Some(path) = uri_to_path(uri) else { continue };
            for d in diags {
                let sev = d["severity"].as_u64().unwrap_or(1) as u8;
                if sev > 2 {
                    continue;
                }
                let line = d["range"]["start"]["line"].as_u64().unwrap_or(0) as usize;
                let tag = if sev == 1 { "✖" } else { "⚠" };
                items.push((sev, Hit { path: path.clone(), line, col: 0, text: format!("{tag} {}", d["message"].as_str().unwrap_or("")), mark: None }));
            }
        }
        items.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.path.cmp(&b.1.path)).then(a.1.line.cmp(&b.1.line)));
        let hits: Vec<Hit> = items.into_iter().map(|(_, h)| h).collect();
        let title = match &self.lsp_state {
            LspState::Ready => format!("Problems ({})", hits.len()),
            _ => format!("Problems ({}) - jdtls chưa xong", hits.len()),
        };
        self.open_list(title, hits, window, cx);
    }

    fn line_text(&self, path: &std::path::Path, line: usize, cx: &Context<Self>) -> String {
        if let Some(t) = self.tabs.iter().find(|t| t.read(cx).path().map(|p| p.as_path()) == Some(path)) {
            return t.read(cx).editor.buffer.line_text(line);
        }
        std::fs::read_to_string(path).ok().and_then(|s| s.lines().nth(line).map(String::from)).unwrap_or_default()
    }

    /// Ctrl+N: workspace symbols (classes, methods…) by name, live from jdtls.
    pub(crate) fn search_everywhere(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let client = self.lsp.clone();
        let picker = cx.new(|cx| Picker::everywhere(root, client, window, cx));
        self.show_picker(picker, window, cx);
    }

    pub(crate) fn goto_symbol(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(client) = self.lsp.clone() else {
            self.message = Some("jdtls chưa chạy".into());
            return;
        };
        let root = self.root.clone();
        let picker = cx.new(|cx| Picker::symbols(root, client, window, cx));
        self.show_picker(picker, window, cx);
    }

    /// Type hierarchy (Ctrl+H: supertypes ↑ then subtypes ↓, indented), call hierarchy
    /// (Ctrl+Alt+H: who calls this, 3 levels), or Ctrl+U (jump to the super method/class).
    pub(crate) fn show_hierarchy(&mut self, kind: crate::editor_view::Hierarchy, params: Value, window: &mut Window, cx: &mut Context<Self>) {
        use crate::editor_view::Hierarchy;
        let Some(client) = self.lsp.clone() else { return };
        let prepare = if kind == Hierarchy::Calls { "textDocument/prepareCallHierarchy" } else { "textDocument/prepareTypeHierarchy" };
        let req = client.request(prepare, params.clone());
        cx.spawn_in(window, async move |this, cx| {
            let items = req.response().await.ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
            let Some(root) = items.into_iter().next() else {
                // Ctrl+U on a method: jdtls has no "super method" request, the type's supertypes
                // are the closest thing
                let _ = this.update(cx, |ws, cx| {
                    ws.message = Some(match kind {
                        Hierarchy::Calls => "Đặt con trỏ trên tên method".into(),
                        _ => "Đặt con trỏ trên tên class / interface".into(),
                    });
                    cx.notify()
                });
                return;
            };
            let name = root["name"].as_str().unwrap_or("").to_string();
            // (depth, item) rows, depth < 0 = supertypes
            let mut rows: Vec<(i32, Value)> = Vec::new();
            match kind {
                Hierarchy::Types | Hierarchy::Super => {
                    let mut frontier = vec![(root.clone(), 0)];
                    while let Some((item, d)) = frontier.pop() {
                        if d <= -6 {
                            continue;
                        }
                        let sup = client.request("typeHierarchy/supertypes", json!({ "item": item })).response().await.ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
                        for s in sup {
                            if s["name"].as_str() == Some("Object") {
                                continue;
                            }
                            rows.push((d - 1, s.clone()));
                            if kind == Hierarchy::Types {
                                frontier.push((s, d - 1));
                            }
                        }
                    }
                    if kind == Hierarchy::Types {
                        rows.push((0, root.clone()));
                        let mut frontier = vec![(root.clone(), 0)];
                        while let Some((item, d)) = frontier.pop() {
                            if d >= 4 || rows.len() > 400 {
                                continue;
                            }
                            let sub = client.request("typeHierarchy/subtypes", json!({ "item": item })).response().await.ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
                            for s in sub {
                                rows.push((d + 1, s.clone()));
                                frontier.push((s, d + 1));
                            }
                        }
                    }
                }
                Hierarchy::Calls => {
                    rows.push((0, root.clone()));
                    let mut frontier = vec![(root.clone(), 0)];
                    while let Some((item, d)) = frontier.pop() {
                        if d >= 3 || rows.len() > 400 {
                            continue;
                        }
                        let calls = client.request("callHierarchy/incomingCalls", json!({ "item": item })).response().await.ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
                        for c in calls {
                            let from = c["from"].clone();
                            rows.push((d + 1, from.clone()));
                            frontier.push((from, d + 1));
                        }
                    }
                }
            }
            let _ = this.update_in(cx, |ws, window, cx| {
                if kind == Hierarchy::Super {
                    let locs: Vec<Value> = rows.iter().map(|(_, i)| json!({ "uri": i["uri"], "range": i["selectionRange"] })).collect();
                    match locs.len() {
                        0 => ws.message = Some(format!("{name} không có lớp cha / interface nào (ngoài Object)")),
                        1 => {
                            ws.push_nav(cx);
                            ws.open_location(locs[0].clone(), window, cx)
                        }
                        _ => ws.show_locations("Super", &locs, window, cx),
                    }
                    return cx.notify();
                }
                let mut hits = Vec::new();
                for (d, item) in &rows {
                    let Some(path) = item["uri"].as_str().and_then(uri_to_path) else { continue };
                    let line = item["selectionRange"]["start"]["line"].as_u64().unwrap_or(0) as usize;
                    let arrow = match d.cmp(&0) {
                        std::cmp::Ordering::Less => "↑ ",
                        std::cmp::Ordering::Equal => "● ",
                        std::cmp::Ordering::Greater => if kind == Hierarchy::Calls { "← " } else { "↓ " },
                    };
                    let indent = "    ".repeat(d.unsigned_abs() as usize);
                    let detail = item["detail"].as_str().unwrap_or("");
                    let text = format!("{indent}{arrow}{}  {detail}", item["name"].as_str().unwrap_or(""));
                    hits.push(Hit { path, line, col: 0, text, mark: None });
                }
                // supertypes: farthest ancestor first
                if kind == Hierarchy::Types {
                    let (mut sup, rest): (Vec<Hit>, Vec<Hit>) = hits.into_iter().partition(|h| h.text.trim_start().starts_with('↑'));
                    sup.reverse();
                    hits = sup.into_iter().chain(rest).collect();
                }
                let title = match kind {
                    Hierarchy::Calls => format!("Call Hierarchy: ai gọi {name}"),
                    _ => format!("Type Hierarchy: {name}"),
                };
                ws.open_list(format!("{title} ({})", hits.len()), hits, window, cx);
            });
        })
        .detach();
    }

    /// Ctrl+E: files shown lately, the previous one first (Enter swaps back like IntelliJ).
    pub(crate) fn show_recent_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.active.and_then(|i| self.tabs.get(i)).and_then(|t| t.read(cx).path().cloned());
        let root = self.root.clone();
        let files: Vec<String> = self
            .recent_files
            .iter()
            .filter(|p| Some(*p) != current.as_ref() && p.exists())
            .map(|p| p.strip_prefix(&root).map(|r| r.display().to_string()).unwrap_or_else(|_| p.display().to_string()))
            .collect();
        if files.is_empty() {
            self.message = Some("Chưa có file nào khác được mở gần đây".into());
            return cx.notify();
        }
        let picker = cx.new(|cx| Picker::with_files("Recent Files".into(), root, files, window, cx));
        self.show_picker(picker, window, cx);
    }

    pub(crate) fn open_list(&mut self, title: String, hits: Vec<Hit>, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let picker = cx.new(|cx| Picker::with_hits(title, root, hits, window, cx));
        self.show_picker(picker, window, cx);
    }

    /// jdtls is importing / building: runs wait until it is done (IntelliJ waits for indexing).
    pub(crate) fn jdtls_busy(&self) -> bool {
        if self.lsp.is_none() {
            return false;
        }
        // a job without news for 2 minutes is considered finished (never block forever)
        let live = self.lsp_jobs.values().any(|j| j.blocks_run() && j.at.elapsed() < std::time::Duration::from_secs(120));
        matches!(self.lsp_state, LspState::Starting(_)) || live
    }

    /// Any jdtls job running (shown in the status bar, even ones that don't block runs).
    fn jdtls_working(&self) -> bool {
        self.lsp.is_some() && (matches!(self.lsp_state, LspState::Starting(_)) || self.lsp_jobs.values().any(|j| j.at.elapsed() < std::time::Duration::from_secs(120)))
    }

    /// Queues a run while jdtls is busy; returns true when it was queued.
    pub(crate) fn defer_run(&mut self, run: PendingRun, cx: &mut Context<Self>) -> bool {
        if !self.jdtls_busy() {
            return false;
        }
        self.message = Some(format!("{} - đợi jdtls xong ({}) rồi chạy…", run.label(), self.busy_text()));
        let first = self.pending_run.is_none();
        self.pending_run = Some(run);
        if first {
            // re-check now and then: a job that never reports "complete" must not block forever
            cx.spawn(async move |this, cx| loop {
                cx.background_executor().timer(std::time::Duration::from_secs(2)).await;
                let waiting = this.update(cx, |ws, cx| {
                    cx.notify();
                    ws.pending_run.is_some()
                });
                if !waiting.unwrap_or(false) {
                    break;
                }
            })
            .detach();
        }
        cx.notify();
        true
    }

    pub(crate) fn start_pending_run(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(run) = self.pending_run.take() else { return };
        self.message = None;
        match run {
            PendingRun::Launch { cfg, debug } => self.launch(cfg, debug, window, cx),
            PendingRun::Tests { module, targets, debug } => self.run_tests(&module, targets, debug, cx),
        }
    }

    fn busy_text(&self) -> String {
        match self.lsp_jobs.values().max_by_key(|j| j.at) {
            Some(j) if !j.status.is_empty() => format!("{}: {}", j.task, j.status),
            Some(j) => j.task.clone(),
            None => match &self.lsp_state {
                LspState::Starting(m) if !m.is_empty() => m.clone(),
                _ => "đang khởi động".into(),
            },
        }
    }

    /// Status bar: what jdtls is doing + a progress bar (+ the queued run, if any).
    pub(crate) fn render_progress(&self) -> Option<gpui::AnyElement> {
        use gpui::{div, prelude::*, px};
        use crate::theme::{self, c};
        if !self.jdtls_working() {
            return None;
        }
        let fraction = self.lsp_jobs.values().max_by_key(|j| j.at).and_then(|j| j.fraction);
        let text: String = self.busy_text().chars().take(70).collect();
        let bar_w = 120.;
        Some(
            div()
                .flex()
                .items_center()
                .gap_2()
                .when_some(self.pending_run.as_ref(), |d, r| d.child(div().text_color(c(theme::WARNING)).child(format!("⏳ {}", r.label()))))
                .child(div().text_color(c(theme::FG)).child(format!("⟳ {text}")))
                .child(
                    div()
                        .w(px(bar_w))
                        .h(px(5.))
                        .rounded_full()
                        .bg(c(theme::BORDER))
                        .child(
                            div()
                                .h_full()
                                .rounded_full()
                                .bg(c(theme::ACCENT))
                                // unknown total: a short segment, so it still reads as "working"
                                .w(px(bar_w * fraction.unwrap_or(0.35).max(0.04))),
                        ),
                )
                .when_some(fraction, |d, f| d.child(format!("{}%", (f * 100.).round())))
                .into_any_element(),
        )
    }

    pub(crate) fn lsp_status(&self) -> String {
        let (errors, warnings) = self.diagnostics.values().flatten().fold((0, 0), |(e, w), d| match d["severity"].as_u64() {
            Some(1) => (e + 1, w),
            Some(2) => (e, w + 1),
            _ => (e, w),
        });
        match &self.lsp_state {
            LspState::Off => String::new(),
            LspState::Starting(m) => format!("jdtls: {}", if m.is_empty() { "đang chạy…" } else { m }),
            LspState::Ready => format!("jdtls ✓  ✖ {errors}  ⚠ {warnings}"),
            LspState::Failed(e) => format!("jdtls lỗi: {}", e.lines().next().unwrap_or("")),
        }
    }

    // ---- keeping jdtls in step with the disk ----------------------------------------
    // Everything the IDE changes behind jdtls' back goes through these.

    /// Files created / changed / deleted outside an open editor (New, Delete, Rename, saves
    /// of non-Java files, refactorings written to disk) - workspace/didChangeWatchedFiles.
    pub(crate) fn notify_files(&self, changes: &[(PathBuf, FileChange)]) {
        let Some(client) = self.lsp.as_ref() else { return };
        if changes.is_empty() {
            return;
        }
        let events: Vec<Value> = changes.iter().map(|(p, k)| json!({ "uri": path_to_uri(p), "type": *k as u8 })).collect();
        client.notify("workspace/didChangeWatchedFiles", json!({ "changes": events }));
        // a pom touched this way changes the classpath too
        let poms: Vec<String> = changes
            .iter()
            .filter(|(p, _)| p.file_name().is_some_and(|n| n == "pom.xml"))
            .filter_map(|(p, _)| p.parent().map(|d| d.display().to_string()))
            .collect();
        if !poms.is_empty() {
            self.notify_modules_changed(Some(&poms));
        }
    }

    /// Modules whose dependencies changed (a jar deleted, pom saved, Maven install) -
    /// java/projectConfigurationsUpdate for their pom.xml; `None` = every module.
    pub(crate) fn notify_modules_changed(&self, modules: Option<&[String]>) {
        let (Some(client), Some(p)) = (self.lsp.as_ref(), self.project.as_ref()) else { return };
        let ids: Vec<Value> = p
            .modules
            .iter()
            .filter(|m| modules.is_none_or(|ms| ms.iter().any(|x| *x == m.path)))
            .map(|m| std::path::Path::new(&m.path).join("pom.xml"))
            .filter(|pom| pom.is_file())
            .map(|pom| json!({ "uri": path_to_uri(&pom) }))
            .collect();
        if !ids.is_empty() {
            client.notify("java/projectConfigurationsUpdate", json!({ "identifiers": ids }));
        }
    }

    /// Asks jdtls for an incremental build (after `mvn clean` wiped target/, etc.).
    pub(crate) fn request_jdtls_build(&self) {
        if let Some(c) = self.lsp.as_ref() {
            c.request("java/buildWorkspace", json!(false));
        }
    }

    /// Modules added/removed: jdtls only reads the module list at startup, so restart it.
    pub(crate) fn restart_lsp(&mut self, cx: &mut Context<Self>) {
        if self.lsp.is_none() && !matches!(self.lsp_state, LspState::Failed(_)) {
            return;
        }
        for t in self.tabs.clone() {
            t.update(cx, |v, _| v.detach_lsp());
        }
        let stop = self.shutdown_lsp();
        cx.background_executor().spawn(stop).detach();
        self.lsp_state = LspState::Off;
        self.diagnostics.clear();
        self.restart_lsp_after_reload = true;
        self.message = Some("Danh sách module đổi - khởi động lại jdtls…".into());
        cx.notify();
    }

    pub(crate) fn shutdown_lsp(&mut self) -> impl std::future::Future<Output = ()> + 'static + use<> {
        let client = self.lsp.take();
        async move {
            if let Some(c) = client {
                // blocking shutdown on a helper thread so quitting stays responsive
                let _ = std::thread::spawn(move || c.shutdown_blocking(std::time::Duration::from_secs(3))).join();
            }
        }
    }
}

/// "jdt://contents/spring-core-5.3.jar/org.springframework.util/StringUtils.class?=…" → "StringUtils.class"
fn class_title(uri: &str) -> String {
    let path = uri.split('?').next().unwrap_or(uri);
    let decoded = path.replace("%24", "$");
    decoded.rsplit('/').next().unwrap_or("class").to_string()
}

#[cfg(test)]
mod tests {
    #[test]
    fn class_title_from_jdt_uri() {
        assert_eq!(super::class_title("jdt://contents/spring-core-5.3.jar/org.springframework.util/StringUtils.class?=abc"), "StringUtils.class");
    }
}

#[cfg(test)]
mod progress_tests {
    use super::Job;

    fn job(task: &str) -> Job {
        Job { task: task.into(), status: String::new(), fraction: None, at: std::time::Instant::now() }
    }

    /// Task names as jdtls 1.54 sends them (`chodilamide lsp progress` on sample-project).
    #[test]
    fn only_import_and_build_block_runs() {
        for t in ["Building", "Importing Maven project(s)", "Update Maven project configuration", "Synchronizing projects"] {
            assert!(job(t).blocks_run(), "{t}");
        }
        for t in ["Searching...", "Reporting encoding changes.", "Validate documents", "Publish Diagnostics"] {
            assert!(!job(t).blocks_run(), "{t}");
        }
    }
}
