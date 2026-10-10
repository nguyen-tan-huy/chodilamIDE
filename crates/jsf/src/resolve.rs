//! Type-aware resolution of an EL chain to the Java member it names, driven by jdtls (port of
//! jsf/resolve.lua): head = bean class (bean index) or a page variable's expression type;
//! each segment = documentSymbol of the current type (walking supertypes for inherited
//! members), then the member's declared type = textDocument/definition on its type token.
use crate::beans::BeanIndex;
use crate::el::{self, ElRef, Segment};
use anyhow::{Result, anyhow, bail};
use lsp_client::Client;
use lsp_client::text::{byte_to_utf16_col, utf16_col_to_byte};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::Path;
use std::pin::Pin;

pub const CLASS: u64 = 5;
pub const METHOD: u64 = 6;
pub const PROPERTY: u64 = 7;
pub const FIELD: u64 = 8;
pub const ENUM: u64 = 10;
pub const INTERFACE: u64 = 11;
pub const ENUM_MEMBER: u64 = 22;

#[derive(Debug, Clone)]
pub struct Sym {
    /// bare name ("getName" for "getName()")
    pub name: String,
    pub raw_name: String,
    pub kind: u64,
    pub range: Value,
    /// (line, utf-16 col) of the name
    pub sel: (usize, usize),
    pub sel_range: Value,
    pub container: Option<String>,
}

fn bare(name: &str) -> String {
    name.chars().take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$').collect()
}

pub fn flatten_symbols(result: &Value) -> Vec<Sym> {
    fn walk(arr: &Value, container: Option<String>, out: &mut Vec<Sym>) {
        for s in arr.as_array().into_iter().flatten() {
            let range = if s["range"].is_object() { s["range"].clone() } else { s["location"]["range"].clone() };
            let sel_range = if s["selectionRange"].is_object() { s["selectionRange"].clone() } else { range.clone() };
            let raw = s["name"].as_str().unwrap_or("").to_string();
            let name = bare(&raw);
            out.push(Sym {
                name: name.clone(),
                raw_name: raw,
                kind: s["kind"].as_u64().unwrap_or(0),
                sel: (sel_range["start"]["line"].as_u64().unwrap_or(0) as usize, sel_range["start"]["character"].as_u64().unwrap_or(0) as usize),
                sel_range,
                range,
                container: container.clone().or_else(|| s["containerName"].as_str().map(bare)),
            });
            if s["children"].is_array() {
                walk(&s["children"], Some(name), out);
            }
        }
    }
    let mut out = Vec::new();
    walk(result, None, &mut out);
    out
}

fn is_class_kind(k: u64) -> bool {
    matches!(k, CLASS | ENUM | INTERFACE)
}

pub fn class_symbol<'a>(syms: &'a [Sym], class: &str) -> Option<&'a Sym> {
    syms.iter().find(|s| is_class_kind(s.kind) && s.name == class)
}

/// Candidate member names for one segment, in lookup order.
fn candidates(seg: &Segment) -> Vec<(String, &'static [u64])> {
    const M: &[u64] = &[METHOD];
    const F: &[u64] = &[FIELD, PROPERTY, ENUM_MEMBER];
    if seg.call {
        return vec![(seg.name.clone(), M)];
    }
    let cap = el::capitalize(&seg.name);
    vec![(format!("get{cap}"), M), (format!("is{cap}"), M), (seg.name.clone(), F), (seg.name.clone(), M), (format!("set{cap}"), M)]
}

pub fn pick_symbol<'a>(syms: &'a [Sym], seg: &Segment, class: &str) -> Option<&'a Sym> {
    let has_class = class_symbol(syms, class).is_some();
    for (name, kinds) in candidates(seg) {
        if let Some(s) = syms.iter().find(|s| s.name == name && kinds.contains(&s.kind) && (!has_class || s.container.as_deref() == Some(class))) {
            return Some(s);
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeRef {
    pub uri: String,
    pub class_name: String,
}

#[derive(Debug, Clone)]
pub struct Found {
    pub uri: String,
    pub sym: Sym,
    pub owner: String,
}

/// Where Ctrl+B lands.
#[derive(Debug, Clone)]
pub enum Target {
    /// a Location (uri + range) in Java
    Java(Value),
    /// offset of a page variable's declaration in the page itself
    PageVar(usize),
}

/// Per-request state: the page, the bean index and caches of files/symbols fetched from jdtls.
pub struct Ctx<'a> {
    pub client: &'a Client,
    pub index: &'a BeanIndex,
    pub page: &'a str,
    pub file: &'a Path,
    texts: HashMap<String, String>,
    syms: HashMap<String, Vec<Sym>>,
}

type BoxFut<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + 'a>>;

impl<'a> Ctx<'a> {
    pub fn new(client: &'a Client, index: &'a BeanIndex, page: &'a str, file: &'a Path) -> Self {
        Self { client, index, page, file, texts: HashMap::new(), syms: HashMap::new() }
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value> {
        self.client.request(method, params).response().await
    }

    /// Text of a file:// or jdt:// (class file) uri.
    pub async fn text(&mut self, uri: &str) -> Result<String> {
        if let Some(t) = self.texts.get(uri) {
            return Ok(t.clone());
        }
        let text = if uri.starts_with("file:") {
            let p = lsp_client::uri_to_path(uri).ok_or_else(|| anyhow!("uri lạ: {uri}"))?;
            std::fs::read_to_string(p)?
        } else {
            self.request("java/classFileContents", json!({ "uri": uri })).await?.as_str().unwrap_or("").to_string()
        };
        self.texts.insert(uri.to_string(), text.clone());
        Ok(text)
    }

    async fn line(&mut self, uri: &str, line: usize) -> Result<String> {
        Ok(self.text(uri).await?.lines().nth(line).unwrap_or("").to_string())
    }

    pub async fn symbols(&mut self, uri: &str) -> Result<Vec<Sym>> {
        if let Some(s) = self.syms.get(uri) {
            return Ok(s.clone());
        }
        let r = self.request("textDocument/documentSymbol", json!({ "textDocument": { "uri": uri } })).await?;
        let s = flatten_symbols(&r);
        self.syms.insert(uri.to_string(), s.clone());
        Ok(s)
    }

    async fn definition(&self, uri: &str, line: usize, character: usize) -> Result<Option<String>> {
        let r = self.request("textDocument/definition", json!({ "textDocument": { "uri": uri }, "position": { "line": line, "character": character } })).await?;
        let loc = if r.is_array() { r[0].clone() } else { r };
        Ok(loc["uri"].as_str().or(loc["targetUri"].as_str()).map(String::from))
    }

    /// Direct supertypes (java.lang.Object excluded).
    async fn supertypes(&mut self, t: &TypeRef, syms: &[Sym]) -> Result<Vec<TypeRef>> {
        let Some(cls) = class_symbol(syms, &t.class_name).cloned() else { return Ok(vec![]) };
        let mut out = Vec::new();
        let items = self
            .request("textDocument/prepareTypeHierarchy", json!({ "textDocument": { "uri": t.uri }, "position": { "line": cls.sel.0, "character": cls.sel.1 } }))
            .await
            .unwrap_or(Value::Null);
        if let Some(item) = items.as_array().and_then(|a| a.first()) {
            let supers = self.request("typeHierarchy/supertypes", json!({ "item": item })).await.unwrap_or(Value::Null);
            for s in supers.as_array().into_iter().flatten() {
                let name = bare(s["name"].as_str().unwrap_or(""));
                if name != "Object" && !name.is_empty() {
                    out.push(TypeRef { uri: s["uri"].as_str().unwrap_or("").to_string(), class_name: name });
                }
            }
        }
        if !out.is_empty() {
            return Ok(out);
        }
        self.declared_supertypes(t, &cls).await
    }

    /// Fallback: the `extends`/`implements` clause read from the source.
    async fn declared_supertypes(&mut self, t: &TypeRef, cls: &Sym) -> Result<Vec<TypeRef>> {
        let text = self.text(&t.uri).await?;
        let mut out = Vec::new();
        let mut active = false;
        for (row, line) in text.lines().enumerate().skip(cls.sel.0).take(15) {
            let head = line.split('{').next().unwrap_or(line);
            // blank out generics, keep offsets
            let mut clean = String::with_capacity(head.len());
            let mut depth = 0;
            for ch in head.chars() {
                match ch {
                    '<' => depth += 1,
                    '>' => depth -= 1,
                    _ => {}
                }
                clean.push(if depth > 0 || ch == '>' { ' ' } else { ch });
            }
            let mut i = 0;
            let b = clean.as_bytes();
            while i < b.len() {
                if b[i].is_ascii_alphabetic() || b[i] == b'_' {
                    let s = i;
                    while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'.') {
                        i += 1;
                    }
                    let word = &clean[s..i];
                    if word == "extends" || word == "implements" {
                        active = true;
                    } else if active {
                        let simple = word.rsplit('.').next().unwrap_or(word);
                        let col = byte_to_utf16_col(line, i - simple.len());
                        if simple != "Object" {
                            if let Ok(Some(uri)) = self.definition(&t.uri, row, col).await {
                                out.push(TypeRef { uri, class_name: simple.to_string() });
                            }
                        }
                    }
                } else {
                    i += 1;
                }
            }
            if line.contains('{') {
                break;
            }
        }
        Ok(out)
    }

    /// `seg` on type `t`, walking up the supertypes for inherited members.
    pub async fn find_member(&mut self, t: &TypeRef, seg: &Segment) -> Result<Option<Found>> {
        let mut queue = vec![t.clone()];
        let mut seen = HashSet::new();
        while let Some(t) = (!queue.is_empty()).then(|| queue.remove(0)) {
            if !seen.insert(t.uri.clone()) || seen.len() > 12 {
                continue;
            }
            let syms = self.symbols(&t.uri).await?;
            if let Some(s) = pick_symbol(&syms, seg, &t.class_name) {
                return Ok(Some(Found { uri: t.uri.clone(), sym: s.clone(), owner: t.class_name.clone() }));
            }
            queue.extend(self.supertypes(&t, &syms).await?);
        }
        Ok(None)
    }

    /// Declared type of a member (method return / field type) or, with `element`, its element type.
    pub async fn member_type(&mut self, f: &Found, element: bool) -> Result<TypeRef> {
        let line = self.line(&f.uri, f.sym.sel.0).await?;
        let name_byte = utf16_col_to_byte(&line, f.sym.sel.1);
        let (ts, te) = el::type_token(&line[..name_byte], element)
            .ok_or_else(|| anyhow!("không suy ra được {} của '{}'", if element { "kiểu phần tử" } else { "kiểu" }, f.sym.name))?;
        let uri = self
            .definition(&f.uri, f.sym.sel.0, byte_to_utf16_col(&line, ts))
            .await?
            .ok_or_else(|| anyhow!("không tìm thấy class '{}' (kiểu của '{}')", &line[ts..te], f.sym.name))?;
        Ok(TypeRef { uri, class_name: line[ts..te].to_string() })
    }

    /// Applies segments 0..n from `t`: returns the last found member (type not computed yet) or
    /// the resulting type when the walk ended on an `[index]`.
    async fn walk(&mut self, mut t: TypeRef, segs: &[Segment], n: usize) -> Result<(Option<Found>, TypeRef)> {
        let mut prev: Option<Found> = None;
        for seg in &segs[..n] {
            if seg.element {
                let p = prev.take().ok_or_else(|| anyhow!("không index được biểu thức ở vị trí này"))?;
                t = self.member_type(&p, true).await?;
            } else {
                if let Some(p) = prev.take() {
                    t = self.member_type(&p, false).await?;
                }
                prev = Some(self.find_member(&t, seg).await?.ok_or_else(|| anyhow!("không tìm thấy '{}' trong {}", seg.name, t.class_name))?);
            }
        }
        Ok((prev, t))
    }

    async fn type_after(&mut self, t: TypeRef, segs: &[Segment], n: usize, element: bool) -> Result<TypeRef> {
        match self.walk(t, segs, n).await? {
            (Some(p), _) => self.member_type(&p, element).await,
            (None, _) if element => bail!("không suy ra được kiểu phần tử"),
            (None, t) => Ok(t),
        }
    }

    /// Type of the head identifier: a bean, or a page variable (its expression's type).
    fn head_type<'b>(&'b mut self, name: &'b str, pos: usize, depth: usize) -> BoxFut<'b, (TypeRef, Option<el::Binding>)> {
        Box::pin(async move {
            if let Some(b) = self.index.pick(name, self.file) {
                return Ok((TypeRef { uri: lsp_client::path_to_uri(&b.file), class_name: b.class_name.clone() }, None));
            }
            if el::IMPLICIT_OBJECTS.contains(&name) {
                bail!("'{name}' là implicit object của EL, không trỏ tới class Java của project");
            }
            let Some(binding) = el::find_var_binding(self.page, pos, name) else {
                bail!("không tìm thấy bean '{name}' (@Named/@ManagedBean/@Component) hay biến var=\"{name}\" trong trang");
            };
            if depth > 8 {
                bail!("biến '{name}' lồng nhau quá sâu");
            }
            let parsed = el::parse_expression(&binding.expr).ok_or_else(|| anyhow!("không đọc được biểu thức gán cho '{name}': {}", binding.expr))?;
            let (t, _) = self.head_type(&parsed.bean, binding.decl, depth + 1).await?;
            let t = self.type_after(t, &parsed.chain, parsed.chain.len(), binding.iterate).await?;
            Ok((t, Some(binding)))
        })
    }

    /// Type of the whole chain (what `.` completes on).
    pub async fn type_of(&mut self, r: &ElRef, pos: usize) -> Result<TypeRef> {
        let (t, _) = self.head_type(&r.bean, pos, 0).await?;
        self.type_after(t, &r.chain, r.chain.len(), false).await
    }

    /// Where Ctrl+B on the segment under the cursor lands.
    pub async fn resolve(&mut self, r: &ElRef, pos: usize) -> Result<Target> {
        let (t, binding) = self.head_type(&r.bean, pos, 0).await?;
        if r.target == 0 {
            if let Some(b) = binding {
                return Ok(Target::PageVar(b.decl));
            }
            let syms = self.symbols(&t.uri).await?;
            let range = class_symbol(&syms, &t.class_name).map(|s| s.sel_range.clone()).unwrap_or(json!({ "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } }));
            return Ok(Target::Java(json!({ "uri": t.uri, "range": range })));
        }
        let owner = self.type_after(t, &r.chain, r.target - 1, false).await?;
        let seg = &r.chain[r.target - 1];
        let f = self.find_member(&owner, seg).await?.ok_or_else(|| anyhow!("không tìm thấy '{}' trong {}", seg.name, owner.class_name))?;
        Ok(Target::Java(json!({ "uri": f.uri, "range": f.sym.sel_range })))
    }

    /// Hover text for the segment under the cursor: the Java declaration line.
    pub async fn hover(&mut self, r: &ElRef, pos: usize) -> Result<String> {
        match self.resolve(r, pos).await? {
            Target::PageVar(decl) => {
                let b = el::find_var_binding(self.page, pos, &r.bean).ok_or_else(|| anyhow!("?"))?;
                let _ = decl;
                Ok(format!("<{} var=\"{}\">  {} {}", b.tag, r.bean, if b.iterate { "phần tử của" } else { "=" }, b.expr))
            }
            Target::Java(loc) => {
                let uri = loc["uri"].as_str().unwrap_or("").to_string();
                let line = loc["range"]["start"]["line"].as_u64().unwrap_or(0) as usize;
                Ok(self.line(&uri, line).await?.trim().trim_end_matches('{').trim().to_string())
            }
        }
    }

    /// Every member of `t` and its supertypes, own class first: (sym, owner, depth, decl line).
    pub async fn members_of(&mut self, t: &TypeRef) -> Result<Vec<(Sym, String, usize, String)>> {
        let mut out = Vec::new();
        let mut queue = vec![(t.clone(), 0usize)];
        let mut seen = HashSet::new();
        while let Some((t, depth)) = (!queue.is_empty()).then(|| queue.remove(0)) {
            if !seen.insert(t.uri.clone()) || depth > 10 {
                continue;
            }
            let syms = self.symbols(&t.uri).await?;
            let text = self.text(&t.uri).await?;
            let lines: Vec<&str> = text.lines().collect();
            for s in syms.iter().filter(|s| matches!(s.kind, METHOD | FIELD | PROPERTY | ENUM_MEMBER) && s.container.as_deref() == Some(t.class_name.as_str())) {
                out.push((s.clone(), t.class_name.clone(), depth, lines.get(s.sel.0).copied().unwrap_or("").to_string()));
            }
            let lombok = text.lines().take(80).any(|l| l.contains("lombok"));
            if lombok {
                // marker: fields of a Lombok class are properties
                out.push((Sym { name: String::new(), raw_name: "@lombok".into(), kind: 0, range: Value::Null, sel: (0, 0), sel_range: Value::Null, container: None }, t.class_name.clone(), depth, String::new()));
            }
            for s in self.supertypes(&t, &syms).await? {
                queue.push((s, depth + 1));
            }
        }
        Ok(out)
    }
}

const MODIFIERS: &[&str] = &["public", "protected", "private", "static", "final", "abstract", "synchronized", "default", "native", "transient", "volatile", "strictfp"];

/// (modifiers present, declared type) from a declaration line up to the name.
fn decl_info(line: &str, name_col16: usize) -> (HashSet<String>, String) {
    let prefix = &line[..utf16_col_to_byte(line, name_col16)];
    let words: HashSet<String> = prefix.split(|c: char| !c.is_alphanumeric() && c != '_').filter(|w| !w.is_empty()).map(String::from).collect();
    // drop annotations and modifiers
    let mut ty = String::new();
    let mut skip_ann = false;
    for tok in prefix.split_whitespace() {
        if tok.starts_with('@') {
            skip_ann = tok.contains('(') && !tok.contains(')');
            continue;
        }
        if skip_ann {
            skip_ann = !tok.contains(')');
            continue;
        }
        if MODIFIERS.contains(&tok) {
            continue;
        }
        if !ty.is_empty() {
            ty.push(' ');
        }
        ty.push_str(tok);
    }
    (words, ty)
}

/// EL-visible members → LSP-style completion items: properties (getX/isX, Lombok fields) first,
/// then public non-accessor methods; setters left out; a name once (subclass wins).
pub fn member_items(members: &[(Sym, String, usize, String)]) -> Vec<Value> {
    let lombok: HashSet<&str> = members.iter().filter(|m| m.0.raw_name == "@lombok").map(|m| m.1.as_str()).collect();
    let mut items = Vec::new();
    let mut seen = HashSet::new();
    let mut add = |label: String, kind: u64, detail: String, group: u8, owner: &str, depth: usize, insert: String| {
        if !seen.insert(label.clone()) {
            return;
        }
        items.push(json!({
            "label": label, "kind": kind, "detail": detail,
            "labelDetails": { "description": owner },
            "insertText": insert, "filterText": insert,
            "sortText": format!("{group}{depth:02}{label}"),
        }));
    };
    for (s, owner, depth, line) in members {
        if s.raw_name == "@lombok" {
            continue;
        }
        let (mods, ty) = decl_info(line, s.sel.1);
        if s.kind == METHOD && !mods.contains("private") && !mods.contains("protected") && !mods.contains("static") {
            let params = s.raw_name.split_once('(').map(|x| x.1.trim_end_matches(')').to_string());
            let (prop, accessor) = el::property_name_for(&s.name);
            if accessor && !s.name.starts_with("set") && params.as_deref().is_none_or(str::is_empty) {
                add(prop.clone(), 10, ty, 0, owner, *depth, prop);
            } else if !accessor {
                add(format!("{}({})", s.name, params.unwrap_or_default()), 2, ty, 1, owner, *depth, s.name.clone());
            }
        } else if matches!(s.kind, FIELD | PROPERTY) && !mods.contains("static") && lombok.contains(owner.as_str()) {
            add(s.name.clone(), 10, ty, 0, owner, *depth, s.name.clone());
        }
    }
    items
}

/// Beans, page variables in scope, implicit objects.
pub fn head_items(index: &BeanIndex, page: &str, pos: usize) -> Vec<Value> {
    let mut items = Vec::new();
    for (name, b) in el::page_variables(page, pos) {
        items.push(json!({ "label": name, "kind": 6, "detail": format!("{} {}", if b.iterate { "phần tử của" } else { "=" }, b.expr),
            "labelDetails": { "description": b.tag }, "sortText": format!("0{name}") }));
    }
    for b in index.beans() {
        items.push(json!({ "label": b.name, "kind": 7, "detail": b.fqcn, "labelDetails": { "description": "bean" }, "sortText": format!("1{}", b.name) }));
    }
    for name in el::IMPLICIT_OBJECTS {
        items.push(json!({ "label": name, "kind": 14, "detail": "EL implicit object", "sortText": format!("2{name}") }));
    }
    items
}

/// Innermost method/field whose range contains (line, col) → its bare name.
pub fn member_at(syms: &[Sym], line: usize, col: usize) -> Option<String> {
    let pos = |v: &Value| (v["line"].as_u64().unwrap_or(0) as usize, v["character"].as_u64().unwrap_or(0) as usize);
    syms.iter()
        .filter(|s| matches!(s.kind, METHOD | FIELD | PROPERTY) && pos(&s.range["start"]) <= (line, col) && (line, col) <= pos(&s.range["end"]))
        .max_by_key(|s| pos(&s.range["start"]))
        .map(|s| s.name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbols_and_items() {
        let r = json!([{ "name": "HelloBean", "kind": 5, "range": {"start":{"line":3,"character":0},"end":{"line":40,"character":1}},
            "selectionRange": {"start":{"line":3,"character":13},"end":{"line":3,"character":22}},
            "children": [
              { "name": "getCustomer()", "kind": 6, "range": {"start":{"line":10,"character":2},"end":{"line":12,"character":3}}, "selectionRange": {"start":{"line":10,"character":18},"end":{"line":10,"character":29}} },
              { "name": "save(Long)", "kind": 6, "range": {"start":{"line":14,"character":2},"end":{"line":16,"character":3}}, "selectionRange": {"start":{"line":14,"character":14},"end":{"line":14,"character":18}} },
              { "name": "setCustomer(Customer)", "kind": 6, "range": {"start":{"line":18,"character":2},"end":{"line":18,"character":40}}, "selectionRange": {"start":{"line":18,"character":14},"end":{"line":18,"character":25}} },
              { "name": "helper()", "kind": 6, "range": {"start":{"line":20,"character":2},"end":{"line":20,"character":40}}, "selectionRange": {"start":{"line":20,"character":15},"end":{"line":20,"character":21}} }
            ]}]);
        let syms = flatten_symbols(&r);
        assert_eq!(syms[1].container.as_deref(), Some("HelloBean"));
        let seg = Segment { name: "customer".into(), ..Default::default() };
        assert_eq!(pick_symbol(&syms, &seg, "HelloBean").unwrap().name, "getCustomer");
        let seg = Segment { name: "save".into(), call: true, ..Default::default() };
        assert_eq!(pick_symbol(&syms, &seg, "HelloBean").unwrap().name, "save");
        let members: Vec<_> = vec![
            (syms[1].clone(), "HelloBean".to_string(), 0, "  public Customer getCustomer() {".to_string()),
            (syms[2].clone(), "HelloBean".to_string(), 0, "  public void save(Long id) {".to_string()),
            (syms[3].clone(), "HelloBean".to_string(), 0, "  public void setCustomer(Customer c) {}".to_string()),
            (syms[4].clone(), "HelloBean".to_string(), 0, "  private int helper() {}".to_string()),
        ];
        let items = member_items(&members);
        let labels: Vec<&str> = items.iter().map(|i| i["label"].as_str().unwrap()).collect();
        assert_eq!(labels, vec!["customer", "save(Long)"]);
        assert_eq!(items[0]["detail"], "Customer");
        assert_eq!(member_at(&syms, 15, 4).as_deref(), Some("save"));
        assert_eq!(member_at(&syms, 5, 0), None);
    }
}
