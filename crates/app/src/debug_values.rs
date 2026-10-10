//! Seeing values while stopped in the debugger, IntelliJ-style: inline values at the end of
//! lines, a value popup when hovering an expression, Evaluate Expression (Alt+F8) and
//! Watches. Everything evaluates in the selected stack frame through DAP `evaluate`, and
//! object values expand lazily through the session's `variables` cache.
use crate::debug::{State, Var};
use crate::dialogs::{self, DialogCancel, DialogConfirm};
use crate::input::TextInput;
use crate::theme::{self, c};
use crate::workspace::Workspace;
use gpui::{AnyElement, Context, Entity, Focusable, MouseButton, Pixels, Point, SharedString, Window, anchored, deferred, div, prelude::*, px};
use serde_json::json;
use std::path::PathBuf;

// ---- pure helpers ---------------------------------------------------------------

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// The expression a hover at byte `col` of `line` means: the identifier under the mouse
/// plus the `a.b.` chain before it (like IntelliJ: hovering `c` in `a.b.c.d` gives `a.b.c`).
pub fn expr_at(line: &str, col: usize) -> Option<String> {
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    let idx = chars.iter().position(|(i, _)| *i >= col.min(line.len())).unwrap_or(chars.len());
    let at = if idx < chars.len() && is_ident(chars[idx].1) {
        idx
    } else if idx > 0 && is_ident(chars[idx - 1].1) {
        idx - 1
    } else {
        return None;
    };
    let mut end = at;
    while end + 1 < chars.len() && is_ident(chars[end + 1].1) {
        end += 1;
    }
    let mut start = at;
    loop {
        while start > 0 && is_ident(chars[start - 1].1) {
            start -= 1;
        }
        // extend over "x." to the left
        if start >= 2 && chars[start - 1].1 == '.' && is_ident(chars[start - 2].1) {
            start -= 1;
            continue;
        }
        break;
    }
    let s: String = chars[start..=end].iter().map(|(_, c)| c).collect();
    // keywords and numbers are not worth evaluating
    let first = s.split('.').next().unwrap_or("");
    if first.chars().next().is_some_and(|c| c.is_ascii_digit()) || JAVA_KEYWORDS.contains(&first) {
        return None;
    }
    Some(s)
}

const JAVA_KEYWORDS: &[&str] = &[
    "public", "private", "protected", "static", "final", "void", "class", "interface", "enum", "return", "new", "if", "else", "for",
    "while", "do", "switch", "case", "default", "break", "continue", "try", "catch", "finally", "throw", "throws", "import",
    "package", "extends", "implements", "int", "long", "short", "byte", "char", "boolean", "double", "float", "true", "false",
    "null", "var", "instanceof", "synchronized", "abstract", "native", "transient", "volatile",
];

fn word_occurs(line: &str, name: &str) -> bool {
    line.match_indices(name).any(|(i, _)| {
        let before = line[..i].chars().next_back();
        let after = line[i + name.len()..].chars().next();
        !before.is_some_and(|c| is_ident(c) || c == '.') && !after.is_some_and(is_ident)
    })
}

/// Inline values: each local shown once, on the last line at or before the stop line that
/// mentions it (within `window` lines), like IntelliJ's grey "name: value" hints.
pub fn inline_values(lines: &[String], stop_line: usize, vars: &[Var], window: usize) -> Vec<(usize, String)> {
    let first = stop_line.saturating_sub(window);
    let mut per_line: std::collections::BTreeMap<usize, Vec<String>> = Default::default();
    for v in vars {
        if v.name == "this" {
            continue;
        }
        let hit = (first..=stop_line.min(lines.len().saturating_sub(1))).rev().find(|&l| {
            let t = lines[l].trim_start();
            // comments don't count
            !t.starts_with("//") && !t.starts_with('*') && word_occurs(&lines[l], &v.name)
        });
        if let Some(l) = hit {
            let mut val = v.value.clone();
            if val.chars().count() > 60 {
                val = val.chars().take(60).collect::<String>() + "…";
            }
            per_line.entry(l).or_default().push(format!("{}: {val}", v.name));
        }
    }
    per_line.into_iter().map(|(l, parts)| (l, parts.join("   "))).collect()
}

// ---- workspace state ----------------------------------------------------------------

pub struct ValuePopup {
    at: Point<Pixels>,
    var: Result<Var, String>,
}

pub struct EvalDialog {
    input: Entity<TextInput>,
    results: Vec<Result<Var, String>>,
}

impl Workspace {
    /// The stopped session and its selected frame id, if any.
    fn stopped_frame(&self) -> Option<(usize, i64)> {
        let s = self.active_session().filter(|s| matches!(s.state, State::Stopped { .. }))?;
        let f = s.frames.get(s.selected_frame)?;
        Some((s.id, f.id))
    }

    /// DAP `evaluate` in the selected frame; `done` gets the value (as a Var) or the error.
    pub(crate) fn evaluate(&mut self, expr: String, context: &'static str, cx: &mut Context<Self>, done: impl FnOnce(&mut Self, Result<Var, String>, &mut Context<Self>) + 'static) {
        let Some((id, frame)) = self.stopped_frame() else {
            done(self, Err("chưa dừng ở breakpoint nào".into()), cx);
            return;
        };
        let Some(client) = self.sessions.iter().find(|s| s.id == id).and_then(|s| s.dap_client()) else { return };
        let req = client.request("evaluate", json!({ "expression": expr, "frameId": frame, "context": context }));
        cx.spawn(async move |this, cx| {
            let res = req.response().await;
            let _ = this.update(cx, |ws, cx| {
                let v = res
                    .map(|b| Var {
                        name: expr.clone(),
                        value: b["result"].as_str().unwrap_or("").to_string(),
                        reference: b["variablesReference"].as_i64().unwrap_or(0),
                    })
                    .map_err(|e| e.to_string());
                done(ws, v, cx);
                cx.notify();
            });
        })
        .detach();
    }

    // ---- inline values ----

    /// Puts "name: value" hints into the editor showing the stopped frame.
    pub(crate) fn refresh_inline_values(&mut self, cx: &mut Context<Self>) {
        let Some(s) = self.active_session().filter(|s| matches!(s.state, State::Stopped { .. })) else { return };
        let Some(frame) = s.frames.get(s.selected_frame).cloned() else { return };
        let (Some(path), Some(locals)) = (frame.path.clone(), s.locals_ref.and_then(|r| s.vars.get(&r)).cloned()) else { return };
        let Some(t) = self.tabs.iter().find(|t| t.read(cx).path() == Some(&path)).cloned() else { return };
        let stop = frame.line.saturating_sub(1);
        let lines: Vec<String> = {
            let b = &t.read(cx).editor.buffer;
            (0..b.line_count()).map(|l| b.line_text(l)).collect()
        };
        let values = inline_values(&lines, stop, &locals, 80);
        t.update(cx, |v, cx| {
            v.inline_values = values;
            cx.notify()
        });
    }

    pub(crate) fn clear_inline_values(&mut self, cx: &mut Context<Self>) {
        self.value_popup = None;
        for t in &self.tabs {
            if !t.read(cx).inline_values.is_empty() {
                t.update(cx, |v, cx| {
                    v.inline_values.clear();
                    cx.notify()
                });
            }
        }
    }

    /// Tabs know whether hovering should ask the debugger for values.
    pub(crate) fn sync_debug_hover_flag(&mut self, cx: &mut Context<Self>) {
        let on = self.stopped_frame().is_some();
        for t in &self.tabs {
            if t.read(cx).debug_active != on {
                t.update(cx, |v, _| v.debug_active = on);
            }
        }
    }

    // ---- hover popup ----

    pub(crate) fn show_value_popup(&mut self, expr: String, at: Point<Pixels>, cx: &mut Context<Self>) {
        self.evaluate(expr, "hover", cx, move |ws, v, _| {
            // an unknown name under the mouse is not worth a popup
            if v.is_ok() {
                ws.value_popup = Some(ValuePopup { at, var: v });
            }
        });
    }

    // ---- watches ----

    fn watches_file(&self) -> PathBuf {
        project_model::cache::state_dir(&self.root_str()).join("chodilamide-watches.json")
    }

    pub(crate) fn load_watches(&mut self) {
        self.watches = std::fs::read_to_string(self.watches_file()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    }

    fn save_watches(&self) {
        if let Ok(b) = serde_json::to_vec(&self.watches) {
            let _ = project_model::cache::write_atomic(&self.watches_file(), &b);
        }
    }

    /// Re-evaluates every watch in the current frame (after each stop / frame change).
    pub(crate) fn refresh_watches(&mut self, cx: &mut Context<Self>) {
        self.watch_values = vec![None; self.watches.len()];
        for (i, w) in self.watches.clone().into_iter().enumerate() {
            self.evaluate(w, "watch", cx, move |ws, v, _| {
                if let Some(slot) = ws.watch_values.get_mut(i) {
                    *slot = Some(v);
                }
            });
        }
    }

    fn add_watch(&mut self, expr: String, cx: &mut Context<Self>) {
        let expr = expr.trim().to_string();
        if expr.is_empty() || self.watches.contains(&expr) {
            return;
        }
        self.watches.push(expr);
        self.save_watches();
        self.refresh_watches(cx);
    }

    fn remove_watch(&mut self, i: usize, cx: &mut Context<Self>) {
        if i < self.watches.len() {
            self.watches.remove(i);
            self.save_watches();
            self.refresh_watches(cx);
        }
    }

    fn watch_input(&mut self, cx: &mut Context<Self>) -> Entity<TextInput> {
        if self.watch_input.is_none() {
            let i = cx.new(|cx| TextInput::new("+ biểu thức theo dõi (Enter)", cx));
            self.watch_input = Some(i);
        }
        self.watch_input.clone().unwrap()
    }

    // ---- Evaluate Expression (Alt+F8) ----

    pub(crate) fn open_evaluate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // prefill: the selection, or the expression at the cursor
        let initial = self
            .active
            .and_then(|i| self.tabs.get(i))
            .map(|t| {
                let e = &t.read(cx).editor;
                let sel = e.buffer.slice(e.selection());
                if !sel.is_empty() && !sel.contains('\n') {
                    sel
                } else {
                    let line = e.buffer.line_of(e.head);
                    let text = e.buffer.line_text(line);
                    expr_at(&text, e.head - e.buffer.line_start(line)).unwrap_or_default()
                }
            })
            .unwrap_or_default();
        let input = cx.new(|cx| TextInput::new("biểu thức Java, vd: order.getItems().size()", cx));
        input.update(cx, |i, cx| i.set_text(&initial, cx));
        dialogs::focus_later(input.read(cx).focus_handle(cx), window, cx);
        self.eval_dialog = Some(EvalDialog { input, results: Vec::new() });
        cx.notify();
    }

    fn run_evaluate(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.eval_dialog.as_ref() else { return };
        let expr = d.input.read(cx).content.trim().to_string();
        if expr.is_empty() {
            return;
        }
        self.evaluate(expr, "repl", cx, |ws, v, _| {
            if let Some(d) = ws.eval_dialog.as_mut() {
                d.results.insert(0, v);
                d.results.truncate(20);
            }
        });
    }

    fn close_evaluate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.eval_dialog = None;
        if let Some(i) = self.active {
            self.activate(i, window, cx);
        }
        cx.notify();
    }

    // ---- rendering ----

    /// Rows of a value tree: roots, then expanded children (lazily fetched) indented.
    fn value_rows(&self, roots: &[Var]) -> Vec<(usize, Var)> {
        let Some(s) = self.active_session() else { return roots.iter().map(|v| (0, v.clone())).collect() };
        let mut out = Vec::new();
        fn walk(s: &crate::debug::Session, vars: &[Var], depth: usize, out: &mut Vec<(usize, Var)>) {
            for v in vars {
                out.push((depth, v.clone()));
                if v.reference > 0 && s.expanded.contains(&v.reference) && out.len() < 1500 {
                    if let Some(kids) = s.vars.get(&v.reference) {
                        walk(s, kids, depth + 1, out);
                    }
                }
            }
        }
        walk(s, roots, 0, &mut out);
        out
    }

    fn value_tree(&self, prefix: &'static str, roots: &[Var], cx: &mut Context<Self>) -> Vec<AnyElement> {
        let sid = self.active_session.unwrap_or(0);
        let expanded = self.active_session().map(|s| s.expanded.clone()).unwrap_or_default();
        self.value_rows(roots)
            .into_iter()
            .enumerate()
            .map(|(i, (depth, v))| {
                let r = v.reference;
                let arrow = if r == 0 { " " } else if expanded.contains(&r) { "▾" } else { "▸" };
                div()
                    .id((prefix, i))
                    .flex()
                    .gap_1()
                    .pl(px(6. + 14. * depth as f32))
                    .pr_2()
                    .whitespace_nowrap()
                    .hover(|st| st.bg(c(theme::ROW_HOVER)))
                    .child(div().w(px(10.)).text_color(c(theme::FG_DIM)).child(arrow))
                    .child(div().text_color(c(0xc77dbb)).child(v.name.clone()))
                    .child(div().text_color(c(theme::FG_DIM)).child("="))
                    .child(div().text_color(c(theme::FG)).child(v.value.clone()))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, cx| {
                        cx.stop_propagation();
                        if r > 0 {
                            ws.toggle_var(sid, r, cx)
                        }
                    }))
                    .into_any_element()
            })
            .collect()
    }

    pub(crate) fn render_value_popup(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = self.value_popup.as_ref()?;
        let at = p.at + gpui::point(px(8.), px(14.));
        let body: Vec<AnyElement> = match &p.var {
            Ok(v) => {
                let v = v.clone();
                self.value_tree("hover-val", &[v], cx)
            }
            Err(e) => vec![div().px_2().text_color(c(theme::ERROR)).child(SharedString::from(e.clone())).into_any_element()],
        };
        Some(
            deferred(
                anchored().position(at).child(
                    div()
                        .id("value-popup")
                        .min_w(px(260.))
                        .max_w(px(760.))
                        .max_h(px(420.))
                        .overflow_y_scroll()
                        .py_1()
                        .bg(c(theme::POPUP_BG))
                        .border_1()
                        .border_color(c(theme::BORDER))
                        .rounded_md()
                        .shadow_lg().occlude().rounded_lg()
                        .font_family("JetBrains Mono")
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .px_2()
                                .text_size(px(10.))
                                .text_color(c(theme::FG_DIM))
                                .child("Alt+F8: Evaluate  ·  bấm ▸ để mở  ·")
                                .child(div().id("vp-close").pl_1().hover(|s| s.text_color(c(theme::FG))).child("×").on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|ws, _, _, cx| {
                                        ws.value_popup = None;
                                        cx.notify()
                                    }),
                                )),
                        )
                        .children(body),
                ),
            )
            .into_any_element(),
        )
    }

    pub(crate) fn render_watches(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let input = self.watch_input(cx);
        let stopped = self.stopped_frame().is_some();
        let mut rows: Vec<AnyElement> = Vec::new();
        for (i, w) in self.watches.clone().into_iter().enumerate() {
            let value = self.watch_values.get(i).cloned().flatten();
            let head = div()
                .id(("watch-rm", i))
                .px_1()
                .text_color(c(theme::FG_DIM))
                .hover(|s| s.text_color(c(theme::ERROR)))
                .child("×")
                .on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, cx| ws.remove_watch(i, cx)));
            match value {
                Some(Ok(v)) => {
                    let tree = self.value_tree("watch-val", &[v], cx);
                    rows.push(div().flex().flex_row().child(head).child(div().flex_1().children(tree)).into_any_element());
                }
                other => {
                    let text = match other {
                        Some(Err(e)) => format!("{w} = <{e}>"),
                        _ if stopped => format!("{w} = …"),
                        _ => format!("{w} (chưa dừng)"),
                    };
                    rows.push(div().flex().flex_row().child(head).child(div().px_2().text_color(c(theme::FG_DIM)).child(text)).into_any_element());
                }
            }
        }
        div()
            .flex()
            .flex_col()
            .border_b_1()
            .border_color(c(theme::BORDER))
            .pb_1()
            .child(div().px_2().text_color(c(theme::ACCENT)).child("Watches"))
            .children(rows)
            .child(
                div()
                    .key_context("WatchBox")
                    .px_1()
                    .on_action(cx.listener(move |ws, _: &DialogConfirm, _, cx| {
                        if let Some(i) = ws.watch_input.clone() {
                            let e = i.read(cx).content.clone();
                            i.update(cx, |i, cx| i.set_value("", cx));
                            ws.add_watch(e, cx);
                        }
                    }))
                    .child(input),
            )
            .into_any_element()
    }

    pub(crate) fn render_evaluate(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let d = self.eval_dialog.as_ref()?;
        let input = d.input.clone();
        let results = d.results.clone();
        let mut body = div().flex().flex_col().gap_1().child(input);
        if self.stopped_frame().is_none() {
            body = body.child(div().text_color(c(theme::WARNING)).child("Chưa dừng ở breakpoint - hãy Debug và dừng lại trước"));
        }
        for (i, r) in results.iter().enumerate() {
            let el: AnyElement = match r {
                Ok(v) => div().children(self.value_tree(if i == 0 { "eval-0" } else { "eval-n" }, &[v.clone()], cx)).into_any_element(),
                Err(e) => div().px_2().text_color(c(theme::ERROR)).child(SharedString::from(e.clone())).into_any_element(),
            };
            body = body.child(div().when(i > 0, |d| d.opacity(0.6)).child(el));
        }
        let footer = div()
            .flex()
            .gap_2()
            .child(dialogs::button("ev-close", "Đóng (Esc)", false, cx, |ws: &mut Workspace, window, cx| ws.close_evaluate(window, cx)))
            .child(dialogs::button("ev-run", "Evaluate (Enter)", true, cx, |ws: &mut Workspace, _, cx| ws.run_evaluate(cx)));
        Some(
            dialogs::modal("ev-body", "Evaluate Expression", 760., body, footer)
                .on_action(cx.listener(|ws, _: &DialogConfirm, _, cx| ws.run_evaluate(cx)))
                .on_action(cx.listener(|ws, _: &DialogCancel, window, cx| ws.close_evaluate(window, cx)))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hover_expression_under_mouse() {
        let l = "        String msg = order.customer.name + count;";
        let at = |s: &str| l.find(s).unwrap();
        assert_eq!(expr_at(l, at("msg")).as_deref(), Some("msg"));
        assert_eq!(expr_at(l, at("customer") + 2).as_deref(), Some("order.customer"));
        assert_eq!(expr_at(l, at("name") + 1).as_deref(), Some("order.customer.name"));
        assert_eq!(expr_at(l, at("count")).as_deref(), Some("count"));
        assert_eq!(expr_at(l, at("String")).as_deref(), Some("String"));
        assert_eq!(expr_at(l, at("=")), None);
        assert_eq!(expr_at("return x;", 1), None); // keyword
    }

    #[test]
    fn inline_values_last_mention_before_stop() {
        let src = ["void greet(String name) {", "  count++;", "  // name here in a comment", "  String msg = prefix + name;", "  List<String> parts = x;", "  return msg;"];
        let lines: Vec<String> = src.iter().map(|s| s.to_string()).collect();
        let v = |n: &str, val: &str| Var { name: n.into(), value: val.into(), reference: 0 };
        let vars = vec![v("this", "Greeter@1"), v("name", "\"Lan\""), v("msg", "\"Xin chào Lan\""), v("parts", "ArrayList (size=2)")];
        // stopped on line 4 (0-based): `parts` is declared there, msg on 3, name last used on 3
        let got = inline_values(&lines, 4, &vars, 80);
        assert_eq!(got, vec![(3, "name: \"Lan\"   msg: \"Xin chào Lan\"".into()), (4, "parts: ArrayList (size=2)".into())]);
    }
}
