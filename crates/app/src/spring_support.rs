//! Spring Boot in the IDE (like IntelliJ's Spring plugin): completion / Ctrl+B / Ctrl+Q for
//! keys in application.properties|yml and in `@Value("${…}")`, the Endpoints and Beans lists,
//! and "go to bean" from an injection point. The logic lives in the `spring` crate.
use crate::editor_view::EditorView;
use crate::search::Hit;
use crate::workspace::Workspace;
use gpui::{Context, Entity, Window};
use serde_json::{Value, json};
use spring::config::{self, Kind};
use spring::metadata::Prop;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub type Metadata = Arc<BTreeMap<String, Prop>>;

/// What the cursor is on, decided from the text alone (cheap, on the UI thread).
enum Site {
    /// typing a key: items are full keys relative to `parent` ("" = top level)
    Key { parent: String, word_start: usize },
    /// typing the value of `key`
    Value { key: String, word_start: usize },
    /// inside `${…}` in Java
    Placeholder { word_start: usize },
}

fn site(path: &Path, text: &str, at: usize) -> Option<Site> {
    let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let line = &text[line_start..at];
    match config::kind_of(path) {
        Some(Kind::Properties) => {
            if line.trim_start().starts_with(['#', '!']) {
                return None;
            }
            match line.find(['=', ':']) {
                Some(sep) => {
                    let key = line[..sep].trim().to_string();
                    let v = &line[sep + 1..];
                    Some(Site::Value { key, word_start: line_start + sep + 1 + (v.len() - v.trim_start().len()) })
                }
                None => Some(Site::Key { parent: String::new(), word_start: line_start + (line.len() - line.trim_start().len()) }),
            }
        }
        Some(Kind::Yaml) => {
            if line.trim_start().starts_with('#') {
                return None;
            }
            let indent = line.len() - line.trim_start().len();
            let entries = config::parse_yaml(text);
            let line_no = text[..line_start].matches('\n').count();
            let parent = config::yaml_parent(&entries, line_no, indent);
            let body = line.trim_start().trim_start_matches("- ");
            let body_start = at - body.len();
            match body.find(": ") {
                Some(sep) => {
                    let own = body[..sep].trim();
                    let key = if parent.is_empty() { own.to_string() } else { format!("{parent}.{own}") };
                    let v = &body[sep + 1..];
                    Some(Site::Value { key, word_start: body_start + sep + 1 + (v.len() - v.trim_start().len()) })
                }
                None => Some(Site::Key { parent, word_start: body_start }),
            }
        }
        None => config::placeholder_at(text, at).map(|(_, start, _)| Site::Placeholder { word_start: start }),
    }
}

fn short_type(ty: &str) -> String {
    // java.util.Map<java.lang.String,java.lang.Integer> → Map<String,Integer>
    let mut out = String::new();
    let mut word = String::new();
    for c in ty.chars() {
        if c.is_alphanumeric() || c == '.' || c == '_' || c == '$' {
            word.push(c);
        } else {
            out.push_str(word.rsplit('.').next().unwrap_or(""));
            word.clear();
            out.push(c);
        }
    }
    out.push_str(word.rsplit('.').next().unwrap_or(""));
    out
}

fn key_item(key: &str, insert: &str, label: &str, prop: Option<&Prop>, project_value: Option<&str>) -> Value {
    let detail = prop.map(|p| short_type(&p.ty)).filter(|t| !t.is_empty()).or_else(|| project_value.map(|v| format!("= {v}"))).unwrap_or_default();
    let deprecated = prop.is_some_and(|p| p.deprecated);
    json!({
        "label": label,
        "insertText": insert,
        "filterText": label,
        "kind": 10,
        "detail": detail,
        "documentation": prop.map(|p| p.description.clone()).unwrap_or_default(),
        // the project's own keys first, deprecated last
        "sortText": format!("{}{}", if deprecated { 2 } else if project_value.is_some() { 0 } else { 1 }, key),
        "tags": if deprecated { json!([1]) } else { json!([]) },
        "data": { "local": true },
    })
}

fn items(site: &Site, path: &Path, meta: &BTreeMap<String, Prop>, project_keys: &[(String, Option<String>)]) -> Vec<Value> {
    let mut keys: BTreeMap<&str, (Option<&Prop>, Option<&str>)> = BTreeMap::new();
    for (k, p) in meta {
        keys.insert(k.as_str(), (Some(p), None));
    }
    for (k, v) in project_keys {
        keys.entry(k.as_str()).or_insert((None, None)).1 = v.as_deref().or(Some(""));
    }
    let yaml = config::kind_of(path) == Some(Kind::Yaml);
    match site {
        Site::Key { parent, .. } => keys
            .iter()
            .filter_map(|(k, (p, v))| {
                let rel = if parent.is_empty() { *k } else { k.strip_prefix(parent.as_str())?.strip_prefix('.')? };
                let insert = if yaml { format!("{rel}: ") } else { format!("{rel}=") };
                Some(key_item(k, &insert, rel, *p, *v))
            })
            .collect(),
        Site::Placeholder { .. } => keys.iter().map(|(k, (p, v))| key_item(k, k, k, *p, *v)).collect(),
        Site::Value { key, .. } => {
            let want = spring::canonical(key);
            let prop = meta.iter().find(|(k, _)| spring::canonical(k) == want).map(|(_, p)| p);
            let mut out: Vec<Value> = prop
                .map(|p| p.values.iter().map(|(v, d)| json!({ "label": v, "insertText": v, "kind": 12, "detail": d, "data": { "local": true } })).collect())
                .unwrap_or_default();
            // logging.level.* and enum-typed keys without hints: the usual levels
            if out.is_empty() && (key.starts_with("logging.level") || prop.is_some_and(|p| p.ty.ends_with("LogLevel"))) {
                out = ["TRACE", "DEBUG", "INFO", "WARN", "ERROR", "OFF"].iter().map(|v| json!({ "label": v, "kind": 12, "data": { "local": true } })).collect();
            }
            out
        }
    }
}

impl Workspace {
    async fn spring_meta(this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp) -> Option<Metadata> {
        let (cached, project) = this.read_with(cx, |ws, _| (ws.spring_meta.clone(), ws.project.clone())).ok()?;
        if let Some(m) = cached {
            return Some(m);
        }
        let project = project?;
        let m: Metadata = Arc::new(cx.background_executor().spawn(async move { spring::metadata::load(&project) }).await);
        let _ = this.update(cx, |ws, _| ws.spring_meta = Some(m.clone()));
        Some(m)
    }

    fn open_at(&mut self, path: PathBuf, line: usize, col: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.push_nav(cx);
        self.open_file(path, window, cx);
        if let Some(t) = self.active.and_then(|i| self.tabs.get(i)) {
            t.update(cx, |v, cx| {
                v.goto(line, col);
                cx.notify()
            });
        }
    }

    /// One location: open it; several: a list to pick from.
    fn show_config_locations(&mut self, title: String, locs: Vec<config::Location>, window: &mut Window, cx: &mut Context<Self>) {
        match locs.len() {
            0 => {}
            1 => {
                let l = locs.into_iter().next().unwrap();
                self.open_at(l.path, l.line, l.col, window, cx)
            }
            _ => {
                let hits: Vec<Hit> = locs.into_iter().map(|l| Hit { text: l.text, path: l.path, line: l.line, col: l.col, mark: None }).collect();
                self.open_list(format!("{title} ({})", hits.len()), hits, window, cx)
            }
        }
    }

    fn spring_key_at(t: &Entity<EditorView>, at: usize, cx: &Context<Self>) -> Option<(String, PathBuf)> {
        let v = t.read(cx);
        let path = v.path()?.clone();
        let text = v.editor.buffer.text();
        let key = match config::kind_of(&path) {
            Some(_) => {
                let line = text[..at.min(text.len())].matches('\n').count();
                config::entry_at(&config::parse(&path, &text), line)?.key.clone()
            }
            None => config::placeholder_at(&text, at)?.0,
        };
        Some((key, path))
    }

    /// Ctrl+B: `${key}` in Java → where it is set; a key in a config file → the Java reading it.
    pub(crate) fn spring_goto(&mut self, t: Entity<EditorView>, at: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((key, path)) = Self::spring_key_at(&t, at, cx) else {
            self.message = Some("Không có key Spring ở con trỏ".into());
            return cx.notify();
        };
        let Some(project) = self.project.clone() else { return };
        let in_config = config::kind_of(&path).is_some();
        cx.spawn_in(window, async move |this, cx| {
            let (key2, p2) = (key.clone(), project.clone());
            let locs = cx.background_executor().spawn(async move { if in_config { config::java_usages(&p2, &key2) } else { config::definitions(&p2, &key2) } }).await;
            let meta = if locs.is_empty() { Self::spring_meta(this.clone(), cx).await } else { None };
            let _ = this.update_in(cx, |ws, window, cx| {
                if locs.is_empty() {
                    let known = meta.as_ref().and_then(|m| m.get(&key)).map(|p| p.description.clone());
                    ws.message = Some(match (in_config, known) {
                        (_, Some(d)) => format!("{key}: key của Spring Boot - {}", d.lines().next().unwrap_or("")),
                        (true, None) => format!("Không có code Java nào đọc {key}"),
                        (false, None) => format!("{key} chưa được đặt trong application*.properties/yml"),
                    });
                    return cx.notify();
                }
                let title = if in_config { format!("Code đọc {key}") } else { format!("Nơi đặt {key}") };
                ws.show_config_locations(title, locs, window, cx);
            });
        })
        .detach();
    }

    pub(crate) fn spring_complete(&mut self, t: Entity<EditorView>, at: usize, explicit: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (path, text) = {
            let v = t.read(cx);
            (v.path().cloned(), v.editor.buffer.text())
        };
        let Some(path) = path else { return };
        let Some(site) = site(&path, &text, at) else {
            return t.update(cx, |v, cx| v.set_jsf_completion(at, at, Vec::new(), explicit, cx));
        };
        let word_start = match &site {
            Site::Key { word_start, .. } | Site::Value { word_start, .. } | Site::Placeholder { word_start } => *word_start,
        };
        let project = self.project.clone();
        cx.spawn_in(window, async move |this, cx| {
            let meta = Self::spring_meta(this.clone(), cx).await.unwrap_or_default();
            let list = cx
                .background_executor()
                .spawn(async move {
                    let pk = project.as_ref().map(config::project_keys).unwrap_or_default();
                    items(&site, &path, &meta, &pk)
                })
                .await;
            let _ = t.update(cx, |v, cx| v.set_jsf_completion(at, word_start, list, explicit, cx));
        })
        .detach();
    }

    /// Ctrl+Q on a key: Spring Boot's description + the value in every profile.
    pub(crate) fn spring_hover(&mut self, t: Entity<EditorView>, at: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((key, _)) = Self::spring_key_at(&t, at, cx) else {
            return t.update(cx, |v, cx| {
                v.message = Some("Không có tài liệu".into());
                cx.notify()
            });
        };
        let project = self.project.clone();
        cx.spawn_in(window, async move |this, cx| {
            let meta = Self::spring_meta(this.clone(), cx).await;
            let k2 = key.clone();
            let defs = cx.background_executor().spawn(async move { project.map(|p| config::definitions(&p, &k2)).unwrap_or_default() }).await;
            let mut out = format!("{key}");
            if let Some(p) = meta.as_ref().and_then(|m| m.get(&key)) {
                if !p.ty.is_empty() {
                    out.push_str(&format!("  ({})", short_type(&p.ty)));
                }
                if p.deprecated {
                    out.push_str("  [deprecated]");
                }
                if !p.description.is_empty() {
                    out.push_str(&format!("\n\n{}", p.description));
                }
                if let Some(d) = &p.default {
                    out.push_str(&format!("\nMặc định: {d}"));
                }
            }
            if !defs.is_empty() {
                out.push_str("\n\nGiá trị trong project:");
                for d in &defs {
                    let file = d.path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
                    out.push_str(&format!("\n  {file}: {}", d.text));
                }
            }
            let _ = t.update(cx, |v, cx| {
                v.hover = Some(out);
                cx.notify()
            });
        })
        .detach();
    }

    /// Endpoints tool window (IntelliJ: Services → Endpoints): every mapped handler.
    pub(crate) fn show_endpoints(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.project.clone() else {
            self.message = Some("Chưa có Project Model".into());
            return cx.notify();
        };
        cx.spawn_in(window, async move |this, cx| {
            let eps = cx.background_executor().spawn(async move { spring::endpoints::endpoints(&project) }).await;
            let _ = this.update_in(cx, |ws, window, cx| {
                if eps.is_empty() {
                    ws.message = Some("Không có @RestController/@Controller nào".into());
                    return cx.notify();
                }
                let hits: Vec<Hit> = eps
                    .into_iter()
                    .map(|e| {
                        let text = format!("{:<7}{}   {}.{}", e.method, e.path, e.class, e.handler);
                        let mark = Some((7, 7 + e.path.len()));
                        Hit { path: e.file, line: e.line, col: 0, text, mark }
                    })
                    .collect();
                ws.open_list(format!("Endpoints ({})", hits.len()), hits, window, cx);
            });
        })
        .detach();
    }

    pub(crate) fn show_beans(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.project.clone() else {
            self.message = Some("Chưa có Project Model".into());
            return cx.notify();
        };
        cx.spawn_in(window, async move |this, cx| {
            let beans = cx.background_executor().spawn(async move { spring::beans::beans(&project) }).await;
            let _ = this.update_in(cx, |ws, window, cx| {
                let hits: Vec<Hit> = beans
                    .into_iter()
                    .map(|b| {
                        let text = format!("@{:<15}{}   : {}", b.kind, b.name, b.ty);
                        let mark = Some((16, 16 + b.name.len()));
                        Hit { path: b.file, line: b.line, col: 0, text, mark }
                    })
                    .collect();
                ws.open_list(format!("Spring Beans ({})", hits.len()), hits, window, cx);
            });
        })
        .detach();
    }

    /// From an injection point (`private MailService mail;`, constructor parameter…): the
    /// beans that would be injected there. The type is the word at the cursor, or the type of
    /// the field/parameter declared on that line.
    pub(crate) fn goto_bean(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.active.and_then(|i| self.tabs.get(i)).cloned() else { return };
        let (line, word) = {
            let v = t.read(cx);
            let b = &v.editor.buffer;
            let l = b.line_of(v.editor.head);
            let text = b.slice(b.line_start(l)..b.line_end(l));
            let col = v.editor.head - b.line_start(l);
            (text.clone(), word_at(&text, col))
        };
        let Some(project) = self.project.clone() else { return };
        let candidates_ty: Vec<String> = [word, injected_type(&line)].into_iter().flatten().filter(|w| w.chars().next().is_some_and(char::is_uppercase)).collect();
        cx.spawn_in(window, async move |this, cx| {
            let beans = cx.background_executor().spawn(async move { spring::beans::beans(&project) }).await;
            let _ = this.update_in(cx, |ws, window, cx| {
                for ty in &candidates_ty {
                    let found = spring::beans::candidates(&beans, ty);
                    if found.is_empty() {
                        continue;
                    }
                    let locs = found.iter().map(|b| config::Location { path: b.file.clone(), line: b.line, col: 0, text: format!("@{} {} : {}", b.kind, b.name, b.ty) }).collect();
                    return ws.show_config_locations(format!("Bean kiểu {ty}"), locs, window, cx);
                }
                ws.message = Some(match candidates_ty.first() {
                    Some(ty) => format!("Không có bean nào kiểu {ty} trong project (có thể do auto-configuration)"),
                    None => "Đặt con trỏ trên kiểu của field/tham số được inject".into(),
                });
                cx.notify();
            });
        })
        .detach();
    }
}

fn word_at(line: &str, col: usize) -> Option<String> {
    let col = col.min(line.len());
    let is_w = |c: char| c.is_alphanumeric() || c == '_';
    let start = line[..col].rfind(|c: char| !is_w(c)).map_or(0, |i| i + 1);
    let end = line[col..].find(|c: char| !is_w(c)).map_or(line.len(), |i| col + i);
    (start < end).then(|| line[start..end].to_string())
}

/// `private final MailService mailService;` → "MailService"; `List<Handler> hs` → "Handler"
/// (Spring injects every bean of the element type into a collection).
fn injected_type(line: &str) -> Option<String> {
    let decl = line.trim().trim_end_matches([';', ',', ')']);
    let decl = decl.split('=').next()?.trim();
    // drop the variable name, then annotations and modifiers
    let ty_part = decl.rsplit_once(char::is_whitespace)?.0.trim();
    let ty = ty_part.split_whitespace().filter(|w| !w.starts_with('@') && !matches!(*w, "private" | "protected" | "public" | "final" | "static")).collect::<Vec<_>>().join(" ");
    let ty = ty.rsplit(['(', ' ']).find(|s| !s.is_empty()).map(String::from).unwrap_or(ty);
    let ty = match ty.split_once('<') {
        Some((outer, inner)) if matches!(outer, "List" | "Set" | "Collection" | "Optional" | "ObjectProvider" | "Provider" | "Map") => {
            inner.trim_end_matches('>').rsplit(',').next()?.trim().to_string()
        }
        Some((outer, _)) => outer.to_string(),
        None => ty,
    };
    Some(ty.rsplit('.').next()?.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_sites() {
        let p = Path::new("/r/application.properties");
        let text = "server.po";
        assert!(matches!(site(p, text, text.len()), Some(Site::Key { word_start: 0, .. })));
        let text = "logging.level.root=IN";
        match site(p, text, text.len()) {
            Some(Site::Value { key, word_start }) => assert_eq!((key.as_str(), word_start), ("logging.level.root", 19)),
            _ => panic!(),
        }
        let y = Path::new("/r/application.yml");
        let text = "spring:\n  datasource:\n    ur";
        match site(y, text, text.len()) {
            Some(Site::Key { parent, word_start }) => assert_eq!((parent.as_str(), word_start), ("spring.datasource", text.len() - 2)),
            _ => panic!(),
        }
        let text = "spring:\n  main:\n    lazy-initialization: t";
        match site(y, text, text.len()) {
            Some(Site::Value { key, .. }) => assert_eq!(key, "spring.main.lazy-initialization"),
            _ => panic!(),
        }
        let j = Path::new("/r/A.java");
        let text = "@Value(\"${serv";
        assert!(matches!(site(j, text, text.len()), Some(Site::Placeholder { word_start: 10 })));
        assert_eq!(short_type("java.util.Map<java.lang.String,java.lang.Integer>"), "Map<String,Integer>");
        assert_eq!(injected_type("    private final MailService mailService;").as_deref(), Some("MailService"));
        assert_eq!(injected_type("@Autowired List<Handler> handlers;").as_deref(), Some("Handler"));
        assert_eq!(injected_type("    public Svc(@Qualifier(\"x\") MailService mail,").as_deref(), Some("MailService"));
    }
}
