//! Language-server features of one editor: document sync, diagnostics, definition,
//! hover, completion, references, format, organize imports.
use crate::editor_view::{EditorEvent, EditorView};
use crate::theme::{self, c};
use gpui::{
    AnyElement, Context, Div, Focusable, KeyBinding, MouseButton, SharedString, Stateful, actions, anchored, deferred, div,
    prelude::*, px,
};
use lsp_client::Client;
use serde_json::{Value, json};
use std::ops::Range;
use std::sync::Arc;

actions!(
    lsp,
    [GotoDefinition, Hover, ShowCompletion, FindUsages, NextError, PrevError, FormatCode, OrganizeImports,
     CompletionNext, CompletionPrev, CompletionAccept, CompletionCancel, ShowCodeActions, MenuNext, MenuPrev,
     MenuAccept, MenuCancel, Rename, ConfirmRename, CancelRename, FileStructure, ToggleBreakpoint, EditBreakpoint,
     ParameterInfo, CloseParameterInfo, ExtractVariable, ExtractConstant, ExtractField, ExtractMethod, IntroduceParameter,
     Generate, GotoImplementation, TypeHierarchy, CallHierarchy, GotoSuper]
);

pub fn bind_keys(cx: &mut gpui::App) {
    let ed = Some("Editor");
    let comp = Some("Editor && completing");
    cx.bind_keys([
        KeyBinding::new("ctrl-b", GotoDefinition, ed),
        KeyBinding::new("f12", GotoDefinition, ed),
        KeyBinding::new("ctrl-q", Hover, ed),
        KeyBinding::new("ctrl-space", ShowCompletion, ed),
        // fcitx5 usually takes Ctrl+Space to switch input methods: alternatives that always work
        KeyBinding::new("ctrl-shift-space", ShowCompletion, ed),
        KeyBinding::new("alt-/", ShowCompletion, ed),
        KeyBinding::new("ctrl-p", ParameterInfo, ed),
        KeyBinding::new("escape", CloseParameterInfo, Some("Editor && sighelp")),
        KeyBinding::new("alt-f7", FindUsages, ed),
        KeyBinding::new("f2", NextError, ed),
        KeyBinding::new("shift-f2", PrevError, ed),
        KeyBinding::new("ctrl-alt-l", FormatCode, ed),
        KeyBinding::new("ctrl-alt-o", OrganizeImports, ed),
        // IntelliJ refactor → extract keys
        KeyBinding::new("ctrl-alt-v", ExtractVariable, ed),
        KeyBinding::new("ctrl-alt-c", ExtractConstant, ed),
        KeyBinding::new("ctrl-alt-f", ExtractField, ed),
        KeyBinding::new("ctrl-alt-m", ExtractMethod, ed),
        KeyBinding::new("ctrl-alt-p", IntroduceParameter, ed),
        KeyBinding::new("alt-insert", Generate, ed),
        KeyBinding::new("ctrl-alt-b", GotoImplementation, ed),
        KeyBinding::new("ctrl-u", GotoSuper, ed),
        KeyBinding::new("ctrl-h", TypeHierarchy, ed),
        KeyBinding::new("ctrl-alt-h", CallHierarchy, ed),
        // bound after the editor keys so they win while the popup is open
        KeyBinding::new("down", CompletionNext, comp),
        KeyBinding::new("up", CompletionPrev, comp),
        KeyBinding::new("enter", CompletionAccept, comp),
        KeyBinding::new("tab", CompletionAccept, comp),
        KeyBinding::new("escape", CompletionCancel, comp),
        KeyBinding::new("alt-enter", ShowCodeActions, ed),
        KeyBinding::new("shift-f6", Rename, ed),
        KeyBinding::new("ctrl-f12", FileStructure, ed),
        KeyBinding::new("ctrl-f8", ToggleBreakpoint, ed),
        KeyBinding::new("ctrl-shift-f8", EditBreakpoint, ed),
        KeyBinding::new("down", MenuNext, Some("Editor && menu")),
        KeyBinding::new("up", MenuPrev, Some("Editor && menu")),
        KeyBinding::new("enter", MenuAccept, Some("Editor && menu")),
        KeyBinding::new("escape", MenuCancel, Some("Editor && menu")),
        KeyBinding::new("enter", ConfirmRename, Some("RenameBox")),
        KeyBinding::new("escape", CancelRename, Some("RenameBox")),
    ]);
}

pub fn register_lsp_actions(div: Stateful<Div>, cx: &mut Context<EditorView>) -> Stateful<Div> {
    div.on_action(cx.listener(|v, _: &GotoDefinition, _, cx| v.goto_definition(cx)))
        .on_action(cx.listener(|v, _: &Hover, _, cx| v.request_hover(cx)))
        .on_action(cx.listener(|v, _: &ShowCompletion, _, cx| v.trigger_completion(true, cx)))
        .on_action(cx.listener(|v, _: &FindUsages, _, cx| v.find_usages(cx)))
        .on_action(cx.listener(|v, _: &NextError, _, cx| v.jump_diagnostic(true, cx)))
        .on_action(cx.listener(|v, _: &PrevError, _, cx| v.jump_diagnostic(false, cx)))
        .on_action(cx.listener(|v, _: &FormatCode, _, cx| v.format(cx)))
        .on_action(cx.listener(|v, _: &OrganizeImports, _, cx| v.organize_imports(cx)))
        .on_action(cx.listener(|v, _: &CompletionNext, _, cx| v.move_completion(1, cx)))
        .on_action(cx.listener(|v, _: &CompletionPrev, _, cx| v.move_completion(-1, cx)))
        .on_action(cx.listener(|v, _: &CompletionAccept, _, cx| v.accept_completion(cx)))
        .on_action(cx.listener(|v, _: &CompletionCancel, _, cx| {
            v.completion = None;
            cx.notify()
        }))
        .on_action(cx.listener(|v, _: &ShowCodeActions, _, cx| v.request_code_actions(None, cx)))
        .on_action(cx.listener(|v, _: &ExtractVariable, _, cx| v.request_code_actions(Some("refactor.extract.variable"), cx)))
        .on_action(cx.listener(|v, _: &ExtractConstant, _, cx| v.request_code_actions(Some("refactor.extract.constant"), cx)))
        .on_action(cx.listener(|v, _: &ExtractField, _, cx| v.request_code_actions(Some("refactor.extract.field"), cx)))
        .on_action(cx.listener(|v, _: &ExtractMethod, _, cx| v.request_code_actions(Some("refactor.extract.function"), cx)))
        .on_action(cx.listener(|v, _: &IntroduceParameter, _, cx| v.request_code_actions(Some("refactor.introduce.parameter"), cx)))
        .on_action(cx.listener(|v, _: &ParameterInfo, _, cx| v.request_signature(true, cx)))
        .on_action(cx.listener(|v, _: &Generate, _, cx| v.request_code_actions(Some("source"), cx)))
        .on_action(cx.listener(|v, _: &GotoImplementation, _, cx| v.goto_implementation(cx)))
        .on_action(cx.listener(|v, _: &GotoSuper, _, cx| v.hierarchy(crate::editor_view::Hierarchy::Super, cx)))
        .on_action(cx.listener(|v, _: &TypeHierarchy, _, cx| v.hierarchy(crate::editor_view::Hierarchy::Types, cx)))
        .on_action(cx.listener(|v, _: &CallHierarchy, _, cx| v.hierarchy(crate::editor_view::Hierarchy::Calls, cx)))
        .on_action(cx.listener(|v, _: &CloseParameterInfo, _, cx| {
            v.signature = None;
            cx.notify()
        }))
        .on_action(cx.listener(|v, _: &MenuNext, _, cx| v.move_menu(1, cx)))
        .on_action(cx.listener(|v, _: &MenuPrev, _, cx| v.move_menu(-1, cx)))
        .on_action(cx.listener(|v, _: &MenuAccept, _, cx| v.accept_menu(cx)))
        .on_action(cx.listener(|v, _: &MenuCancel, _, cx| {
            v.actions_menu = None;
            cx.notify()
        }))
        .on_action(cx.listener(|v, _: &Rename, window, cx| v.start_rename(window, cx)))
        .on_action(cx.listener(|v, _: &ConfirmRename, window, cx| v.confirm_rename(window, cx)))
        .on_action(cx.listener(|v, _: &CancelRename, window, cx| v.cancel_rename(window, cx)))
        .on_action(cx.listener(|v, _: &FileStructure, _, cx| v.request_structure(cx)))
        .on_action(cx.listener(|v, _: &ToggleBreakpoint, _, cx| v.toggle_breakpoint_at_cursor(cx)))
        .on_action(cx.listener(|v, _: &EditBreakpoint, _, cx| v.edit_breakpoint_at_cursor(cx)))
}

/// What semantic highlighting changes on a token (the rest keeps its tree-sitter color).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sem {
    /// field / enum constant: IntelliJ's purple
    pub field: bool,
    pub annotation: bool,
    /// static members: italic
    pub is_static: bool,
    /// deprecated: struck through
    pub deprecated: bool,
}

/// LSP relative token data → byte ranges with their `Sem` (tokens that change nothing skipped).
pub fn decode_semantic(b: &editor_core::Buffer, data: &Value, legend: &Value) -> Vec<(Range<usize>, Sem)> {
    let types: Vec<&str> = legend["tokenTypes"].as_array().map(|a| a.iter().filter_map(|t| t.as_str()).collect()).unwrap_or_default();
    let mods: Vec<&str> = legend["tokenModifiers"].as_array().map(|a| a.iter().filter_map(|t| t.as_str()).collect()).unwrap_or_default();
    let bit = |name: &str| mods.iter().position(|m| *m == name).map(|i| 1u64 << i).unwrap_or(0);
    let (st, dep) = (bit("static"), bit("deprecated"));
    let Some(d) = data.as_array() else { return Vec::new() };
    let mut out = Vec::new();
    let (mut line, mut col) = (0usize, 0usize);
    for t in d.chunks(5) {
        let [dl, ds, len, ty, m] = [0, 1, 2, 3, 4].map(|i| t.get(i).and_then(|v| v.as_u64()).unwrap_or(0));
        if dl > 0 {
            line += dl as usize;
            col = ds as usize;
        } else {
            col += ds as usize;
        }
        let kind = types.get(ty as usize).copied().unwrap_or("");
        let sem = Sem {
            field: matches!(kind, "property" | "enumMember"),
            annotation: matches!(kind, "annotation" | "decorator"),
            is_static: m & st != 0 && matches!(kind, "property" | "enumMember" | "method"),
            deprecated: m & dep != 0,
        };
        if !(sem.field || sem.annotation || sem.is_static || sem.deprecated) {
            continue;
        }
        let s = b.byte_of_lsp_position(line, col);
        let e = b.byte_of_lsp_position(line, col + len as usize);
        if e > s {
            out.push((s..e, sem));
        }
    }
    out
}

/// Parameter info (Ctrl+P / typing "(" or ","): signatures of the call being typed.
pub struct SignatureHelp {
    signatures: Vec<Value>,
    active: usize,
    param: usize,
}

pub struct ActionsMenu {
    items: Vec<Value>,
    selected: usize,
    at: usize,
}

pub struct LspDoc {
    pub client: Arc<Client>,
    pub uri: String,
    version: i64,
    /// false for jdt:// class files: they are never didOpen'ed or changed
    synced: bool,
}

#[derive(Debug, Clone)]
pub struct Diag {
    pub range: Range<usize>,
    /// 1 error, 2 warning, 3 info, 4 hint
    pub severity: u8,
    pub message: String,
}

pub struct Completion {
    pub(crate) items: Vec<Value>,
    /// indices into `items` matching the typed prefix, best first
    pub visible: Vec<usize>,
    selected: usize,
    /// where the identifier being completed starts
    word_start: usize,
    request: Option<i64>,
}

fn is_ident(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_' || ch == '$'
}

impl EditorView {
    // ---- document sync -------------------------------------------------------

    pub fn attach_lsp(&mut self, client: Arc<Client>, uri: String, synced: bool) {
        if synced {
            client.notify(
                "textDocument/didOpen",
                json!({ "textDocument": { "uri": uri, "languageId": language_id(&uri), "version": 1, "text": self.editor.buffer.text() } }),
            );
            self.editor.buffer.set_track_changes(true);
        }
        self.lsp = Some(LspDoc { client, uri, version: 1, synced });
    }

    pub fn detach_lsp(&mut self) {
        if let Some(doc) = self.lsp.take() {
            if doc.synced {
                doc.client.notify("textDocument/didClose", json!({ "textDocument": { "uri": doc.uri } }));
            }
        }
        self.editor.buffer.set_track_changes(false);
        self.diagnostics.clear();
        self.completion = None;
    }

    pub fn lsp_uri(&self) -> Option<&str> {
        self.lsp.as_ref().map(|d| d.uri.as_str())
    }

    /// Sends buffered edits as one incremental didChange (called on every render).
    /// Asks jdtls for semantic tokens when the text changed since the last answer (one
    /// request in flight at a time; an answer for an older version is dropped).
    pub(crate) fn refresh_semantic(&mut self, cx: &mut Context<Self>) {
        let version = self.editor.buffer.version();
        if self.sem_inflight || self.sem_version == Some(version) {
            return;
        }
        let Some(doc) = self.lsp.as_ref().filter(|d| d.synced) else { return };
        let legend = doc.client.capabilities.lock().unwrap()["semanticTokensProvider"]["legend"].clone();
        if legend.is_null() {
            return;
        }
        self.lsp_sync();
        let doc = self.lsp.as_ref().unwrap();
        let req = doc.client.request("textDocument/semanticTokens/full", json!({ "textDocument": { "uri": doc.uri } }));
        self.sem_inflight = true;
        cx.spawn(async move |this, cx| {
            let res = req.response().await;
            let _ = this.update(cx, |v, cx| {
                v.sem_inflight = false;
                if v.editor.buffer.version() != version {
                    return; // typed meanwhile: the next render asks again
                }
                v.sem_version = Some(version);
                if let Ok(r) = res {
                    v.semantic = decode_semantic(&v.editor.buffer, &r["data"], &legend);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(crate) fn lsp_sync(&mut self) {
        let Some(doc) = self.lsp.as_mut().filter(|d| d.synced) else { return };
        let changes = self.editor.buffer.take_changes();
        if changes.is_empty() {
            return;
        }
        doc.version += 1;
        let content: Vec<Value> = changes
            .iter()
            .map(|ch| {
                json!({
                    "range": { "start": { "line": ch.start.0, "character": ch.start.1 }, "end": { "line": ch.end.0, "character": ch.end.1 } },
                    "text": ch.text
                })
            })
            .collect();
        doc.client.notify("textDocument/didChange", json!({ "textDocument": { "uri": doc.uri, "version": doc.version }, "contentChanges": content }));
    }

    pub fn lsp_did_save(&mut self) {
        self.lsp_sync();
        if let Some(doc) = self.lsp.as_ref().filter(|d| d.synced) {
            doc.client.notify("textDocument/didSave", json!({ "textDocument": { "uri": doc.uri } }));
        }
    }

    fn position_params(&self, at: usize) -> Option<(Arc<Client>, Value)> {
        let doc = self.lsp.as_ref()?;
        let (line, character) = self.editor.buffer.lsp_position(at);
        Some((doc.client.clone(), json!({ "textDocument": { "uri": doc.uri }, "position": { "line": line, "character": character } })))
    }

    fn no_lsp(&mut self, cx: &mut Context<Self>) {
        self.message = Some("jdtls chưa sẵn sàng cho file này".into());
        cx.notify();
    }

    // ---- diagnostics ---------------------------------------------------------

    pub fn set_diagnostics(&mut self, diags: &[Value]) {
        let b = &self.editor.buffer;
        let pos = |p: &Value| b.byte_of_lsp_position(p["line"].as_u64().unwrap_or(0) as usize, p["character"].as_u64().unwrap_or(0) as usize);
        self.diagnostics = diags
            .iter()
            .map(|d| {
                let mut range = pos(&d["range"]["start"])..pos(&d["range"]["end"]);
                if range.is_empty() {
                    // zero-width (e.g. "missing ;"): underline the next character
                    range.end = b.next_grapheme(range.start).max(range.start);
                }
                Diag { range, severity: d["severity"].as_u64().unwrap_or(1) as u8, message: d["message"].as_str().unwrap_or("").to_string() }
            })
            .collect();
        self.diagnostics.sort_by_key(|d| (d.range.start, d.severity));
    }

    /// Diagnostics computed by the IDE itself (JSF inspections), as byte ranges.
    pub fn set_byte_diagnostics(&mut self, diags: Vec<Diag>) {
        let len = self.editor.buffer.len();
        self.diagnostics = diags.into_iter().filter(|d| d.range.end <= len).collect();
        self.diagnostics.sort_by_key(|d| (d.range.start, d.severity));
    }

    /// Message of the most severe diagnostic under the cursor (status bar).
    pub fn diagnostic_at_cursor(&self) -> Option<&Diag> {
        let h = self.editor.head;
        self.diagnostics.iter().filter(|d| d.range.start <= h && h <= d.range.end).min_by_key(|d| d.severity)
    }

    fn jump_diagnostic(&mut self, forward: bool, cx: &mut Context<Self>) {
        // errors first; warnings only when there are no errors (IntelliJ's F2)
        let max_sev = if self.diagnostics.iter().any(|d| d.severity == 1) { 1 } else { 2 };
        let cands: Vec<usize> = self.diagnostics.iter().filter(|d| d.severity <= max_sev).map(|d| d.range.start).collect();
        let h = self.editor.head;
        let target = if forward {
            cands.iter().copied().find(|&s| s > h).or(cands.first().copied())
        } else {
            cands.iter().rev().copied().find(|&s| s < h).or(cands.last().copied())
        };
        match target {
            Some(t) => {
                self.editor.set_cursor(t, false);
                self.request_center();
            }
            None => self.message = Some("Không có lỗi/cảnh báo".into()),
        }
        cx.notify();
    }

    // ---- navigation ----------------------------------------------------------

    pub(crate) fn is_xhtml(&self) -> bool {
        self.path().is_some_and(|p| p.extension().is_some_and(|e| e == "xhtml"))
    }

    /// application*.properties / application*.yml (Spring Boot config)
    pub(crate) fn is_spring_config(&self) -> bool {
        self.path().is_some_and(|p| spring::config::kind_of(p).is_some())
    }

    /// Cursor inside `${…}` of a Java string (`@Value("${key}")`).
    fn in_spring_placeholder(&self) -> bool {
        if !self.path().is_some_and(|p| p.extension().is_some_and(|e| e == "java")) {
            return false;
        }
        let b = &self.editor.buffer;
        let start = b.line_start(b.line_of(self.editor.head));
        let line = b.slice(start..self.editor.head);
        spring::config::placeholder_at(&line, line.len()).is_some()
    }

    fn spring_here(&self) -> bool {
        self.is_spring_config() || self.in_spring_placeholder()
    }

    pub(crate) fn goto_definition(&mut self, cx: &mut Context<Self>) {
        if self.is_xhtml() {
            return cx.emit(EditorEvent::JsfGoto(self.editor.head));
        }
        if self.spring_here() {
            return cx.emit(EditorEvent::SpringGoto(self.editor.head));
        }
        let Some((client, params)) = self.position_params(self.editor.head) else { return self.no_lsp(cx) };
        let (uri, pos) = (params["textDocument"]["uri"].clone(), params["position"].clone());
        let req = client.request("textDocument/definition", params);
        cx.spawn(async move |this, cx| {
            let res = req.response().await;
            let _ = this.update(cx, |v, cx| {
                let locs = match res {
                    Ok(Value::Array(locs)) => Ok(locs),
                    Ok(loc @ Value::Object(_)) => Ok(vec![loc]),
                    Ok(_) => Ok(Vec::new()),
                    Err(e) => Err(e),
                };
                match locs {
                    // already on the declaration: show its usages instead (IntelliJ's Ctrl+B)
                    Ok(locs) if locs.len() == 1 && is_at(&locs[0], &uri, &pos) => v.find_usages(cx),
                    Ok(locs) if !locs.is_empty() => cx.emit(EditorEvent::Goto(locs)),
                    Ok(_) => v.message = Some("Không tìm thấy định nghĩa".into()),
                    Err(e) => v.message = Some(format!("{e}")),
                }
                cx.notify();
            });
        })
        .detach();
    }


    /// Ctrl+Alt+B: implementations of the interface/abstract method at the cursor.
    fn goto_implementation(&mut self, cx: &mut Context<Self>) {
        let Some((client, params)) = self.position_params(self.editor.head) else { return self.no_lsp(cx) };
        self.lsp_sync();
        let req = client.request("textDocument/implementation", params);
        cx.spawn(async move |this, cx| {
            let res = req.response().await;
            let _ = this.update(cx, |v, cx| {
                match res {
                    Ok(Value::Array(locs)) if locs.len() == 1 => cx.emit(EditorEvent::Goto(locs)),
                    Ok(Value::Array(locs)) if !locs.is_empty() => cx.emit(EditorEvent::Locations { title: "Implementations".into(), locs }),
                    Ok(loc @ Value::Object(_)) => cx.emit(EditorEvent::Goto(vec![loc])),
                    Ok(_) => v.message = Some("Không có implementation nào".into()),
                    Err(e) => v.message = Some(format!("{e}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Ctrl+H / Ctrl+Alt+H / Ctrl+U: the workspace walks the hierarchy (several requests).
    fn hierarchy(&mut self, kind: crate::editor_view::Hierarchy, cx: &mut Context<Self>) {
        let Some((_, params)) = self.position_params(self.editor.head) else { return self.no_lsp(cx) };
        self.lsp_sync();
        cx.emit(EditorEvent::Hierarchy { kind, params });
    }

    fn find_usages(&mut self, cx: &mut Context<Self>) {
        let Some((client, mut params)) = self.position_params(self.editor.head) else { return self.no_lsp(cx) };
        params["context"] = json!({ "includeDeclaration": false });
        let uri = params["textDocument"]["uri"].as_str().unwrap_or("").to_string();
        let (line, col) = (params["position"]["line"].as_u64().unwrap_or(0) as usize, params["position"]["character"].as_u64().unwrap_or(0) as usize);
        self.message = Some("Đang tìm chỗ dùng…".into());
        let req = client.request("textDocument/references", params);
        cx.spawn(async move |this, cx| {
            let res = req.response().await;
            let _ = this.update(cx, |v, cx| {
                v.message = None;
                match res {
                    // the workspace adds .xhtml EL usages and reports "none" when both are empty
                    Ok(Value::Array(locs)) => cx.emit(EditorEvent::Usages { locs, uri, line, col }),
                    Ok(_) => cx.emit(EditorEvent::Usages { locs: Vec::new(), uri, line, col }),
                    Err(e) => v.message = Some(format!("{e}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn request_hover(&mut self, cx: &mut Context<Self>) {
        if self.is_xhtml() {
            return cx.emit(EditorEvent::JsfHover(self.editor.head));
        }
        if self.spring_here() {
            return cx.emit(EditorEvent::SpringHover(self.editor.head));
        }
        let Some((client, params)) = self.position_params(self.editor.head) else { return self.no_lsp(cx) };
        let req = client.request("textDocument/hover", params);
        cx.spawn(async move |this, cx| {
            let res = req.response().await;
            let _ = this.update(cx, |v, cx| {
                let text = res.ok().map(|h| hover_text(&h["contents"])).unwrap_or_default();
                v.hover = (!text.trim().is_empty()).then(|| text.trim().to_string());
                if v.hover.is_none() {
                    v.message = Some("Không có tài liệu".into());
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ---- edits from the server -------------------------------------------------

    /// Applies LSP TextEdits (ranges refer to the current text), keeping the cursor in place.
    pub fn apply_text_edits(&mut self, edits: &[Value]) {
        let b = &self.editor.buffer;
        let pos = |p: &Value| b.byte_of_lsp_position(p["line"].as_u64().unwrap_or(0) as usize, p["character"].as_u64().unwrap_or(0) as usize);
        let mut resolved: Vec<(Range<usize>, String)> = edits
            .iter()
            .map(|e| (pos(&e["range"]["start"])..pos(&e["range"]["end"]), e["newText"].as_str().unwrap_or("").to_string()))
            .collect();
        // apply bottom-up so earlier ranges stay valid
        resolved.sort_by(|a, b| b.0.start.cmp(&a.0.start));
        self.editor.buffer.break_group();
        let mut head = self.editor.head;
        for (r, text) in resolved {
            let sel = self.editor.anchor..self.editor.head;
            self.editor.buffer.edit(r.clone(), &text, sel.clone(), sel);
            if r.end <= head {
                head = head + text.len() - r.len();
            } else if r.start < head {
                head = r.start + text.len();
            }
        }
        self.editor.buffer.break_group();
        let head = head.min(self.editor.buffer.len());
        self.editor.anchor = head;
        self.editor.head = head;
    }

    /// Ctrl+S: a Java file synced with jdtls is formatted first, then written; without jdtls
    /// (or when it does not answer in 2 s) it is written as is.
    pub(crate) fn save_formatted(&mut self, cx: &mut Context<Self>) {
        let java = self.editor.buffer.path.as_ref().is_some_and(|p| p.extension().is_some_and(|e| e == "java"));
        if !java || self.read_only || self.lsp.as_ref().is_none_or(|d| !d.synced) {
            self.save();
            self.lsp_did_save();
            return;
        }
        self.lsp_sync();
        let doc = self.lsp.as_ref().unwrap();
        let options = json!({ "tabSize": self.tab_width, "insertSpaces": self.editor.indent != "\t" });
        let req = doc.client.request("textDocument/formatting", json!({ "textDocument": { "uri": doc.uri }, "options": options }));
        let version = self.editor.buffer.version();
        cx.spawn(async move |this, cx| {
            let timer = cx.background_executor().timer(std::time::Duration::from_secs(2));
            let res = match futures::future::select(Box::pin(req.response()), Box::pin(timer)).await {
                futures::future::Either::Left((r, _)) => r.ok(),
                futures::future::Either::Right(_) => None,
            };
            let _ = this.update(cx, |v, cx| {
                // typed meanwhile: the edits no longer fit
                if let (Some(Value::Array(edits)), true) = (res, v.editor.buffer.version() == version) {
                    if !edits.is_empty() {
                        v.apply_text_edits(&edits);
                    }
                }
                v.save();
                v.lsp_did_save();
                cx.notify();
            });
        })
        .detach();
    }

    fn format(&mut self, cx: &mut Context<Self>) {
        let Some(_) = self.lsp.as_ref().filter(|d| d.synced) else { return self.no_lsp(cx) };
        self.lsp_sync();
        let doc = self.lsp.as_ref().unwrap();
        let tab = self.editor.indent == "\t";
        let options = json!({ "tabSize": self.tab_width, "insertSpaces": !tab });
        // a selection formats just those lines (IntelliJ), otherwise the whole file
        let req = if self.editor.has_selection() {
            let sel = self.editor.selection();
            let (s, e) = (self.editor.buffer.lsp_position(sel.start), self.editor.buffer.lsp_position(sel.end));
            doc.client.request(
                "textDocument/rangeFormatting",
                json!({ "textDocument": { "uri": doc.uri }, "range": { "start": { "line": s.0, "character": 0 }, "end": { "line": e.0, "character": e.1 } }, "options": options }),
            )
        } else {
            doc.client.request("textDocument/formatting", json!({ "textDocument": { "uri": doc.uri }, "options": options }))
        };
        cx.spawn(async move |this, cx| {
            let res = req.response().await;
            let _ = this.update(cx, |v, cx| {
                match res {
                    Ok(Value::Array(edits)) if edits.is_empty() => v.message = Some("Code đã đúng format".into()),
                    Ok(Value::Array(edits)) => {
                        v.apply_text_edits(&edits);
                        v.message = Some("Đã format".into());
                    }
                    Ok(_) => {}
                    Err(e) => v.message = Some(format!("{e}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn organize_imports(&mut self, cx: &mut Context<Self>) {
        let Some(_) = self.lsp.as_ref().filter(|d| d.synced) else { return self.no_lsp(cx) };
        self.lsp_sync();
        let doc = self.lsp.as_ref().unwrap();
        let end = self.editor.buffer.lsp_position(self.editor.buffer.len());
        let uri = doc.uri.clone();
        let req = doc.client.request(
            "textDocument/codeAction",
            json!({
                "textDocument": { "uri": uri },
                "range": { "start": { "line": 0, "character": 0 }, "end": { "line": end.0, "character": end.1 } },
                "context": { "diagnostics": [], "only": ["source.organizeImports"] }
            }),
        );
        cx.spawn(async move |this, cx| {
            let res = req.response().await;
            let _ = this.update(cx, |v, cx| {
                let action = res.ok().and_then(|r| r.as_array().and_then(|a| a.first().cloned()));
                match action.as_ref().map(|a| &a["edit"]) {
                    Some(edit) if !edit.is_null() => {
                        let edits = edits_for_uri(edit, &uri);
                        v.apply_text_edits(&edits);
                        v.message = Some("Đã sắp xếp import".into());
                    }
                    _ => v.message = Some("Không có import cần sắp xếp".into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ---- completion ------------------------------------------------------------

    /// Called after text was typed: '.' opens completion, identifier chars refilter it
    /// (or open it after 2 chars), anything else closes it.
    pub(crate) fn after_typed(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.is_xhtml() && self.editor.marked.is_none() {
            return self.jsf_after_typed(text, cx);
        }
        if self.editor.marked.is_none() && self.spring_here() {
            return self.spring_after_typed(text, cx);
        }
        if self.lsp.is_none() || self.editor.marked.is_some() {
            return;
        }
        let Some(last) = text.chars().last() else { return };
        // parameter info follows the call being typed
        match last {
            '(' | ',' => self.request_signature(false, cx),
            ')' | ';' => self.signature = None,
            _ => {}
        }
        if last == '.' {
            self.completion = None;
            self.trigger_completion(false, cx);
        } else if is_ident(last) {
            if self.completion.is_some() {
                self.refilter_completion(cx);
            } else if self.word_before_cursor().chars().count() >= 2 {
                self.trigger_completion(false, cx);
            }
        } else {
            self.completion = None;
        }
    }

    fn word_before_cursor(&self) -> String {
        let b = &self.editor.buffer;
        let line = b.line_of(self.editor.head);
        let start = b.line_start(line);
        let before = b.slice(start..self.editor.head);
        before.chars().rev().take_while(|c| is_ident(*c)).collect::<Vec<_>>().into_iter().rev().collect()
    }

    fn trigger_completion(&mut self, explicit: bool, cx: &mut Context<Self>) {
        if self.is_xhtml() {
            return self.jsf_complete(explicit, cx);
        }
        if self.spring_here() {
            return self.spring_complete(explicit, cx);
        }
        let Some(_) = self.lsp.as_ref() else {
            if explicit {
                self.no_lsp(cx);
            }
            return;
        };
        self.lsp_sync();
        if let Some(old) = self.completion.as_ref().and_then(|c| c.request) {
            self.lsp.as_ref().unwrap().client.cancel(old);
        }
        let word_start = self.editor.head - self.word_before_cursor().len();
        let Some((client, mut params)) = self.position_params(self.editor.head) else { return };
        params["context"] = if explicit { json!({ "triggerKind": 1 }) } else { json!({ "triggerKind": 1 }) };
        let req = client.request("textDocument/completion", params);
        let id = req.id;
        self.completion = Some(Completion { items: Vec::new(), visible: Vec::new(), selected: 0, word_start, request: Some(id) });
        cx.spawn(async move |this, cx| {
            let res = req.response().await;
            let _ = this.update(cx, |v, cx| {
                // stale (cancelled/replaced) or popup closed meanwhile
                let Some(comp) = v.completion.as_mut().filter(|c| c.request == Some(id)) else { return };
                let items = match res {
                    Ok(Value::Array(a)) => a,
                    Ok(obj @ Value::Object(_)) => obj["items"].as_array().cloned().unwrap_or_default(),
                    _ => Vec::new(),
                };
                comp.items = items;
                comp.request = None;
                v.refilter_completion(cx);
                if v.completion.as_ref().is_some_and(|c| c.visible.is_empty()) && explicit {
                    v.message = Some("Không có gợi ý".into());
                }
            });
        })
        .detach();
    }

    // ---- JSF (.xhtml) -------------------------------------------------------------
    // The workspace owns the bean index and jdtls, so the editor only opens the popup and asks.

    fn jsf_after_typed(&mut self, text: &str, cx: &mut Context<Self>) {
        let Some(last) = text.chars().last() else { return };
        if is_ident(last) {
            if self.completion.is_some() {
                self.refilter_completion(cx);
            } else {
                self.jsf_complete(false, cx);
            }
        } else if matches!(last, '.' | '{' | '[' | '\'' | '"' | '/' | ':') {
            self.completion = None;
            self.jsf_complete(false, cx);
        } else {
            self.completion = None;
        }
    }

    /// Spring keys contain '.' and '-': they keep refiltering the open popup instead of closing it.
    fn spring_after_typed(&mut self, text: &str, cx: &mut Context<Self>) {
        let Some(last) = text.chars().last() else { return };
        if is_ident(last) || matches!(last, '.' | '-' | '[' | ']') {
            if self.completion.is_some() {
                self.refilter_completion(cx);
            } else {
                self.spring_complete(false, cx);
            }
        } else if matches!(last, '=' | '{') || (last == ' ' && text.len() == 1 && self.char_before_is(':')) {
            self.completion = None;
            self.spring_complete(false, cx);
        } else {
            self.completion = None;
        }
    }

    fn char_before_is(&self, c: char) -> bool {
        let h = self.editor.head;
        h >= 2 && self.editor.buffer.slice(h - 2..h - 1) == c.to_string()
    }

    fn spring_complete(&mut self, explicit: bool, cx: &mut Context<Self>) {
        let word_start = self.editor.head - self.word_before_cursor().len();
        self.jsf_request += 1;
        self.jsf_at = self.editor.head;
        self.completion = Some(Completion { items: Vec::new(), visible: Vec::new(), selected: 0, word_start, request: Some(-self.jsf_request) });
        cx.emit(EditorEvent::SpringComplete { at: self.editor.head, explicit });
    }

    fn jsf_complete(&mut self, explicit: bool, cx: &mut Context<Self>) {
        let word_start = self.editor.head - self.word_before_cursor().len();
        self.jsf_request += 1;
        self.jsf_at = self.editor.head;
        self.completion = Some(Completion { items: Vec::new(), visible: Vec::new(), selected: 0, word_start, request: Some(-self.jsf_request) });
        cx.emit(EditorEvent::JsfComplete { at: self.editor.head, explicit });
    }

    /// Items for the popup opened by `jsf_complete` (dropped when the popup moved on).
    pub(crate) fn set_jsf_completion(&mut self, at: usize, word_start: usize, items: Vec<Value>, explicit: bool, cx: &mut Context<Self>) {
        // an answer to an older request (typing went on: "#{" then "bean."): not ours
        if at != self.jsf_at {
            return;
        }
        let Some(comp) = self.completion.as_mut().filter(|c| c.request == Some(-self.jsf_request)) else { return };
        if self.editor.head < at {
            self.completion = None;
            return;
        }
        comp.items = items;
        comp.word_start = word_start;
        comp.request = None;
        self.refilter_completion(cx);
        if self.completion.is_none() && explicit {
            self.message = Some("Không có gợi ý".into());
        }
        cx.notify();
    }

    pub(crate) fn refilter_completion(&mut self, cx: &mut Context<Self>) {
        let head = self.editor.head;
        let Some(comp) = self.completion.as_mut() else { return };
        if head < comp.word_start || self.editor.buffer.line_of(head) != self.editor.buffer.line_of(comp.word_start) {
            self.completion = None;
            cx.notify();
            return;
        }
        let typed = self.editor.buffer.slice(comp.word_start..head).to_lowercase();
        let mut scored: Vec<(usize, u8, String)> = comp
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, it)| {
                let key = it["filterText"].as_str().or(it["label"].as_str()).unwrap_or("").to_lowercase();
                let rank = if key.starts_with(&typed) {
                    0
                } else if subsequence(&typed, &key) {
                    1
                } else {
                    return None;
                };
                Some((i, rank, it["sortText"].as_str().unwrap_or("").to_string()))
            })
            .collect();
        scored.sort_by(|a, b| a.1.cmp(&b.1).then(a.2.cmp(&b.2)));
        comp.visible = scored.into_iter().map(|(i, _, _)| i).take(200).collect();
        comp.selected = 0;
        if comp.visible.is_empty() && comp.request.is_none() {
            self.completion = None;
        }
        cx.notify();
    }

    fn move_completion(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let Some(c) = self.completion.as_mut() {
            let n = c.visible.len() as isize;
            if n > 0 {
                c.selected = ((c.selected as isize + delta).rem_euclid(n)) as usize;
            }
        }
        cx.notify();
    }

    fn accept_completion(&mut self, cx: &mut Context<Self>) {
        let Some(comp) = self.completion.take() else { return };
        let Some(&idx) = comp.visible.get(comp.selected) else { return };
        let item = comp.items[idx].clone();
        let head = self.editor.head;
        // main edit: from the textEdit's start (or the word start) to the cursor now
        let (start, text) = match item.get("textEdit") {
            Some(te) if te.is_object() => {
                let r = &te["range"]["start"];
                let s = self.editor.buffer.byte_of_lsp_position(r["line"].as_u64().unwrap_or(0) as usize, r["character"].as_u64().unwrap_or(0) as usize);
                (s.min(head), te["newText"].as_str().unwrap_or("").to_string())
            }
            _ => (comp.word_start, item["insertText"].as_str().or(item["label"].as_str()).unwrap_or("").to_string()),
        };
        // methods/constructors: add "()" like IntelliJ, caret inside when there are parameters
        let kind = item["kind"].as_u64().unwrap_or(0);
        let params = item["labelDetails"]["detail"].as_str().unwrap_or("").to_string();
        let is_call = matches!(kind, 2 | 3 | 4) && !text.contains('(');
        let next_is_paren = self.editor.buffer.slice(head..self.editor.buffer.next_grapheme(head)) == "(";
        let (text, caret_back) = if let Some(back) = item["data"]["caretBack"].as_u64() {
            // attribute="": caret between the quotes
            (text, back as usize)
        } else if is_call && !next_is_paren {
            if params.is_empty() || params == "()" { (format!("{text}()"), 0) } else { (format!("{text}()"), 1) }
        } else {
            (text, 0)
        };
        self.editor.buffer.break_group();
        self.editor.anchor = start;
        self.editor.head = head;
        self.editor.insert(&text);
        if caret_back > 0 {
            let at = self.editor.head - caret_back;
            self.editor.set_cursor(at, false);
        }
        self.editor.buffer.break_group();
        if caret_back > 0 {
            self.request_signature(false, cx);
        }
        if item["data"]["local"].as_bool() == Some(true) {
            // IDE-made item (Spring keys): nothing for jdtls to resolve
        } else if let Some(extra) = item["additionalTextEdits"].as_array().filter(|a| !a.is_empty()) {
            let extra = extra.clone();
            self.apply_text_edits(&extra);
        } else if self.lsp.is_some() {
            // jdtls computes auto-imports lazily: resolve, then apply additionalTextEdits
            self.lsp_sync();
            let doc = self.lsp.as_ref().unwrap();
            let req = doc.client.request("completionItem/resolve", item);
            cx.spawn(async move |this, cx| {
                if let Ok(resolved) = req.response().await {
                    if let Some(extra) = resolved["additionalTextEdits"].as_array().filter(|a| !a.is_empty()) {
                        let extra = extra.clone();
                        let _ = this.update(cx, |v, cx| {
                            v.apply_text_edits(&extra);
                            cx.notify();
                        });
                    }
                }
            })
            .detach();
        }
        cx.notify();
    }

    // ---- parameter info (signature help) -----------------------------------------------

    pub(crate) fn request_signature(&mut self, explicit: bool, cx: &mut Context<Self>) {
        let Some(_) = self.lsp.as_ref() else {
            if explicit {
                self.no_lsp(cx);
            }
            return;
        };
        self.lsp_sync();
        let Some((client, params)) = self.position_params(self.editor.head) else { return };
        let req = client.request("textDocument/signatureHelp", params);
        cx.spawn(async move |this, cx| {
            let res = req.response().await;
            let _ = this.update(cx, |v, cx| {
                let sigs = res.ok().filter(|r| !r.is_null());
                v.signature = sigs.and_then(|r| {
                    let signatures = r["signatures"].as_array().cloned().unwrap_or_default();
                    (!signatures.is_empty()).then(|| SignatureHelp {
                        active: r["activeSignature"].as_u64().unwrap_or(0) as usize,
                        param: r["activeParameter"].as_u64().unwrap_or(0) as usize,
                        signatures,
                    })
                });
                if v.signature.is_none() && explicit {
                    v.message = Some("Không có thông tin tham số ở đây".into());
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ---- code actions (Alt+Enter) -----------------------------------------------

    /// Alt+Enter (`kind` None: everything, quick fixes first) or one refactoring kind
    /// (Ctrl+Alt+V/C/F/M/P): a single match runs at once, several open the menu.
    /// jdtls ignores `context.only`, so the kind is filtered here.
    fn request_code_actions(&mut self, kind: Option<&'static str>, cx: &mut Context<Self>) {
        let Some(_) = self.lsp.as_ref().filter(|d| d.synced) else { return self.no_lsp(cx) };
        self.lsp_sync();
        // nothing selected: offer the expressions around the caret (IntelliJ's chooser)
        if let Some(k) = kind.filter(|k| matches!(*k, "refactor.extract.variable" | "refactor.extract.constant" | "refactor.extract.field")) {
            if !self.editor.has_selection() {
                return self.extract_around_caret(k, cx);
            }
        }
        let doc = self.lsp.as_ref().unwrap();
        let sel = self.editor.selection();
        let (s, e) = (self.editor.buffer.lsp_position(sel.start), self.editor.buffer.lsp_position(sel.end));
        // the diagnostics under the cursor are what quick fixes are computed for
        let h = self.editor.head;
        let uri = doc.uri.clone();
        let diags: Vec<Value> = self
            .diagnostics
            .iter()
            .filter(|d| d.range.start <= h && h <= d.range.end)
            .map(|d| {
                let (a, b) = (self.editor.buffer.lsp_position(d.range.start), self.editor.buffer.lsp_position(d.range.end));
                json!({ "range": { "start": { "line": a.0, "character": a.1 }, "end": { "line": b.0, "character": b.1 } },
                        "severity": d.severity, "message": d.message, "source": "Java" })
            })
            .collect();
        let req = doc.client.request(
            "textDocument/codeAction",
            json!({ "textDocument": { "uri": uri },
                    "range": { "start": { "line": s.0, "character": s.1 }, "end": { "line": e.0, "character": e.1 } },
                    "context": { "diagnostics": diags, "triggerKind": 1 } }),
        );
        let at = self.editor.head;
        cx.spawn(async move |this, cx| {
            let res = req.response().await;
            let _ = this.update(cx, |v, cx| {
                let mut items = res.ok().and_then(|r| r.as_array().cloned()).unwrap_or_default();
                if let Some(k) = kind.filter(|k| *k == "source") {
                    // Alt+Insert: every source action (getters/setters, constructors, toString,
                    // equals/hashCode, override methods…), generators first
                    items.retain(|a| a["kind"].as_str().is_some_and(|x| x.starts_with(k)));
                    items.sort_by_key(|a| !a["kind"].as_str().is_some_and(|x| x.contains("generate")));
                } else if let Some(k) = kind {
                    items.retain(|a| a["kind"].as_str() == Some(k));
                    // "this occurrence" before "replace all occurrences" (IntelliJ's default)
                    items.sort_by_key(|a| a["title"].as_str().is_some_and(|t| t.contains("replace all")));
                } else {
                    // quick fixes first, like IntelliJ's intention list
                    items.sort_by_key(|a| if a["kind"].as_str().is_some_and(|k| k.starts_with("quickfix")) { 0 } else { 1 });
                }
                if items.is_empty() {
                    v.message = Some(match kind {
                        Some("source") => "Không có gì để generate ở đây - đặt con trỏ trong một class".into(),
                        Some(_) => "Không tách được ở đây - đặt con trỏ trên (hoặc chọn) một biểu thức / đoạn lệnh".into(),
                        None => "Không có gợi ý sửa ở đây".into(),
                    });
                } else if kind.is_some_and(|k| k != "source") && items.len() == 1 {
                    v.actions_menu = Some(ActionsMenu { items, selected: 0, at });
                    v.accept_menu(cx);
                } else {
                    v.actions_menu = Some(ActionsMenu { items, selected: 0, at });
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn extract_around_caret(&mut self, kind: &'static str, cx: &mut Context<Self>) {
        let text = self.editor.buffer.text();
        let ranges = expr_candidates(&text, self.editor.head);
        if ranges.is_empty() {
            self.message = Some("Không tách được ở đây - đặt con trỏ trên (hoặc chọn) một biểu thức".into());
            return cx.notify();
        }
        let doc = self.lsp.as_ref().unwrap();
        let reqs: Vec<_> = ranges
            .iter()
            .map(|r| {
                let (s, e) = (self.editor.buffer.lsp_position(r.start), self.editor.buffer.lsp_position(r.end));
                let label: String = text[r.clone()].split_whitespace().collect::<Vec<_>>().join(" ");
                let label = if label.chars().count() > 50 { format!("{}…", label.chars().take(50).collect::<String>()) } else { label };
                let req = doc.client.request(
                    "textDocument/codeAction",
                    json!({ "textDocument": { "uri": doc.uri },
                            "range": { "start": { "line": s.0, "character": s.1 }, "end": { "line": e.0, "character": e.1 } },
                            "context": { "diagnostics": [], "triggerKind": 1, "only": [kind] } }),
                );
                async move { (label, req.response().await) }
            })
            .collect();
        let at = self.editor.head;
        cx.spawn(async move |this, cx| {
            let res = futures::future::join_all(reqs).await;
            let _ = this.update(cx, |v, cx| {
                let mut items = Vec::new();
                for (label, r) in res {
                    let mut acts: Vec<Value> = r.ok().and_then(|r| r.as_array().cloned()).unwrap_or_default();
                    acts.retain(|a| a["kind"].as_str() == Some(kind));
                    acts.sort_by_key(|a| a["title"].as_str().is_some_and(|t| t.contains("replace all")));
                    for mut a in acts {
                        let t = a["title"].as_str().unwrap_or("").to_string();
                        a["title"] = json!(format!("{label}  →  {t}"));
                        items.push(a);
                    }
                }
                if items.is_empty() {
                    v.message = Some("Không tách được ở đây - đặt con trỏ trên (hoặc chọn) một biểu thức".into());
                } else {
                    v.actions_menu = Some(ActionsMenu { items, selected: 0, at });
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn move_menu(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let Some(m) = self.actions_menu.as_mut() {
            let n = m.items.len() as isize;
            m.selected = ((m.selected as isize + delta).rem_euclid(n.max(1))) as usize;
        }
        cx.notify();
    }

    fn accept_menu(&mut self, cx: &mut Context<Self>) {
        if let Some(m) = self.actions_menu.take() {
            if let Some(a) = m.items.get(m.selected) {
                cx.emit(EditorEvent::RunAction(a.clone()));
            }
        }
        cx.notify();
    }

    // ---- rename (Shift+F6) ----------------------------------------------------------

    fn start_rename(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) {
        if self.read_only || self.lsp.as_ref().is_none_or(|d| !d.synced) {
            return self.no_lsp(cx);
        }
        let b = &self.editor.buffer;
        let h = self.editor.head;
        let line_start = b.line_start(b.line_of(h));
        let line = b.line_text(b.line_of(h));
        let col = h - line_start;
        let s = line[..col].char_indices().rev().take_while(|(_, c)| is_ident(*c)).last().map(|(i, _)| i).unwrap_or(col);
        let e = col + line[col..].chars().take_while(|c| is_ident(*c)).map(char::len_utf8).sum::<usize>();
        let word = line[s..e].to_string();
        if word.is_empty() {
            self.message = Some("Đặt con trỏ lên tên cần đổi".into());
            cx.notify();
            return;
        }
        let input = cx.new(|cx| crate::input::TextInput::new("Tên mới…", cx));
        input.update(cx, |i, cx| i.set_text(&word, cx));
        window.focus(&input.read(cx).focus_handle(cx));
        self.rename_old = word;
        self.rename_input = Some(input);
        cx.notify();
    }

    fn cancel_rename(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) {
        self.rename_input = None;
        window.focus(&self.focus_handle(cx));
        cx.notify();
    }

    fn confirm_rename(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) {
        let Some(input) = self.rename_input.take() else { return };
        let new_name = input.read(cx).content.trim().to_string();
        window.focus(&self.focus_handle(cx));
        if new_name.is_empty() {
            return;
        }
        self.lsp_sync();
        let Some((client, mut params)) = self.position_params(self.editor.head) else { return };
        params["newName"] = json!(new_name);
        self.message = Some(format!("Đang đổi tên thành {new_name}…"));
        let req = client.request("textDocument/rename", params);
        let old = std::mem::take(&mut self.rename_old);
        cx.spawn(async move |this, cx| {
            let res = req.response().await;
            let _ = this.update(cx, |v, cx| {
                match res {
                    Ok(edit) if !edit.is_null() => {
                        v.message = None;
                        cx.emit(EditorEvent::ApplyEdit(edit));
                        if !old.is_empty() && old != new_name {
                            cx.emit(EditorEvent::Renamed { old, new: new_name });
                        }
                    }
                    Ok(_) => v.message = Some("Không đổi tên được ở vị trí này".into()),
                    Err(e) => v.message = Some(format!("Đổi tên lỗi: {e}")),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    // ---- file structure (Ctrl+F12) ------------------------------------------------

    fn request_structure(&mut self, cx: &mut Context<Self>) {
        let Some(doc) = self.lsp.as_ref() else { return self.no_lsp(cx) };
        let req = doc.client.request("textDocument/documentSymbol", json!({ "textDocument": { "uri": doc.uri } }));
        cx.spawn(async move |this, cx| {
            let res = req.response().await;
            let _ = this.update(cx, |v, cx| {
                let mut out = Vec::new();
                if let Ok(Value::Array(syms)) = res {
                    flatten_symbols(&syms, 0, &mut out);
                }
                if out.is_empty() {
                    v.message = Some("Không có symbol".into());
                } else {
                    cx.emit(EditorEvent::Structure(out));
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ---- popups ------------------------------------------------------------------

    pub(crate) fn render_popups(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = Vec::new();
        let lh = self.line_height().unwrap_or(px(20.));
        if let Some(comp) = self.completion.as_ref().filter(|c| !c.visible.is_empty()) {
            if let Some(at) = self.cursor_window_point(comp.word_start) {
                let first = comp.selected.saturating_sub(9);
                let rows: Vec<AnyElement> = comp
                    .visible
                    .iter()
                    .enumerate()
                    .skip(first)
                    .take(10)
                    .map(|(i, &idx)| {
                        let it = &comp.items[idx];
                        let label = it["label"].as_str().unwrap_or("").to_string();
                        // "giam" + "(int gia, int phanTram)" … "int"
                        let params = it["labelDetails"]["detail"].as_str().unwrap_or("").to_string();
                        let detail = it["labelDetails"]["description"].as_str().or(it["detail"].as_str()).unwrap_or("").to_string();
                        let kind = kind_glyph(it["kind"].as_u64().unwrap_or(0));
                        div()
                            .flex()
                            .gap_2()
                            .px_2()
                            .whitespace_nowrap()
                            .mx_1()
                            .rounded_md()
                            .when(i == comp.selected, |d| d.bg(c(theme::ROW_ACTIVE)))
                            .child(div().w(px(14.)).text_color(c(kind.1)).child(kind.0))
                            .child(div().flex().child(div().text_color(c(theme::FG)).child(label)).child(div().text_color(c(theme::GUTTER_FG_ACTIVE)).child(params)))
                            .child(div().flex_1())
                            .child(div().pl_4().text_color(c(theme::FG_DIM)).child(detail))
                            .into_any_element()
                    })
                    .collect();
                let footer = format!("{}/{}  ↑↓ Enter Esc", comp.selected + 1, comp.visible.len());
                out.push(
                    deferred(
                        anchored().position(at).child(
                            div()
                                .min_w(px(320.))
                                .max_w(px(720.))
                                .bg(c(theme::POPUP_BG))
                                .border_1()
                                .border_color(c(theme::BORDER))
                                .shadow_lg().occlude().rounded_lg()
                                .children(rows)
                                .child(div().px_2().text_color(c(theme::FG_DIM)).text_size(px(11.)).child(footer))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()),
                        ),
                    )
                    .into_any_element(),
                );
            }
        }
        if let Some(menu) = &self.actions_menu {
            if let Some(at) = self.cursor_window_point(menu.at) {
                let rows: Vec<AnyElement> = menu
                    .items
                    .iter()
                    .enumerate()
                    .map(|(i, a)| {
                        let title = a["title"].as_str().unwrap_or("?").to_string();
                        let quick = a["kind"].as_str().is_some_and(|k| k.starts_with("quickfix"));
                        div()
                            .flex()
                            .gap_2()
                            .px_2()
                            .whitespace_nowrap()
                            .mx_1()
                            .rounded_md()
                            .when(i == menu.selected, |d| d.bg(c(theme::ROW_ACTIVE)))
                            .child(div().w(px(14.)).text_color(c(if quick { theme::WARNING } else { theme::FG_DIM })).child(if quick { "💡" } else { "·" }))
                            .child(div().text_color(c(theme::FG)).child(title))
                            .into_any_element()
                    })
                    .collect();
                out.push(
                    deferred(anchored().position(at).child(
                        div().min_w(px(280.)).max_w(px(720.)).py_1().bg(c(theme::POPUP_BG)).border_1().border_color(c(theme::BORDER)).shadow_lg().occlude().rounded_lg().children(rows),
                    ))
                    .into_any_element(),
                );
            }
        }
        if let Some(input) = &self.rename_input {
            if let Some(at) = self.cursor_window_point(self.editor.head) {
                out.push(
                    deferred(anchored().position(at).child(
                        div()
                            .key_context("RenameBox")
                            .w(px(320.))
                            .p_1()
                            .bg(c(theme::POPUP_BG))
                            .border_1()
                            .border_color(c(theme::BORDER))
                            .shadow_lg().occlude().rounded_lg()
                            .child(div().px_1().text_color(c(theme::FG_DIM)).text_size(px(11.)).child("Đổi tên (Enter đồng ý, Esc hủy)"))
                            .child(input.clone()),
                    ))
                    .into_any_element(),
                );
            }
        }
        if let Some(sig) = &self.signature {
            if let Some(at) = self.cursor_window_point(self.editor.head) {
                // above the current line, like IntelliJ's parameter info
                let lh = self.line_height().unwrap_or(px(20.));
                let at = gpui::point(at.x, at.y - lh);
                out.push(deferred(anchored().anchor(gpui::Corner::BottomLeft).position(at).child(render_signature(sig))).into_any_element());
            }
        }
        if let Some(text) = &self.hover {
            if let Some(at) = self.cursor_window_point(self.editor.head) {
                let lines: Vec<SharedString> = text.lines().take(30).map(|l| SharedString::from(l.to_string())).collect();
                out.push(
                    deferred(
                        anchored().position(at).child(
                            div()
                                .max_w(px(760.))
                                .p_2()
                                .bg(c(theme::POPUP_BG))
                                .border_1()
                                .border_color(c(theme::BORDER))
                                .shadow_lg().occlude().rounded_lg()
                                .text_color(c(theme::FG))
                                .children(lines.into_iter().map(|l| div().min_h(lh * 0.8).child(l)))
                                .on_mouse_down(MouseButton::Left, cx.listener(|v, _, _, cx| {
                                    v.hover = None;
                                    cx.notify()
                                })),
                        ),
                    )
                    .into_any_element(),
                );
            }
        }
        out
    }
}

/// Signature list, the active one first, its current parameter highlighted.
fn render_signature(sig: &SignatureHelp) -> AnyElement {
    let mut order: Vec<usize> = (0..sig.signatures.len()).collect();
    order.sort_by_key(|&i| i != sig.active);
    let rows: Vec<AnyElement> = order
        .into_iter()
        .take(6)
        .map(|i| {
            let s = &sig.signatures[i];
            let label = s["label"].as_str().unwrap_or("").to_string();
            let param = s["activeParameter"].as_u64().map(|p| p as usize).unwrap_or(sig.param);
            // the active parameter's text inside the label (labels are "Type name")
            let range = s["parameters"][param]["label"].as_str().and_then(|p| label.find(p).map(|at| at..at + p.len()));
            let (a, b, c2) = match range {
                Some(r) if i == sig.active => (label[..r.start].to_string(), label[r.clone()].to_string(), label[r.end..].to_string()),
                _ => (label.clone(), String::new(), String::new()),
            };
            div()
                .flex()
                .whitespace_nowrap()
                .px_2()
                .when(i != sig.active, |d| d.opacity(0.6))
                .child(div().text_color(c(theme::FG)).child(a))
                .child(div().text_color(c(theme::ACCENT)).font_weight(gpui::FontWeight::BOLD).child(b))
                .child(div().text_color(c(theme::FG)).child(c2))
                .into_any_element()
        })
        .collect();
    div()
        .py_1()
        .bg(c(theme::POPUP_BG))
        .border_1()
        .border_color(c(theme::BORDER))
        .rounded_md()
        .shadow_lg().occlude().rounded_lg()
        .children(rows)
        .when(sig.signatures.len() > 6, |d| d.child(div().px_2().text_size(px(10.)).text_color(c(theme::FG_DIM)).child(format!("+{} chữ ký khác", sig.signatures.len() - 6))))
        .into_any_element()
}

/// DocumentSymbol tree (or flat SymbolInformation list) → (name, detail, line, col16, depth).
fn flatten_symbols(syms: &[Value], depth: usize, out: &mut Vec<(String, String, usize, usize, usize)>) {
    for s in syms {
        let range = if s["selectionRange"].is_object() { &s["selectionRange"] } else { &s["location"]["range"] };
        out.push((
            s["name"].as_str().unwrap_or("").to_string(),
            s["detail"].as_str().unwrap_or("").to_string(),
            range["start"]["line"].as_u64().unwrap_or(0) as usize,
            range["start"]["character"].as_u64().unwrap_or(0) as usize,
            depth,
        ));
        if let Some(children) = s["children"].as_array() {
            flatten_symbols(children, depth + 1, out);
        }
    }
}

fn subsequence(needle: &str, hay: &str) -> bool {
    let mut it = hay.chars();
    needle.chars().all(|c| it.any(|h| h == c))
}

fn language_id(uri: &str) -> &'static str {
    if uri.ends_with(".java") { "java" } else if uri.ends_with(".xml") { "xml" } else { "plaintext" }
}

/// Plain text of a hover's `contents` (MarkupContent | MarkedString | MarkedString[]).
pub fn hover_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().map(hover_text).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n\n"),
        Value::Object(o) => o.get("value").and_then(Value::as_str).unwrap_or("").to_string(),
        _ => String::new(),
    }
    .replace("```java", "")
    .replace("```", "")
    .replace("&nbsp;", " ")
}

/// TextEdits for `uri` from a WorkspaceEdit (`changes` or `documentChanges`).
pub fn edits_for_uri(edit: &Value, uri: &str) -> Vec<Value> {
    if let Some(e) = edit["changes"][uri].as_array() {
        return e.clone();
    }
    edit["documentChanges"]
        .as_array()
        .map(|dc| {
            dc.iter()
                .filter(|d| d["textDocument"]["uri"] == uri)
                .flat_map(|d| d["edits"].as_array().cloned().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default()
}

fn kind_glyph(kind: u64) -> (&'static str, u32) {
    match kind {
        2 | 3 | 4 => ("m", 0x56a8f5),  // method/function/constructor
        5 | 10 => ("f", 0xc77dbb),     // field/property
        6 => ("v", 0xbcbec4),          // variable
        7 | 22 => ("C", 0x3574f0),     // class/struct
        8 => ("I", 0x6aab73),          // interface
        9 => ("P", 0x8c8f96),          // module/package
        13 | 20 => ("E", 0xd5b778),    // enum/enum member
        14 => ("k", 0xcf8e6d),         // keyword
        21 => ("c", 0xc77dbb),         // constant
        _ => ("·", 0x8c8f96),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_edit_and_hover_text() {
        let edit = json!({ "documentChanges": [{ "textDocument": { "uri": "file:///a.java", "version": 3 }, "edits": [{ "newText": "x" }] }] });
        assert_eq!(edits_for_uri(&edit, "file:///a.java").len(), 1);
        assert_eq!(edits_for_uri(&json!({ "changes": { "file:///b.java": [{}, {}] } }), "file:///b.java").len(), 2);
        assert_eq!(hover_text(&json!({ "kind": "markdown", "value": "```java\nString x\n```" })).trim(), "String x");
        assert!(subsequence("gtnm", "getname"));
    }
}

/// True when an LSP Location/LocationLink points at `pos` in `uri` (the name being declared).
fn is_at(loc: &Value, uri: &Value, pos: &Value) -> bool {
    let (target, range) = match loc.get("targetUri") {
        Some(t) => (t, &loc["targetSelectionRange"]),
        None => (&loc["uri"], &loc["range"]),
    };
    if target != uri {
        return false;
    }
    let at = |p: &Value| (p["line"].as_u64().unwrap_or(0), p["character"].as_u64().unwrap_or(0));
    let (s, e, p) = (at(&range["start"]), at(&range["end"]), at(pos));
    s <= p && p <= e
}

#[cfg(test)]
mod goto_tests {
    use super::is_at;
    use serde_json::json;

    #[test]
    fn declaration_under_cursor() {
        let uri = json!("file:///p/A.java");
        let loc = json!({ "uri": "file:///p/A.java", "range": { "start": { "line": 9, "character": 16 }, "end": { "line": 9, "character": 23 } } });
        assert!(is_at(&loc, &uri, &json!({ "line": 9, "character": 20 })));
        assert!(!is_at(&loc, &uri, &json!({ "line": 12, "character": 20 })));
        assert!(!is_at(&loc, &json!("file:///p/B.java"), &json!({ "line": 9, "character": 20 })));
        let link = json!({ "targetUri": "file:///p/A.java", "targetSelectionRange": { "start": { "line": 3, "character": 4 }, "end": { "line": 3, "character": 8 } } });
        assert!(is_at(&link, &uri, &json!({ "line": 3, "character": 8 })));
    }
}

#[cfg(test)]
mod semantic_tests {
    use super::*;

    #[test]
    fn decode_relative_tokens() {
        let b = editor_core::Buffer::from_text("class A {\n  static int MAX;\n  String name;\n}\n");
        let legend = json!({ "tokenTypes": ["class", "property", "method"], "tokenModifiers": ["declaration", "static", "deprecated"] });
        // A (class, skipped) · MAX (property, static|declaration) · name (property, deprecated)
        let data = json!([0, 6, 1, 0, 0, 1, 13, 3, 1, 3, 1, 9, 4, 1, 4]);
        let s = decode_semantic(&b, &data, &legend);
        let text = b.text();
        let got: Vec<(&str, bool, bool, bool)> = s.iter().map(|(r, x)| (&text[r.clone()], x.field, x.is_static, x.deprecated)).collect();
        assert_eq!(got, [("MAX", true, true, false), ("name", true, false, true)]);
    }
}

/// Expressions around `caret`, innermost first: the call / field chain at the caret, then
/// each call whose arguments contain it (`a.b(x, c.d()|)` → `c.d()`, `a.b(x, c.d())`).
fn expr_candidates(text: &str, caret: usize) -> Vec<std::ops::Range<usize>> {
    let b = text.as_bytes();
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'$' || c >= 0x80;
    let close_fwd = |mut i: usize| -> Option<usize> {
        // b[i] is an opener: index after its closer
        let mut d = 0i32;
        while i < b.len() {
            match b[i] {
                b'(' | b'[' => d += 1,
                b')' | b']' => {
                    d -= 1;
                    if d == 0 {
                        return Some(i + 1);
                    }
                }
                b';' | b'{' | b'}' => return None,
                _ => {}
            }
            i += 1;
        }
        None
    };
    // start of the chain ending at `end`
    let chain_start = |end: usize| -> usize {
        let mut i = end;
        loop {
            if i == 0 {
                break;
            }
            let c = b[i - 1];
            if c == b')' || c == b']' {
                let mut d = 0i32;
                let mut j = i;
                while j > 0 {
                    j -= 1;
                    match b[j] {
                        b')' | b']' => d += 1,
                        b'(' | b'[' => {
                            d -= 1;
                            if d == 0 {
                                break;
                            }
                        }
                        b';' | b'{' | b'}' => return i,
                        _ => {}
                    }
                }
                i = j;
            } else if c == b'"' {
                let mut j = i - 1;
                while j > 0 && !(b[j - 1] == b'"' && (j < 2 || b[j - 2] != b'\\')) {
                    j -= 1;
                }
                i = j.saturating_sub(1);
            } else if ident(c) || c == b'.' {
                i -= 1;
            } else {
                break;
            }
        }
        while i < end && b[i] == b'.' {
            i += 1;
        }
        i
    };
    let mut caret = caret.min(b.len());
    while caret > 0 && !text.is_char_boundary(caret) {
        caret -= 1;
    }
    // end of the expression at the caret: rest of the identifier, then its call / index
    let mut end = caret;
    while end < b.len() && ident(b[end]) {
        end += 1;
    }
    while end < b.len() && (b[end] == b'(' || b[end] == b'[') {
        match close_fwd(end) {
            Some(e) => end = e,
            None => break,
        }
    }
    let mut out: Vec<std::ops::Range<usize>> = Vec::new();
    let start = chain_start(end);
    if start < end {
        out.push(start..end);
    }
    // enclosing calls
    let mut from = if start < end { start } else { caret };
    for _ in 0..3 {
        let mut d = 0i32;
        let mut j = from;
        let mut open = None;
        while j > 0 {
            j -= 1;
            match b[j] {
                b')' | b']' => d += 1,
                b'(' | b'[' => {
                    if d == 0 {
                        open = Some(j);
                        break;
                    }
                    d -= 1;
                }
                b';' | b'{' | b'}' => break,
                _ => {}
            }
        }
        let Some(o) = open else { break };
        let Some(e) = close_fwd(o) else { break };
        let s = chain_start(o);
        // `if (`, `for (`: a statement, not an expression
        let head = &text[s..o];
        if s == o || matches!(head.trim(), "if" | "for" | "while" | "switch" | "catch" | "synchronized" | "return") {
            break;
        }
        if !out.contains(&(s..e)) {
            out.push(s..e);
        }
        from = s;
    }
    out
}

#[cfg(test)]
mod expr_tests {
    use super::expr_candidates;

    #[test]
    fn caret_after_call_offers_it_and_the_enclosing_call() {
        let t = "    c.build(null, \"X\", dto.getId());";
        let caret = t.find("());").unwrap() + 2;
        let r: Vec<&str> = expr_candidates(t, caret).into_iter().map(|r| &t[r]).collect();
        assert_eq!(r, vec!["dto.getId()", "c.build(null, \"X\", dto.getId())"]);
    }

    #[test]
    fn caret_inside_identifier() {
        let t = "int a = user.getName().trim();";
        let caret = t.find("Name").unwrap();
        let r: Vec<&str> = expr_candidates(t, caret).into_iter().map(|r| &t[r]).collect();
        assert_eq!(r[0], "user.getName()");
    }
}
