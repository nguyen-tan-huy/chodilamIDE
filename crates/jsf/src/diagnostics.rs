//! IntelliJ-style inspections for Facelets (.xhtml) - port of java-debug-model's
//! jsf/diagnostics.lua:
//!   * EL that doesn't resolve   `#{bean.nope}` → error (type-aware, through jdtls)
//!   * ajax ids                  update/process/render/execute naming an id not in the page
//!   * `update="@all"`           re-renders the whole view
//!   * `<c:forEach>`/`<c:if>`    build-time tags, not re-evaluated by ajax
//!   * `<p:dataTable paginator>` without lazy="true" keeps every row in memory
use crate::beans::BeanIndex;
use crate::el::{self, ElRef, RefKind, IMPLICIT_OBJECTS};
use crate::resolve::Ctx;
use std::collections::{HashMap, HashSet};

pub const ERROR: u8 = 1;
pub const WARNING: u8 = 2;
pub const HINT: u8 = 4;

#[derive(Debug, Clone, PartialEq)]
pub struct Diag {
    /// byte range in the page (end exclusive)
    pub start: usize,
    pub end: usize,
    pub severity: u8,
    pub message: String,
    pub code: &'static str,
}

const AJAX_ATTRS: &[&str] = &["update", "process", "render", "execute"];
const SEARCH_KEYWORDS: &[&str] = &[
    "@this", "@form", "@all", "@none", "@parent", "@namingcontainer", "@composite", "@previous", "@next", "@child", "@root", "@id", "@widgetVar", "@obs", "@row", "@first", "@last",
];

fn has_el(s: &str) -> bool {
    s.contains("#{") || s.contains("${")
}

pub fn static_checks(text: &str) -> Vec<Diag> {
    let mut out = Vec::new();
    let tags = el::tags(text, text.len());
    let mut ids = HashSet::new();
    let mut dynamic_ids = false;
    for t in &tags {
        if let Some(id) = t.attrs.get("id") {
            if has_el(&id.value) {
                dynamic_ids = true;
            } else {
                ids.insert(id.value.clone());
            }
        }
    }
    // ids coming from templates/includes/composites aren't visible here: only flag plain
    // relative ids on a page that includes nothing
    let composition_without_template = text.contains("<ui:composition") && !tags.iter().any(|t| t.name == "ui:composition" && t.attrs.contains_key("template"));
    let includes_others = ["<ui:include", "<ui:insert", "<ui:decorate", ":implementation>", "<ui:component"].iter().any(|s| text.contains(s))
        || !el::composite_namespaces(text).is_empty()
        || composition_without_template;

    for t in &tags {
        for (name, attr) in &t.attrs {
            if !AJAX_ATTRS.contains(&name.as_str()) || has_el(&attr.value) {
                continue;
            }
            let mut from = 0;
            for token in attr.value.split([' ', ',', '\t', '\n']).filter(|s| !s.is_empty()) {
                let ts = attr.value[from..].find(token).map_or(from, |i| from + i);
                from = ts + token.len();
                let (s, e) = (attr.vs + ts, attr.vs + ts + token.len());
                if token == "@all" {
                    out.push(Diag { start: s, end: e, severity: HINT, message: "update=\"@all\" render lại toàn bộ trang - chỉ update đúng vùng cần.".into(), code: "ajax-all" });
                } else if let Some(rest) = token.strip_prefix('@') {
                    // "@(.css-selector)" is a jQuery selector - anything goes
                    let kw: String = rest.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
                    if !kw.is_empty() && !SEARCH_KEYWORDS.contains(&format!("@{kw}").as_str()) {
                        out.push(Diag { start: s, end: e, severity: WARNING, message: format!("Search expression lạ: {token}"), code: "ajax-keyword" });
                    }
                } else if !token.contains(':') && !dynamic_ids && !includes_others && !ids.contains(token) && !token.contains(['(', ')', '*']) {
                    out.push(Diag {
                        start: s,
                        end: e,
                        severity: WARNING,
                        message: format!("Không tìm thấy component id '{token}' trong trang ({name}=). PrimeFaces sẽ lỗi \"Cannot find component\" lúc chạy."),
                        code: "ajax-id",
                    });
                }
            }
        }
        let (ns, ne) = (t.s + 1, t.s + 1 + t.name.len());
        if matches!(t.name.as_str(), "c:forEach" | "c:if" | "c:choose") {
            let alt = if t.name == "c:forEach" { "<ui:repeat>/<p:repeat>" } else { "rendered=\"#{...}\"" };
            out.push(Diag { start: ns, end: ne, severity: HINT, message: format!("<{}> chạy lúc build view, không cập nhật theo ajax - dùng {alt}.", t.name), code: "jstl-build-time" });
        }
        if t.name == "p:dataTable" && t.attrs.get("paginator").is_some_and(|a| a.value == "true") && !t.attrs.get("lazy").is_some_and(|a| a.value != "false") {
            out.push(Diag {
                start: ns,
                end: ne,
                severity: HINT,
                message: "p:dataTable phân trang nhưng không lazy - cả danh sách nằm trong bộ nhớ. Dùng lazy=\"true\" + LazyDataModel.".into(),
                code: "datatable-lazy",
            });
        }
    }
    out.sort_by_key(|d| d.start);
    out
}

/// A reference chain of the page targeting its last named segment, with its byte span.
#[derive(Debug, Clone)]
pub struct ElChain {
    pub r: ElRef,
    pub start: usize,
    pub end: usize,
}

/// Every reference chain in the page's EL expressions (`#{a.b.c}` and each chain inside a
/// larger expression like `#{a.x and b.y(c.z)}`).
pub fn el_chains(text: &str) -> Vec<ElChain> {
    let t = text.as_bytes();
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for sp in el::el_spans(text) {
        let (body, end) = (sp.body, sp.end);
        let mut i = body;
        let mut quote = None;
        while i < end {
            let ch = t[i];
            if let Some(q) = quote {
                if ch == q {
                    quote = None;
                }
                i += 1;
                continue;
            }
            if ch == b'\'' || ch == b'"' {
                quote = Some(ch);
                i += 1;
                continue;
            }
            let starts_ident = ch.is_ascii_alphabetic() || ch == b'_';
            let prev = if i > body { t[i - 1] } else { b' ' };
            if !starts_ident || prev == b'.' || prev.is_ascii_alphanumeric() || prev == b'_' {
                i += 1;
                continue;
            }
            let mut we = i;
            while we < end && (t[we].is_ascii_alphanumeric() || t[we] == b'_') {
                we += 1;
            }
            if let Some(mut r) = el::parse_el(text, i).filter(|r| r.bean == text[i..we]) {
                let last = r.chain.iter().rposition(|s| !s.element).map_or(0, |p| p + 1);
                r.target = last;
                r.member = last.checked_sub(1).map(|k| r.chain[k].name.clone());
                r.kind = match last.checked_sub(1).map(|k| &r.chain[k]) {
                    None => RefKind::Bean,
                    Some(s) if s.call => RefKind::Method,
                    Some(_) => RefKind::Property,
                };
                // span of the chain: identifier plus the tail of `.x[...]()` characters
                let mut e = we;
                while e < end && (t[e].is_ascii_alphanumeric() || matches!(t[e], b'_' | b'.' | b'[' | b']' | b'(' | b')' | b'\'' | b'"')) {
                    e += 1;
                }
                if seen.insert((i, e)) {
                    out.push(ElChain { r, start: i, end: e });
                }
            }
            i = we.max(i + 1);
        }
    }
    out
}

const EL_KEYWORDS: &[&str] = &["true", "false", "null", "empty", "not", "and", "or", "eq", "ne", "lt", "gt", "le", "ge", "div", "mod", "instanceof", "new"];

/// How many chains one run resolves at most (each costs several jdtls round-trips).
pub const MAX_EL_CHECKS: usize = 300;

/// EL chains that don't resolve. Only heads that are beans or variables of THIS page are
/// checked (a `ui:param` from a template client is unknowable here), and only "không tìm thấy"
/// counts - inference gaps (generics, jdtls hiccups) are not findings.
pub async fn el_checks(ctx: &mut Ctx<'_>, index: &BeanIndex, text: &str, file: &std::path::Path) -> Vec<Diag> {
    let mut out = Vec::new();
    let mut cache: HashMap<String, Option<String>> = HashMap::new();
    let mut n = 0;
    for c in el_chains(text) {
        let head = c.r.bean.as_str();
        if EL_KEYWORDS.contains(&head) || IMPLICIT_OBJECTS.contains(&head) || n >= MAX_EL_CHECKS {
            continue;
        }
        let binding = el::find_var_binding(text, c.start, head);
        if index.pick(head, file).is_none() && binding.is_none() {
            continue;
        }
        n += 1;
        // a page variable's type depends on where it is: key by position then
        let key = if binding.is_some() { format!("{}@{}", &text[c.start..c.end], c.start) } else { text[c.start..c.end].to_string() };
        let msg = match cache.get(&key) {
            Some(m) => m.clone(),
            None => {
                let m = match ctx.resolve(&c.r, c.start).await {
                    Ok(_) => None,
                    Err(e) => Some(format!("{e:#}")),
                };
                cache.insert(key, m.clone());
                m
            }
        };
        if let Some(m) = msg.filter(|m| m.contains("không tìm thấy")) {
            out.push(Diag { start: c.start, end: c.end, severity: ERROR, message: format!("Cannot resolve: {m}"), code: "el-unresolved" });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_inspections() {
        let page = r##"<h:form id="f">
  <p:commandButton update="missing @all f @foo @(.x)" process="@this"/>
  <c:forEach items="#{b.items}" var="i"/>
  <p:dataTable paginator="true" value="#{b.rows}"/>
  <p:dataTable paginator="true" lazy="true" value="#{b.rows}"/>
</h:form>"##;
        let d = static_checks(page);
        let got: Vec<(&str, &str)> = d.iter().map(|d| (d.code, &page[d.start..d.end])).collect();
        assert_eq!(got, [("ajax-id", "missing"), ("ajax-all", "@all"), ("ajax-keyword", "@foo"), ("jstl-build-time", "c:forEach"), ("datatable-lazy", "p:dataTable")]);
        // a page that includes others: unknown ids are not flagged
        assert!(static_checks(r#"<ui:include src="x.xhtml"/><p:ajax update="elsewhere"/>"#).is_empty());
    }

    #[test]
    fn chains_of_a_page() {
        let page = r##"<h:outputText value="#{userBean.name} #{userBean.address.city and not empty cart.items[0].sku}" rendered="#{x.ok('a.b')}"/>"##;
        let got: Vec<(String, Option<String>, &str)> = el_chains(page).into_iter().map(|c| (c.r.bean.clone(), c.r.member.clone(), &page[c.start..c.end])).collect();
        assert_eq!(
            got,
            [
                ("userBean".into(), Some("name".into()), "userBean.name"),
                ("userBean".into(), Some("city".into()), "userBean.address.city"),
                ("cart".into(), Some("sku".into()), "cart.items[0].sku"),
                ("x".into(), Some("ok".into()), "x.ok('a.b')"),
            ]
        );
    }
}

#[cfg(test)]
mod fixture_tests {
    #[test]
    fn sample_page_terminates() {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../java-debug-model/test-fixtures/sample-project/module-a/src/main/webapp/hello.xhtml");
        let Ok(page) = std::fs::read_to_string(&p) else { return };
        let s = super::static_checks(&page);
        let c = super::el_chains(&page);
        eprintln!("static {} chains {}", s.len(), c.len());
    }
}
