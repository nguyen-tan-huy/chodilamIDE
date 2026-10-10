//! Facelets tag completion (IntelliJ's JSF support): tag names after `<p:`, attributes
//! inside a start tag. PrimeFaces / OmniFaces / any library come from the `*.taglib.xml`
//! in the project's jars; the standard h: f: ui: c: composite: tags are built in.
use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TagAttr {
    pub name: String,
    pub required: bool,
    pub ty: String,
    pub description: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TagDef {
    pub name: String,
    pub description: String,
    pub attrs: Vec<TagAttr>,
}

/// namespace key (see `ns_key`) → tag name → definition
pub type TagLibs = HashMap<String, BTreeMap<String, TagDef>>;

/// The same library under its javax / jcp / jakarta namespaces maps to one key.
pub fn ns_key(uri: &str) -> String {
    let u = uri.trim();
    for (suffix, key) in [("/jsf/html", "html"), ("/jsf/core", "core"), ("/jsf/facelets", "facelets"), ("/jsf/composite", "composite"), ("/jsp/jstl/core", "jstl"), ("/jsf/passthrough", "passthrough")] {
        if u.ends_with(suffix) {
            return key.into();
        }
    }
    match u {
        "jakarta.faces.html" => "html".into(),
        "jakarta.faces.core" => "core".into(),
        "jakarta.faces.facelets" => "facelets".into(),
        "jakarta.faces.composite" => "composite".into(),
        "jakarta.tags.core" => "jstl".into(),
        _ => u.to_string(),
    }
}

fn text_of(block: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let i = block.find(&open)? + open.len();
    let j = block[i..].find(&format!("</{tag}>"))?;
    Some(block[i..i + j].split_whitespace().collect::<Vec<_>>().join(" "))
}

/// A facelet-taglib XML → (namespace, tags).
pub fn parse_taglib(xml: &str) -> Option<(String, BTreeMap<String, TagDef>)> {
    let ns = text_of(xml, "namespace")?;
    let mut tags = BTreeMap::new();
    let mut rest = xml;
    while let Some(i) = rest.find("<tag>") {
        let Some(j) = rest[i..].find("</tag>") else { break };
        let block = &rest[i + 5..i + j];
        rest = &rest[i + j + 6..];
        let Some(name) = text_of(block, "tag-name") else { continue };
        // the tag's own description comes before its first <attribute>
        let head = block.split("<attribute>").next().unwrap_or("");
        let mut def = TagDef { name: name.clone(), description: text_of(head, "description").unwrap_or_default(), attrs: Vec::new() };
        let mut ar = block;
        while let Some(a) = ar.find("<attribute>") {
            let Some(b) = ar[a..].find("</attribute>") else { break };
            let ab = &ar[a + 11..a + b];
            ar = &ar[a + b + 12..];
            if let Some(n) = text_of(ab, "name") {
                def.attrs.push(TagAttr {
                    name: n,
                    required: text_of(ab, "required").is_some_and(|r| r == "true"),
                    ty: text_of(ab, "type").unwrap_or_default(),
                    description: text_of(ab, "description").unwrap_or_default(),
                });
            }
        }
        tags.insert(name, def);
    }
    Some((ns_key(&ns), tags))
}

/// Attributes most standard components share.
const COMMON: &[&str] = &["id", "rendered", "binding"];
const HTML_UI: &[&str] = &["style", "styleClass", "title", "onclick", "ondblclick", "onkeydown", "onkeyup", "onmouseover", "onmouseout"];
const INPUT: &[&str] = &["value", "required", "requiredMessage", "converter", "converterMessage", "validator", "validatorMessage", "valueChangeListener", "immediate", "disabled", "readonly", "label", "onchange", "onblur", "onfocus", "placeholder"];

fn builtin_lib(tags: &[(&str, &[&str])], extra_common: bool) -> BTreeMap<String, TagDef> {
    tags.iter()
        .map(|(name, attrs)| {
            let mut all: Vec<&str> = attrs.to_vec();
            if extra_common {
                all.extend(COMMON);
            }
            all.dedup();
            (name.to_string(), TagDef { name: name.to_string(), description: String::new(), attrs: all.iter().map(|a| TagAttr { name: a.to_string(), ..Default::default() }).collect() })
        })
        .collect()
}

/// h: f: ui: c: composite: (the parts of the standard taglibs people type every day).
pub fn builtin() -> TagLibs {
    let input: Vec<&str> = INPUT.iter().chain(HTML_UI).copied().collect();
    let mut libs = TagLibs::new();
    let h: Vec<(&str, Vec<&str>)> = vec![
        ("form", [&["prependId", "enctype", "acceptcharset", "onsubmit"][..], HTML_UI].concat()),
        ("inputText", [&["size", "maxlength", "autocomplete"][..], &input].concat()),
        ("inputSecret", [&["size", "maxlength", "redisplay"][..], &input].concat()),
        ("inputTextarea", [&["rows", "cols"][..], &input].concat()),
        ("inputHidden", vec!["value", "converter", "required"]),
        ("outputText", [&["value", "escape", "converter"][..], HTML_UI].concat()),
        ("outputLabel", [&["value", "for", "escape"][..], HTML_UI].concat()),
        ("outputLink", [&["value", "target"][..], HTML_UI].concat()),
        ("outputFormat", vec!["value", "escape"]),
        ("commandButton", [&["value", "action", "actionListener", "type", "immediate", "disabled", "image"][..], HTML_UI].concat()),
        ("commandLink", [&["value", "action", "actionListener", "immediate", "disabled", "target"][..], HTML_UI].concat()),
        ("link", [&["value", "outcome", "includeViewParams", "fragment"][..], HTML_UI].concat()),
        ("button", [&["value", "outcome", "includeViewParams"][..], HTML_UI].concat()),
        ("graphicImage", [&["value", "name", "library", "alt", "width", "height", "url"][..], HTML_UI].concat()),
        ("panelGrid", [&["columns", "columnClasses", "rowClasses", "cellpadding", "cellspacing", "border", "width"][..], HTML_UI].concat()),
        ("panelGroup", [&["layout"][..], HTML_UI].concat()),
        ("dataTable", [&["value", "var", "rows", "first", "rowClasses", "columnClasses", "headerClass", "footerClass"][..], HTML_UI].concat()),
        ("column", vec!["headerClass", "footerClass"]),
        ("selectOneMenu", [&["size"][..], &input].concat()),
        ("selectOneRadio", [&["layout"][..], &input].concat()),
        ("selectManyCheckbox", [&["layout"][..], &input].concat()),
        ("selectManyMenu", input.clone()),
        ("selectBooleanCheckbox", input.clone()),
        ("message", [&["for", "showDetail", "showSummary", "errorClass", "infoClass"][..], HTML_UI].concat()),
        ("messages", [&["globalOnly", "showDetail", "showSummary", "layout", "errorClass", "infoClass"][..], HTML_UI].concat()),
        ("head", vec![]),
        ("body", vec!["style", "styleClass", "onload"]),
        ("outputScript", vec!["name", "library", "target"]),
        ("outputStylesheet", vec!["name", "library"]),
    ];
    let h_refs: Vec<(&str, &[&str])> = h.iter().map(|(n, a)| (*n, a.as_slice())).collect();
    libs.insert("html".into(), builtin_lib(&h_refs, true));
    libs.insert(
        "core".into(),
        builtin_lib(
            &[
                ("ajax", &["event", "execute", "render", "listener", "onevent", "onerror", "immediate", "disabled", "delay"]),
                ("selectItem", &["itemValue", "itemLabel", "itemDisabled", "noSelectionOption", "value"]),
                ("selectItems", &["value", "var", "itemValue", "itemLabel", "itemDisabled", "noSelectionValue"]),
                ("param", &["name", "value"]),
                ("attribute", &["name", "value"]),
                ("facet", &["name"]),
                ("metadata", &[]),
                ("viewParam", &["name", "value", "required", "converter", "validator"]),
                ("viewAction", &["action", "onPostback", "phase", "if"]),
                ("event", &["type", "listener"]),
                ("convertDateTime", &["pattern", "type", "timeZone", "locale", "dateStyle", "timeStyle"]),
                ("convertNumber", &["pattern", "type", "currencySymbol", "maxFractionDigits", "minFractionDigits", "groupingUsed", "locale"]),
                ("converter", &["converterId", "binding"]),
                ("validator", &["validatorId", "binding", "disabled"]),
                ("validateLength", &["minimum", "maximum"]),
                ("validateRegex", &["pattern"]),
                ("validateRequired", &[]),
                ("actionListener", &["type", "binding"]),
                ("setPropertyActionListener", &["target", "value"]),
                ("loadBundle", &["basename", "var"]),
                ("view", &["locale", "contentType", "encoding"]),
                ("passThroughAttribute", &["name", "value"]),
            ],
            false,
        ),
    );
    libs.insert(
        "facelets".into(),
        builtin_lib(
            &[
                ("composition", &["template"]),
                ("define", &["name"]),
                ("insert", &["name"]),
                ("include", &["src"]),
                ("param", &["name", "value"]),
                ("repeat", &["value", "var", "varStatus", "offset", "size", "step"]),
                ("fragment", &["id", "binding", "rendered"]),
                ("decorate", &["template"]),
                ("component", &["id", "binding", "rendered"]),
                ("debug", &["hotkey", "rendered"]),
                ("remove", &[]),
            ],
            false,
        ),
    );
    libs.insert(
        "jstl".into(),
        builtin_lib(
            &[
                ("if", &["test", "var", "scope"]),
                ("forEach", &["items", "var", "varStatus", "begin", "end", "step"]),
                ("choose", &[]),
                ("when", &["test"]),
                ("otherwise", &[]),
                ("set", &["var", "value", "scope", "target", "property"]),
                ("catch", &["var"]),
            ],
            false,
        ),
    );
    libs.insert(
        "composite".into(),
        builtin_lib(
            &[
                ("interface", &["name", "componentType", "displayName", "shortDescription"]),
                ("implementation", &[]),
                ("attribute", &["name", "type", "required", "default", "method-signature", "targets", "shortDescription"]),
                ("insertChildren", &[]),
                ("renderFacet", &["name", "required"]),
                ("insertFacet", &["name", "required"]),
                ("facet", &["name", "required"]),
                ("actionSource", &["name", "targets", "event"]),
                ("editableValueHolder", &["name", "targets"]),
                ("clientBehavior", &["name", "targets", "event", "default"]),
            ],
            false,
        ),
    );
    libs
}

type Cache = HashMap<PathBuf, (Option<std::time::SystemTime>, Vec<(String, BTreeMap<String, TagDef>)>)>;
static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

fn jar_taglibs(jar: &Path) -> Vec<(String, BTreeMap<String, TagDef>)> {
    let mtime = std::fs::metadata(jar).and_then(|m| m.modified()).ok();
    let mut g = CACHE.lock().unwrap();
    let cache = g.get_or_insert_with(HashMap::new);
    if let Some((t, v)) = cache.get(jar) {
        if *t == mtime {
            return v.clone();
        }
    }
    let mut out = Vec::new();
    if let Ok(f) = std::fs::File::open(jar) {
        if let Ok(mut z) = zip::ZipArchive::new(f) {
            let names: Vec<String> = z.file_names().filter(|n| n.starts_with("META-INF/") && n.ends_with(".taglib.xml")).map(String::from).collect();
            for n in names {
                if let Ok(mut e) = z.by_name(&n) {
                    let mut s = String::new();
                    if e.read_to_string(&mut s).is_ok() {
                        if let Some(lib) = parse_taglib(&s) {
                            out.push(lib);
                        }
                    }
                }
            }
        }
    }
    cache.insert(jar.to_path_buf(), (mtime, out.clone()));
    out
}

/// Built-in libraries plus every taglib in the project's UI jars (PrimeFaces, OmniFaces…).
pub fn load(project: &project_model::Project) -> TagLibs {
    let mut libs = builtin();
    let mut jars: Vec<String> = project
        .modules
        .iter()
        .flat_map(|m| m.classpath_jars.clone().unwrap_or_default())
        .filter(|j| {
            let n = Path::new(j).file_name().and_then(|n| n.to_str()).unwrap_or("").to_lowercase();
            n.ends_with(".jar") && !n.contains("-sources") && (n.contains("faces") || n.contains("primefaces") || n.contains("omnifaces") || n.contains("bootsfaces") || n.contains("richfaces"))
        })
        .collect();
    jars.sort();
    jars.dedup();
    for j in jars {
        for (ns, tags) in jar_taglibs(Path::new(&j)) {
            libs.entry(ns).or_default().extend(tags);
        }
    }
    libs
}

/// `xmlns:p="http://primefaces.org/ui"` declarations of a page: prefix → namespace key.
pub fn prefixes(text: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut rest = text;
    while let Some(i) = rest.find("xmlns:") {
        rest = &rest[i + 6..];
        let Some(eq) = rest.find('=') else { break };
        let prefix = rest[..eq].trim().to_string();
        let after = rest[eq + 1..].trim_start();
        let Some(q) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else { continue };
        let Some(end) = after[1..].find(q) else { continue };
        out.insert(prefix, ns_key(&after[1..1 + end]));
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub enum TagSite {
    /// `<p:dataT|` - tag names of prefix `p`, the typed part starts at `start`
    TagName { prefix: String, start: usize },
    /// inside `<p:dataTable … |`: attributes of that tag (those already written excluded)
    AttrName { prefix: String, tag: String, start: usize, present: Vec<String> },
}

/// What tag completion applies at byte `pos` (None inside attribute values / text).
pub fn tag_site(text: &str, pos: usize) -> Option<TagSite> {
    let before = &text[..pos.min(text.len())];
    let lt = before.rfind('<')?;
    if before[lt..].contains('>') {
        return None;
    }
    let inside = &before[lt + 1..];
    // inside an attribute value: an odd number of quotes
    if inside.matches('"').count() % 2 == 1 || inside.matches('\'').count() % 2 == 1 {
        return None;
    }
    let name_end = inside.find(|c: char| c.is_whitespace()).unwrap_or(inside.len());
    let qname = &inside[..name_end];
    let (prefix, local) = qname.split_once(':')?;
    if name_end == inside.len() {
        // still typing the tag name
        return Some(TagSite::TagName { prefix: prefix.to_string(), start: lt + 1 + prefix.len() + 1 });
    }
    // attribute name being typed: letters since the last whitespace
    let word_len = inside.chars().rev().take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '-' || *c == ':').map(char::len_utf8).sum::<usize>();
    let present = inside[name_end..].split_whitespace().filter_map(|a| a.split('=').next()).filter(|a| !a.is_empty()).map(String::from).collect();
    Some(TagSite::AttrName { prefix: prefix.to_string(), tag: local.to_string(), start: pos - word_len, present })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taglib_and_sites() {
        let xml = r#"<facelet-taglib><namespace>http://primefaces.org/ui</namespace>
            <tag><description>A table</description><tag-name>dataTable</tag-name>
              <attribute><description>Rows</description><name>rows</name><required>false</required><type>java.lang.Integer</type></attribute>
              <attribute><name>value</name><required>true</required></attribute></tag></facelet-taglib>"#;
        let (ns, tags) = parse_taglib(xml).unwrap();
        assert_eq!(ns, "http://primefaces.org/ui");
        let t = &tags["dataTable"];
        assert_eq!((t.description.as_str(), t.attrs.len(), t.attrs[1].required), ("A table", 2, true));
        assert_eq!(ns_key("http://xmlns.jcp.org/jsf/html"), "html");
        assert_eq!(ns_key("jakarta.faces.facelets"), "facelets");
        let page = r#"<ui:composition xmlns:p="http://primefaces.org/ui" xmlns:h="http://java.sun.com/jsf/html">"#;
        let px = prefixes(page);
        assert_eq!((px["p"].as_str(), px["h"].as_str()), ("http://primefaces.org/ui", "html"));
        let s = "<p:dataT";
        assert_eq!(tag_site(s, s.len()), Some(TagSite::TagName { prefix: "p".into(), start: 3 }));
        let s = "<p:dataTable value=\"#{b.x}\" ro";
        assert_eq!(tag_site(s, s.len()), Some(TagSite::AttrName { prefix: "p".into(), tag: "dataTable".into(), start: s.len() - 2, present: vec!["value".into(), "ro".into()] }));
        let s = "<p:dataTable value=\"#{b.";
        assert_eq!(tag_site(s, s.len()), None);
        assert!(builtin()["html"].contains_key("commandButton"));
    }
}

