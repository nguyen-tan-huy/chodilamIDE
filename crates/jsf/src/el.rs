//! Pure JSF/Facelets text parsing (port of java-debug-model's jsf/el.lua): EL expressions,
//! start tags, include paths, composite components, page variables and the completion context.
//! Everything works on the whole document text and 0-based byte offsets, so expressions and
//! tags split over several lines work like single-line ones.
use std::collections::BTreeMap;

const KEYWORDS: &[&str] = &["empty", "not", "and", "or", "eq", "ne", "lt", "gt", "le", "ge", "div", "mod", "true", "false", "null", "instanceof", "new"];

/// EL implicit objects - never beans, never a class of the project.
pub const IMPLICIT_OBJECTS: &[&str] = &[
    "cc", "param", "paramValues", "header", "headerValues", "cookie", "initParam", "request", "session", "application", "flash", "facesContext", "view",
    "component", "resource", "requestScope", "sessionScope", "applicationScope", "viewScope", "flowScope",
];

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}
fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// End (exclusive) of the identifier starting at `s`.
fn ident_end(t: &[u8], s: usize) -> usize {
    let mut e = s;
    while e < t.len() && is_ident(t[e]) {
        e += 1;
    }
    e
}

fn skip_ws(t: &[u8], mut i: usize, limit: usize) -> usize {
    while i < limit && t[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

/// `#{...}` / `${...}` span: `start` = the `#`, `body` = after the `{`, `end` = the `}` (all 0-based).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Span {
    pub start: usize,
    pub body: usize,
    pub end: usize,
}

/// Every EL span; quote-aware so `#{bean.label('}')}` isn't cut early.
pub fn el_spans(text: &str) -> Vec<Span> {
    let t = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(s) = find_el_open(t, i) {
        match close_of(t, s + 2) {
            Some(end) => {
                out.push(Span { start: s, body: s + 2, end });
                i = end + 1;
            }
            None => break,
        }
    }
    out
}

fn find_el_open(t: &[u8], from: usize) -> Option<usize> {
    (from..t.len().saturating_sub(1)).find(|&i| (t[i] == b'#' || t[i] == b'$') && t[i + 1] == b'{')
}

/// Offset of the `}` closing an expression whose body starts at `from`.
fn close_of(t: &[u8], from: usize) -> Option<usize> {
    let mut quote = None;
    for (j, &ch) in t.iter().enumerate().skip(from) {
        match quote {
            Some(q) if ch == q => quote = None,
            Some(_) => {}
            None if ch == b'\'' || ch == b'"' => quote = Some(ch),
            None if ch == b'}' => return Some(j),
            None => {}
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Segment {
    /// empty for an `[index]` element access
    pub name: String,
    pub call: bool,
    pub bracket: bool,
    pub element: bool,
    pub(crate) s: usize,
}

#[derive(Debug, Clone)]
struct Chain {
    head: String,
    head_end: usize,
    segments: Vec<Segment>,
    pub(crate) s: usize,
    e: usize,
}

/// Matching `close` for the `open` at `i` (balanced, like Lua's %b()), before `limit`.
fn balanced(t: &[u8], i: usize, open: u8, close: u8, limit: usize) -> Option<usize> {
    let mut depth = 0;
    for (j, &ch) in t.iter().enumerate().take(limit).skip(i) {
        if ch == open {
            depth += 1;
        } else if ch == close {
            depth -= 1;
            if depth == 0 {
                return Some(j);
            }
        }
    }
    None
}

fn parse_chain(t: &[u8], s: usize, limit: usize, out: &mut Vec<Chain>) -> (Chain, usize) {
    let he = ident_end(t, s);
    let mut chain = Chain { head: String::from_utf8_lossy(&t[s..he]).into(), head_end: he, segments: Vec::new(), s, e: he };
    let mut pos = he;
    while pos < limit {
        let d = skip_ws(t, pos, limit);
        if d < limit && t[d] == b'.' {
            let ns = skip_ws(t, d + 1, limit);
            if ns < limit && is_ident_start(t[ns]) {
                let ne = ident_end(t, ns).min(limit);
                let mut seg = Segment { name: String::from_utf8_lossy(&t[ns..ne]).into(), s: ns, ..Default::default() };
                pos = ne;
                let p = skip_ws(t, pos, limit);
                if p < limit && t[p] == b'(' {
                    if let Some(pe) = balanced(t, p, b'(', b')', limit) {
                        seg.call = true;
                        scan(t, p + 1, pe, out);
                        pos = pe + 1;
                    }
                }
                chain.segments.push(seg);
                continue;
            }
            break;
        }
        if d < limit && t[d] == b'[' {
            let Some(be) = balanced(t, d, b'[', b']', limit) else { break };
            let inner = &t[d + 1..be];
            let trimmed_start = inner.iter().position(|c| !c.is_ascii_whitespace()).unwrap_or(inner.len());
            let key = quoted_key(&inner[trimmed_start..]);
            match key {
                Some(k) => {
                    let ks = d + 1 + trimmed_start + 1;
                    chain.segments.push(Segment { name: k, bracket: true, s: ks, ..Default::default() });
                }
                None => {
                    chain.segments.push(Segment { element: true, s: d, ..Default::default() });
                    scan(t, d + 1, be, out);
                }
            }
            pos = be + 1;
            continue;
        }
        break;
    }
    chain.e = pos;
    (chain, pos)
}

/// `'name'` / `"name"` (plus trailing spaces) → name.
fn quoted_key(s: &[u8]) -> Option<String> {
    let q = *s.first()?;
    if q != b'\'' && q != b'"' {
        return None;
    }
    let close = s[1..].iter().position(|&c| c == q)? + 1;
    let key = &s[1..close];
    if key.is_empty() || !is_ident_start(key[0]) || !key.iter().all(|&c| is_ident(c)) || !s[close + 1..].iter().all(|c| c.is_ascii_whitespace()) {
        return None;
    }
    Some(String::from_utf8_lossy(key).into())
}

fn scan(t: &[u8], mut pos: usize, limit: usize, out: &mut Vec<Chain>) {
    while pos < limit {
        let c = t[pos];
        if c == b'\'' || c == b'"' {
            let close = t[pos + 1..limit].iter().position(|&x| x == c).map(|p| pos + 1 + p).unwrap_or(limit);
            pos = close + 1;
        } else if is_ident_start(c) {
            let e = ident_end(t, pos);
            let word = std::str::from_utf8(&t[pos..e]).unwrap_or("");
            let prev = if pos > 0 { t[pos - 1] } else { b' ' };
            let prev2 = if pos > 1 { t[pos - 2] } else { b' ' };
            let fn_prefix = e + 1 < t.len() && t[e] == b':' && is_ident_start(t[e + 1]);
            let fn_name = prev == b':' && is_ident(prev2);
            if is_ident(prev) || prev == b'.' || KEYWORDS.contains(&word) || fn_prefix || fn_name {
                pos = e;
            } else {
                let (chain, np) = parse_chain(t, pos, limit, out);
                out.push(chain);
                pos = np.max(pos + 1);
            }
        } else {
            pos += 1;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RefKind {
    Bean,
    Method,
    Property,
}

/// An EL reference: `bean` = head identifier, `chain` = segments after it, `target` = index
/// (1-based into chain, 0 = head) of the segment the cursor is on.
#[derive(Debug, Clone, PartialEq)]
pub struct ElRef {
    pub bean: String,
    pub chain: Vec<Segment>,
    pub target: usize,
    pub kind: RefKind,
    pub member: Option<String>,
}

/// The EL reference at `pos`, or None outside EL. Inside a larger expression the innermost chain
/// under the cursor wins; on an operator/whitespace, the first chain's last segment.
pub fn parse_el(text: &str, pos: usize) -> Option<ElRef> {
    let span = el_spans(text).into_iter().find(|sp| pos >= sp.start && pos <= sp.end)?;
    let t = text.as_bytes();
    let mut chains = Vec::new();
    scan(t, span.body, span.end, &mut chains);
    if chains.is_empty() {
        return None;
    }
    let mut hit: Option<&Chain> = None;
    for ch in &chains {
        if pos >= ch.s && pos <= ch.e && hit.is_none_or(|h| ch.e - ch.s < h.e - h.s) {
            hit = Some(ch);
        }
    }
    let mut target = 0;
    let hit = match hit {
        Some(h) => {
            if pos > h.head_end {
                for (i, seg) in h.segments.iter().enumerate() {
                    if !seg.element && pos >= seg.s {
                        target = i + 1;
                    }
                }
            }
            h
        }
        None => {
            let h = &chains[0];
            for (i, seg) in h.segments.iter().enumerate() {
                if !seg.element {
                    target = i + 1;
                }
            }
            h
        }
    };
    let seg = target.checked_sub(1).and_then(|i| hit.segments.get(i));
    Some(ElRef {
        bean: hit.head.clone(),
        chain: hit.segments.clone(),
        target,
        kind: match seg {
            None => RefKind::Bean,
            Some(s) if s.call => RefKind::Method,
            Some(_) => RefKind::Property,
        },
        member: seg.map(|s| s.name.clone()),
    })
}

/// Reference of a standalone attribute value like "#{helloBean.items}" (target = last segment).
pub fn parse_expression(expr: &str) -> Option<ElRef> {
    let sp = *el_spans(expr).first()?;
    parse_el(expr, sp.end)
}

// ---- tags --------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Attr {
    pub value: String,
    /// value start/end (exclusive) offsets
    pub vs: usize,
    pub ve: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tag {
    pub name: String,
    /// `<` offset and the offset of the closing `>`
    pub s: usize,
    pub e: usize,
    pub self_closing: bool,
    pub attrs: BTreeMap<String, Attr>,
}

/// The start tag whose `<` is at `ts`; quote-aware, may span lines.
pub fn parse_tag(text: &str, ts: usize) -> Option<Tag> {
    let t = text.as_bytes();
    if t.get(ts) != Some(&b'<') || !t.get(ts + 1).is_some_and(|&c| is_ident_start(c)) {
        return None;
    }
    let mut ne = ts + 1;
    while ne < t.len() && (is_ident(t[ne]) || matches!(t[ne], b'-' | b'.' | b':')) {
        ne += 1;
    }
    let mut tag = Tag { name: text[ts + 1..ne].to_string(), s: ts, e: 0, self_closing: false, attrs: BTreeMap::new() };
    let mut pos = ne;
    loop {
        let ws = skip_ws(t, pos, t.len());
        if ws >= t.len() {
            return None;
        }
        if t[ws] == b'>' {
            tag.e = ws;
            return Some(tag);
        }
        if t[ws] == b'/' && t.get(ws + 1) == Some(&b'>') {
            tag.e = ws + 1;
            tag.self_closing = true;
            return Some(tag);
        }
        let mut ae = ws;
        while ae < t.len() && (is_ident(t[ae]) || matches!(t[ae], b':' | b'-' | b'.')) {
            ae += 1;
        }
        if ae == ws {
            return None;
        }
        let name = text[ws..ae].to_string();
        let eq = skip_ws(t, ae, t.len());
        if t.get(eq) != Some(&b'=') {
            return None;
        }
        let q = skip_ws(t, eq + 1, t.len());
        let quote = *t.get(q)?;
        if quote != b'"' && quote != b'\'' {
            return None;
        }
        let close = t[q + 1..].iter().position(|&c| c == quote)? + q + 1;
        tag.attrs.insert(name, Attr { value: text[q + 1..close].to_string(), vs: q + 1, ve: close });
        pos = close + 1;
    }
}

/// Every start tag beginning before `upto`, in document order.
pub fn tags(text: &str, upto: usize) -> Vec<Tag> {
    let t = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < t.len() && i < upto {
        if t[i] == b'<' && is_ident_start(t[i + 1]) {
            if let Some(tag) = parse_tag(text, i) {
                i = tag.e + 1;
                out.push(tag);
                continue;
            }
        }
        i += 1;
    }
    out
}

/// The start tag `pos` is inside (between `<` and `>`).
pub fn tag_at(text: &str, pos: usize) -> Option<Tag> {
    tags(text, pos + 1).into_iter().rfind(|t| pos >= t.s && pos <= t.e)
}

/// Tag → attribute holding a Facelets path.
fn include_attr(tag: &str) -> Option<&'static str> {
    match tag {
        "ui:include" => Some("src"),
        "ui:composition" | "ui:decorate" => Some("template"),
        "c:import" => Some("url"),
        _ => None,
    }
}

/// Path under the cursor in `src=` of ui:include, `template=` of ui:composition/decorate, `url=` of c:import.
pub fn parse_include_path(text: &str, pos: usize) -> Option<String> {
    let tag = tag_at(text, pos)?;
    let a = tag.attrs.get(include_attr(&tag.name)?)?;
    (pos >= a.vs && pos <= a.ve).then(|| a.value.clone())
}

/// `xmlns:prefix=".../jsf/composite/<lib>"` (or `jakarta.faces.composite/<lib>`) → prefix → lib.
pub fn composite_namespaces(text: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    let mut rest = text;
    while let Some(i) = rest.find("xmlns:") {
        rest = &rest[i + 6..];
        let pe = rest.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(rest.len());
        let prefix = &rest[..pe];
        let after = rest[pe..].trim_start();
        let Some(after) = after.strip_prefix('=') else { continue };
        let after = after.trim_start();
        let Some(q) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else { continue };
        let Some(end) = after[1..].find(q) else { continue };
        let uri = &after[1..1 + end];
        let lib = uri.split_once("/jsf/composite/").map(|x| x.1).or_else(|| uri.strip_prefix("jakarta.faces.composite/"));
        if let Some(lib) = lib {
            map.insert(prefix.to_string(), lib.to_string());
        }
    }
    map
}

/// `<prefix:tag` (cursor on the name) of a composite namespace → (library, tag).
pub fn parse_composite_tag(text: &str, pos: usize, namespaces: &BTreeMap<String, String>) -> Option<(String, String)> {
    let t = text.as_bytes();
    // walk back to the '<'
    let mut i = pos.min(t.len().saturating_sub(1));
    while i > 0 && (is_ident(t[i]) || matches!(t[i], b':' | b'-')) {
        i -= 1;
    }
    let mut s = i + 1;
    if t.get(i) == Some(&b'/') && i > 0 && t[i - 1] == b'<' {
    } else if t.get(i) != Some(&b'<') {
        return None;
    }
    if t.get(s) == Some(&b'/') {
        s += 1;
    }
    let mut e = s;
    while e < t.len() && (is_ident(t[e]) || matches!(t[e], b':' | b'-')) {
        e += 1;
    }
    if pos < s || pos >= e {
        return None;
    }
    let (prefix, tag) = text[s..e].split_once(':')?;
    namespaces.get(prefix).map(|lib| (lib.clone(), tag.to_string()))
}

// ---- page variables ------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Binding {
    pub tag: String,
    pub expr: String,
    /// iterating tag (ui:repeat, dataTable…): the variable is an ELEMENT of `expr`
    pub iterate: bool,
    /// offset of the declared name
    pub decl: usize,
}

/// The declaration in scope at `pos` for page variable `name` (var= of an iterating tag whose
/// body contains `pos`, c:set var=, ui:param name=); the nearest wins.
pub fn find_var_binding(text: &str, pos: usize, name: &str) -> Option<Binding> {
    let mut best = None;
    for tag in tags(text, pos) {
        let (decl, expr, iterate) = if tag.name == "ui:param" && tag.attrs.get("name").is_some_and(|a| a.value == name) {
            (tag.attrs.get("name"), tag.attrs.get("value"), false)
        } else if tag.attrs.get("var").is_some_and(|a| a.value == name) {
            (tag.attrs.get("var"), tag.attrs.get("value").or(tag.attrs.get("items")), tag.name != "c:set")
        } else {
            continue;
        };
        let (Some(decl), Some(expr)) = (decl, expr) else { continue };
        if tag.e >= pos {
            continue;
        }
        let in_scope = !iterate || (!tag.self_closing && !text[tag.e + 1..].find(&format!("</{}", tag.name)).is_some_and(|c| tag.e + 1 + c < pos));
        if in_scope {
            best = Some(Binding { tag: tag.name.clone(), expr: expr.value.clone(), iterate, decl: decl.vs });
        }
    }
    best
}

/// Page variables in scope at `pos`, in document order.
pub fn page_variables(text: &str, pos: usize) -> Vec<(String, Binding)> {
    let mut out: Vec<(String, Binding)> = Vec::new();
    for tag in tags(text, pos) {
        let name = if tag.name == "ui:param" { tag.attrs.get("name") } else { tag.attrs.get("var") };
        let Some(name) = name.map(|a| a.value.clone()) else { continue };
        if out.iter().any(|(n, _)| *n == name) {
            continue;
        }
        if let Some(b) = find_var_binding(text, pos, &name) {
            out.push((name, b));
        }
    }
    out
}

// ---- completion context ----------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum CompletionKind {
    /// after `chain.` / `chain['` - members of that chain's type
    Member(ElRef),
    /// start of a reference: beans, page variables, implicit objects
    Head,
    /// include/template path
    Include,
    /// `<prefix:` of a composite namespace
    Composite(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompletionContext {
    pub kind: CompletionKind,
    pub typed: String,
    /// offset where `typed` starts (the replaced range is typed_s..pos)
    pub typed_s: usize,
}

const MAX_OPEN_EL: usize = 2000;

fn open_el_start(before: &[u8]) -> Option<usize> {
    let mut i = 0;
    while let Some(s) = find_el_open(before, i) {
        match close_of(before, s + 2) {
            Some(f) => i = f + 1,
            None => return Some(s),
        }
    }
    None
}

/// What completion at `pos` should offer, or None when nothing JSF-specific applies.
pub fn completion_context(text: &str, pos: usize) -> Option<CompletionContext> {
    let before = &text[..pos.min(text.len())];
    let b = before.as_bytes();
    if let Some(el_start) = open_el_start(b).filter(|s| b.len() - s < MAX_OPEN_EL) {
        let body = &before[el_start + 2..];
        let typed_len = body.bytes().rev().take_while(|&c| is_ident(c)).count();
        // an identifier can't start with a digit
        let typed = &body[body.len() - typed_len..];
        let typed = typed.trim_start_matches(|c: char| c.is_ascii_digit());
        let rest = &body[..body.len() - typed.len()];
        let typed_s = pos - typed.len();
        let rt = rest.trim_end();
        let chain_text = if let Some(c) = rt.strip_suffix('.') {
            Some(c.trim_end())
        } else {
            let r = rest.trim_end_matches(|c: char| c.is_whitespace());
            r.strip_suffix(['\'', '"']).and_then(|x| x.trim_end().strip_suffix('[')).map(|x| x.trim_end())
        };
        if let Some(chain_text) = chain_text {
            let last = chain_text.bytes().last()?;
            if !(is_ident(last) || last == b')' || last == b']') {
                return None;
            }
            if chain_text.bytes().rev().take_while(|c| is_ident(*c)).all(|c| c.is_ascii_digit()) && is_ident(last) {
                return None; // "1." of a number
            }
            let synthetic = format!("#{{{chain_text}}}");
            let parsed = parse_el(&synthetic, synthetic.len() - 1)?;
            return Some(CompletionContext { kind: CompletionKind::Member(parsed), typed: typed.to_string(), typed_s });
        }
        let last = rest.bytes().last();
        if last.is_some_and(|c| is_ident(c) || c == b'\'' || c == b'"') || rest.ends_with(':') && rest.len() > 1 && is_ident(rest.as_bytes()[rest.len() - 2]) {
            return None;
        }
        return Some(CompletionContext { kind: CompletionKind::Head, typed: typed.to_string(), typed_s });
    }
    let lt = before.rfind('<')?;
    let frag = &before[lt..];
    if frag.contains('>') {
        return None;
    }
    if let Some(sp) = frag.find(char::is_whitespace) {
        let tag = &frag[1..sp];
        if let Some(attr) = include_attr(tag) {
            // attr="typed
            let q = frag.rfind(['"', '\''])?;
            let typed = &frag[q + 1..];
            let head = frag[..q].trim_end().strip_suffix('=')?.trim_end();
            if head.ends_with(attr) && !typed.contains(['"', '\'']) {
                return Some(CompletionContext { kind: CompletionKind::Include, typed: typed.to_string(), typed_s: pos - typed.len() });
            }
        }
        return None;
    }
    let (prefix, typed) = frag[1..].split_once(':')?;
    if prefix.is_empty() || !prefix.bytes().all(is_ident) || !typed.bytes().all(|c| is_ident(c) || c == b'-') {
        return None;
    }
    Some(CompletionContext { kind: CompletionKind::Composite(prefix.to_string()), typed: typed.to_string(), typed_s: pos - typed.len() })
}

// ---- Java naming -------------------------------------------------------------------------------

/// CDI/JSF lower-case the first char; Spring/JavaBeans (`introspector`) keep "URL"-style names.
pub fn decapitalize(name: &str, introspector: bool) -> String {
    let mut ch = name.chars();
    let Some(first) = ch.next() else { return String::new() };
    if introspector && first.is_uppercase() && name.chars().nth(1).is_some_and(|c| c.is_uppercase()) {
        return name.to_string();
    }
    first.to_lowercase().collect::<String>() + ch.as_str()
}

pub fn capitalize(name: &str) -> String {
    let mut ch = name.chars();
    ch.next().map(|f| f.to_uppercase().collect::<String>() + ch.as_str()).unwrap_or_default()
}

/// getFoo/isFoo/setFoo → ("foo", true); anything else → (name, false).
pub fn property_name_for(member: &str) -> (String, bool) {
    for p in ["get", "is", "set"] {
        if let Some(rest) = member.strip_prefix(p) {
            if rest.chars().next().is_some_and(|c| c.is_uppercase()) {
                return (decapitalize(rest, true), true);
            }
        }
    }
    (member.to_string(), false)
}

fn last_ident(s: &str) -> Option<(usize, usize)> {
    let s = s.trim_end_matches(|c: char| c.is_whitespace() || c == '[' || c == ']');
    let e = s.len();
    let st = s.bytes().rev().take_while(|&c| is_ident(c) || c == b'$').count();
    if st == 0 {
        return None;
    }
    Some((e - st, e))
}

/// In the text of a declaration before a member's name ("public List<Item> "), the type
/// identifier to resolve: the declared type, or with `element` its element type (array
/// component, or the last type argument). Returns (start, end) byte offsets.
pub fn type_token(prefix: &str, element: bool) -> Option<(usize, usize)> {
    let mut s = prefix.trim_end();
    let mut array = false;
    while let Some(x) = s.strip_suffix(']') {
        let x = x.trim_end();
        let Some(x) = x.strip_suffix('[') else { break };
        s = x.trim_end();
        array = true;
    }
    let (mut args, mut outer) = (None, s);
    if s.ends_with('>') {
        let b = s.as_bytes();
        let mut depth = 0i32;
        let mut found = None;
        for i in (0..b.len()).rev() {
            match b[i] {
                b'>' => depth += 1,
                b'<' => depth -= 1,
                _ => {}
            }
            if depth == 0 {
                found = Some(i);
                break;
            }
        }
        let i = found?;
        args = Some((i + 1, s.len() - 1));
        outer = &s[..i];
    }
    if !element || array {
        return last_ident(outer);
    }
    let (a, e) = args?;
    let b = s.as_bytes();
    let (mut depth, mut last) = (0, a);
    for (i, &ch) in b.iter().enumerate().take(e).skip(a) {
        match ch {
            b'<' => depth += 1,
            b'>' => depth -= 1,
            b',' if depth == 0 => last = i + 1,
            _ => {}
        }
    }
    let arg = &s[last..e];
    let head = arg.find('<').map(|g| &arg[..g]).unwrap_or(arg);
    let (st, fin) = last_ident(head)?;
    if matches!(&head[st..fin], "extends" | "super") {
        return None;
    }
    Some((last + st, last + fin))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str, marker: &str) -> (String, usize) {
        let pos = text.find('|').unwrap();
        let _ = marker;
        (text.replacen('|', "", 1), pos)
    }

    #[test]
    fn parse_el_targets() {
        let (t, p) = at(r##"<h:outputText value="#{helloBean.cus|tomer.name}"/>"##, "");
        let r = parse_el(&t, p).unwrap();
        assert_eq!((r.bean.as_str(), r.target, r.kind, r.member.as_deref()), ("helloBean", 1, RefKind::Property, Some("customer")));
        let (t, p) = at(r##"<p:commandButton action="#{helloBean.sa|ve(item.id)}"/>"##, "");
        let r = parse_el(&t, p).unwrap();
        assert_eq!((r.target, r.kind), (1, RefKind::Method));
        let (t, p) = at(r##"#{helloBean.save(item.i|d)}"##, "");
        let r = parse_el(&t, p).unwrap();
        assert_eq!((r.bean.as_str(), r.member.as_deref()), ("item", Some("id")));
        let (t, p) = at(r##"#{empty hel|loBean.items ? 'a' : b.c}"##, "");
        let r = parse_el(&t, p).unwrap();
        assert_eq!((r.bean.as_str(), r.target), ("helloBean", 0));
        let (t, p) = at(r##"#{bean.items[0].na|me} #{bean['label']}"##, "");
        let r = parse_el(&t, p).unwrap();
        assert!(r.chain[1].element);
        assert_eq!(r.member.as_deref(), Some("name"));
        let p2 = t.find("abel").unwrap();
        assert_eq!(parse_el(&t, p2).unwrap().member.as_deref(), Some("label"));
        assert!(parse_el("<div>plain</div>", 3).is_none());
        // a '}' inside a string does not end the expression
        assert_eq!(el_spans("#{a.b('}')} x").len(), 1);
        // EL functions and keywords are not beans
        let (t, p) = at("#{fn:length(bean.it|ems) gt 0}", "");
        assert_eq!(parse_el(&t, p).unwrap().bean, "bean");
    }

    #[test]
    fn tags_includes_composites() {
        let t = r##"<ui:composition template="/WEB-INF/t.xhtml" xmlns:cc="http://xmlns.jcp.org/jsf/composite/comp">
<ui:include src="includes/header.xhtml"/>
<cc:greeting who="x"/>"##;
        let p = t.find("header").unwrap();
        assert_eq!(parse_include_path(t, p).as_deref(), Some("includes/header.xhtml"));
        assert_eq!(parse_include_path(t, t.find("WEB-INF").unwrap()).as_deref(), Some("/WEB-INF/t.xhtml"));
        let ns = composite_namespaces(t);
        assert_eq!(ns.get("cc").map(String::as_str), Some("comp"));
        let p = t.find("greeting").unwrap() + 2;
        assert_eq!(parse_composite_tag(t, p, &ns), Some(("comp".into(), "greeting".into())));
        let tag = parse_tag(r##"<h:x rendered="#{a > b}" id='q'/>"##, 0).unwrap();
        assert!(tag.self_closing);
        assert_eq!(tag.attrs["rendered"].value, "#{a > b}");
    }

    #[test]
    fn page_variables_scope() {
        let t = r##"<ui:repeat value="#{bean.items}" var="item"><h:outputText value="#{item.name}"/></ui:repeat>#{item.x}<c:set var="total" value="#{bean.total}"/>#{total}"##;
        let inside = t.find("item.name").unwrap();
        let b = find_var_binding(t, inside, "item").unwrap();
        assert_eq!((b.expr.as_str(), b.iterate), ("#{bean.items}", true));
        let after = t.find("item.x").unwrap();
        assert!(find_var_binding(t, after, "item").is_none());
        let end = t.len() - 3;
        assert_eq!(find_var_binding(t, end, "total").unwrap().iterate, false);
        let vars: Vec<String> = page_variables(t, end).into_iter().map(|v| v.0).collect();
        assert_eq!(vars, vec!["total"]);
    }

    #[test]
    fn completion_contexts() {
        let t = "<h:x value=\"#{helloBean.cust";
        let cc = completion_context(t, t.len()).unwrap();
        match cc.kind {
            CompletionKind::Member(r) => assert_eq!((r.bean.as_str(), r.chain.len()), ("helloBean", 0)),
            k => panic!("{k:?}"),
        }
        assert_eq!((cc.typed.as_str(), cc.typed_s), ("cust", t.len() - 4));
        let t = "<h:x value=\"#{a.b.";
        match completion_context(t, t.len()).unwrap().kind {
            CompletionKind::Member(r) => assert_eq!((r.bean.as_str(), r.chain.len(), r.target), ("a", 1, 1)),
            k => panic!("{k:?}"),
        }
        let t = "<h:x value=\"#{hel";
        assert_eq!(completion_context(t, t.len()).unwrap().kind, CompletionKind::Head);
        let t = "<h:x value=\"#{";
        assert_eq!(completion_context(t, t.len()).unwrap().kind, CompletionKind::Head);
        let t = "<h:x value=\"#{1.";
        assert!(completion_context(t, t.len()).is_none());
        let t = "<ui:include src=\"/WEB";
        let cc = completion_context(t, t.len()).unwrap();
        assert_eq!((cc.kind, cc.typed.as_str()), (CompletionKind::Include, "/WEB"));
        let t = "<cc:gre";
        assert_eq!(completion_context(t, t.len()).unwrap().kind, CompletionKind::Composite("cc".into()));
        let t = "#{a.b} plain text";
        assert!(completion_context(t, t.len()).is_none());
    }

    #[test]
    fn java_naming_and_types() {
        assert_eq!(decapitalize("HelloBean", false), "helloBean");
        assert_eq!(decapitalize("URLBean", true), "URLBean");
        assert_eq!(decapitalize("URLBean", false), "uRLBean");
        assert_eq!(property_name_for("getCustomer"), ("customer".into(), true));
        assert_eq!(property_name_for("isActive"), ("active".into(), true));
        assert_eq!(property_name_for("getURL"), ("URL".into(), true));
        assert_eq!(property_name_for("save"), ("save".into(), false));
        let p = "    public List<Item> ";
        let (s, e) = type_token(p, false).unwrap();
        assert_eq!(&p[s..e], "List");
        let (s, e) = type_token(p, true).unwrap();
        assert_eq!(&p[s..e], "Item");
        let p = "private Map<String, List<Order>> ";
        let (s, e) = type_token(p, true).unwrap();
        assert_eq!(&p[s..e], "List");
        let p = "public Item[] ";
        let (s, e) = type_token(p, true).unwrap();
        assert_eq!(&p[s..e], "Item");
        let p = "private com.x.Address ";
        let (s, e) = type_token(p, false).unwrap();
        assert_eq!(&p[s..e], "Address");
    }
}
