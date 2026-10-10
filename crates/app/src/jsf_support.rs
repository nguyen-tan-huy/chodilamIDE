//! JSF in the IDE (port of java-debug-model's jsf/nav.lua + jsf/server.lua): Ctrl+B, completion
//! and hover in .xhtml, and the .xhtml EL usages added to Find Usages / Ctrl+B of a Java bean
//! member. Parsing and resolution live in the `jsf` crate; this file wires them to the editor.
use crate::editor_view::EditorView;
use crate::workspace::Workspace;
use gpui::{Context, Entity, Window};
use jsf::beans::BeanIndex;
use jsf::el::{self, CompletionKind};
use jsf::resolve::{Ctx, Target};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

impl Workspace {
    /// The bean index, built in the background on first use and after a Java file changes.
    async fn jsf_index(this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp) -> Option<Arc<BeanIndex>> {
        let (cached, project) = this.read_with(cx, |ws, _| (ws.jsf_index.clone(), ws.project.clone())).ok()?;
        if let Some(i) = cached {
            return Some(i);
        }
        let project = project?;
        let idx = Arc::new(cx.background_executor().spawn(async move { BeanIndex::build(&project) }).await);
        let _ = this.update(cx, |ws, _| ws.jsf_index = Some(idx.clone()));
        Some(idx)
    }

    /// Tag libraries (built-in + taglibs of the project's jars), loaded on first use.
    async fn jsf_taglibs(this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp) -> Arc<jsf::tags::TagLibs> {
        let (cached, project) = this.read_with(cx, |ws, _| (ws.jsf_tags.clone(), ws.project.clone())).unwrap_or((None, None));
        if let Some(t) = cached {
            return t;
        }
        let libs = Arc::new(cx.background_executor().spawn(async move { project.map(|p| jsf::tags::load(&p)).unwrap_or_else(jsf::tags::builtin) }).await);
        let _ = this.update(cx, |ws, _| ws.jsf_tags = Some(libs.clone()));
        libs
    }

    /// A .java file changed: bean names may have too.
    pub(crate) fn jsf_invalidate(&mut self, path: &std::path::Path) {
        if path.extension().is_some_and(|e| e == "java") {
            self.jsf_index = None;
        }
    }

    /// JSF inspections of an .xhtml tab: the static ones at once, the EL ones once jdtls runs.
    pub(crate) fn jsf_check(&mut self, t: Entity<EditorView>, cx: &mut Context<Self>) {
        let Some((text, path)) = Self::tab_text(&t, cx) else { return };
        if path.extension().is_none_or(|e| e != "xhtml") {
            return;
        }
        let to_editor = |d: Vec<jsf::diagnostics::Diag>| -> Vec<crate::lsp_editor::Diag> {
            d.into_iter().map(|d| crate::lsp_editor::Diag { range: d.start..d.end, severity: d.severity, message: format!("{} [{}]", d.message, d.code) }).collect()
        };
        let stat = jsf::diagnostics::static_checks(&text);
        t.update(cx, |v, cx| {
            v.set_byte_diagnostics(to_editor(stat.clone()));
            cx.notify()
        });
        let Some(client) = self.lsp.clone().filter(|_| matches!(self.lsp_state, crate::lsp_workspace::LspState::Ready)) else { return };
        cx.spawn(async move |this, cx| {
            let Some(index) = Self::jsf_index(this.clone(), cx).await else { return };
            let mut ctx = Ctx::new(&client, &index, &text, &path);
            let el = jsf::diagnostics::el_checks(&mut ctx, &index, &text, &path).await;
            let _ = t.update(cx, |v, cx| {
                // the page changed meanwhile: offsets are stale, the next save re-checks
                if v.editor.buffer.text() != text {
                    return;
                }
                let mut all = stat;
                all.extend(el);
                v.set_byte_diagnostics(to_editor(all));
                cx.notify()
            });
        })
        .detach();
    }

    /// After Shift+F6 in a bean's Java file (IntelliJ renames the whole property): the other
    /// members of the property (field ⇄ getter/setter) through jdtls, then every
    /// `#{bean.old}` / `#{bean['old']}` in the pages.
    pub(crate) fn jsf_after_rename(&mut self, t: Entity<EditorView>, old: String, new: String, cx: &mut Context<Self>) {
        let Some(file) = t.read(cx).path().cloned() else { return };
        if file.extension().is_none_or(|e| e != "java") {
            return;
        }
        let project = self.project.clone();
        cx.spawn(async move |this, cx| {
            let Some(index) = Self::jsf_index(this.clone(), cx).await else { return };
            let beans: Vec<String> = index.by_file(&file).into_iter().map(|b| b.name.clone()).collect();
            if beans.is_empty() {
                return;
            }
            let (old_prop, old_is_acc) = el::property_name_for(&old);
            let (new_prop, _) = el::property_name_for(&new);
            let text = t.read_with(cx, |v, _| v.editor.buffer.text()).unwrap_or_default();
            // the rest of the property: only when it really is one (accessor renamed, or a field
            // that has accessors)
            let others: Vec<String> = jsf::refactor::property_members(&old_prop).into_iter().filter(|m| *m != old).collect();
            let others_declared = jsf::refactor::declarations(&text, &others);
            let is_property = old_is_acc || others_declared.iter().any(|(_, _, n)| n != &old_prop);
            let mut extra = 0;
            if is_property {
                // one at a time: every rename shifts positions, so re-read the declarations
                for member in others {
                    let this2 = this.clone();
                    let Ok(Some((client, params))) = t.update(cx, |v, ecx| {
                        v.lsp_sync();
                        let text = v.editor.buffer.text();
                        let (line, col, _) = jsf::refactor::declarations(&text, std::slice::from_ref(&member)).into_iter().next()?;
                        let b = &v.editor.buffer;
                        let at = b.line_start(line) + col;
                        let (l, ch) = b.lsp_position(at);
                        let uri = v.lsp_uri()?.to_string();
                        let client = this2.upgrade()?.read(ecx).lsp.clone()?;
                        Some((client, json!({ "textDocument": { "uri": uri }, "position": { "line": l, "character": ch }, "newName": jsf::refactor::renamed_member(&member, &old_prop, &new_prop) })))
                    }) else {
                        continue;
                    };
                    if let Ok(edit) = client.request("textDocument/rename", params).response().await {
                        if !edit.is_null() {
                            extra += 1;
                            let _ = this.update(cx, |ws, cx| ws.apply_workspace_edit(&edit, cx));
                        }
                    }
                }
            }
            // the pages
            let files = project.map(|p| jsf::beans::xhtml_files(&p)).unwrap_or_default();
            let (mut total, mut changed) = (0, 0);
            for f in files {
                let open = this.read_with(cx, |ws, cx| ws.tabs.iter().find(|t| t.read(cx).path() == Some(&f)).cloned()).ok().flatten();
                let text = match &open {
                    Some(t) => t.read_with(cx, |v, _| v.editor.buffer.text()).unwrap_or_default(),
                    None => std::fs::read_to_string(&f).unwrap_or_default(),
                };
                let mut new_text = text.clone();
                let mut n = 0;
                for b in &beans {
                    let (s, k) = jsf::refactor::replace_el_refs(&new_text, b, &old_prop, &new_prop);
                    new_text = s;
                    n += k;
                }
                if n == 0 {
                    continue;
                }
                total += n;
                changed += 1;
                match open {
                    // an open page: one undoable edit in its buffer
                    Some(t) => {
                        let _ = t.update(cx, |v, cx| {
                            v.editor.anchor = 0;
                            v.editor.head = v.editor.buffer.len();
                            v.editor.insert(&new_text);
                            cx.notify()
                        });
                    }
                    None => {
                        let _ = std::fs::write(&f, new_text);
                    }
                }
            }
            let _ = this.update(cx, |ws, cx| {
                ws.jsf_index = None;
                let acc = if extra > 0 { format!(" + {extra} getter/setter/field") } else { String::new() };
                ws.message = Some(format!("JSF: đổi {total} chỗ #{{{}.{old_prop}}} → {new_prop} trong {changed} file .xhtml{acc}", beans[0]));
                cx.notify()
            });
        })
        .detach();
    }

    /// "◇ N xhtml" after every member of a bean that pages use, and a "◇ 0 xhtml" after public
    /// getters/actions no page uses (IntelliJ's JSF usage gutter).
    pub(crate) fn jsf_lens(&mut self, t: Entity<EditorView>, cx: &mut Context<Self>) {
        let Some((text, file)) = Self::tab_text(&t, cx) else { return };
        if file.extension().is_none_or(|e| e != "java") || !(text.contains("@Named") || text.contains("@ManagedBean") || text.contains("@Component") || text.contains("@Controller")) {
            return;
        }
        let Some(project) = self.project.clone() else { return };
        cx.spawn(async move |this, cx| {
            let Some(index) = Self::jsf_index(this.clone(), cx).await else { return };
            let beans: Vec<String> = index.by_file(&file).into_iter().map(|b| b.name.clone()).collect();
            if beans.is_empty() {
                return;
            }
            let lens = cx
                .background_executor()
                .spawn(async move {
                    let mut usage: std::collections::BTreeMap<String, usize> = Default::default();
                    for f in jsf::beans::xhtml_files(&project) {
                        let Ok(page) = std::fs::read_to_string(&f) else { continue };
                        for b in &beans {
                            for (m, n) in jsf::refactor::member_usages(&page, b) {
                                *usage.entry(m).or_insert(0) += n;
                            }
                        }
                    }
                    let mut out = Vec::new();
                    for (i, line) in text.lines().enumerate() {
                        let tl = line.trim_start();
                        if !tl.starts_with("public ") || tl.contains(" class ") || tl.contains(" static ") {
                            continue;
                        }
                        let Some(d) = line.find('(') else { continue };
                        let head = line[..d].trim_end();
                        let name: String = head.chars().rev().take_while(|c| c.is_alphanumeric() || *c == '_').collect::<Vec<_>>().into_iter().rev().collect();
                        if name.is_empty() || name.starts_with("set") && name.chars().nth(3).is_some_and(char::is_uppercase) {
                            continue;
                        }
                        // constructors have no return type: "public UserBean("
                        if head.split_whitespace().count() < 3 && !head.contains('<') {
                            continue;
                        }
                        let (prop, is_acc) = el::property_name_for(&name);
                        let n = usage.get(if is_acc { &prop } else { &name }).copied().unwrap_or(0);
                        out.push((i, format!("◇ {n} xhtml")));
                    }
                    out
                })
                .await;
            let _ = t.update(cx, |v, cx| {
                v.lens = lens;
                cx.notify()
            });
        })
        .detach();
    }

    /// Page Map: which page uses which template/include/composite, and pages nothing references.
    pub(crate) fn jsf_page_map(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.project.clone() else {
            self.message = Some("Chưa có Project Model".into());
            return cx.notify();
        };
        cx.spawn_in(window, async move |this, cx| {
            let pages = cx
                .background_executor()
                .spawn(async move {
                    let roots = jsf::beans::webapp_roots(&project, None);
                    let files = jsf::beans::xhtml_files(&project);
                    jsf::refactor::page_map(&roots, &files)
                })
                .await;
            let _ = this.update_in(cx, |ws, window, cx| {
                if pages.is_empty() {
                    ws.message = Some("Không có trang .xhtml nào".into());
                    return cx.notify();
                }
                let mut hits = Vec::new();
                for p in &pages {
                    let tag = if p.used || p.rel.starts_with("/resources/") { "" } else { "   (không trang nào tham chiếu)" };
                    hits.push(crate::search::Hit { path: p.file.clone(), line: 0, col: 0, text: format!("{}{tag}", p.rel), mark: Some((0, p.rel.len())) });
                    for (kind, target, resolved) in &p.refs {
                        let missing = if resolved.is_none() { "   ✗ không tồn tại" } else { "" };
                        hits.push(crate::search::Hit { path: resolved.clone().unwrap_or_else(|| p.file.clone()), line: 0, col: 0, text: format!("    {kind:<9} → {target}{missing}"), mark: None });
                    }
                }
                ws.open_list(format!("JSF Page Map ({} trang)", pages.len()), hits, window, cx);
            });
        })
        .detach();
    }

    /// Writes the generated files (never overwriting); returns the last one to open.
    pub(crate) fn jsf_generate(&mut self, entity: &std::path::Path, pkg: &str, crud: bool, _cx: &mut Context<Self>) -> Result<Option<PathBuf>, String> {
        let text = std::fs::read_to_string(entity).map_err(|e| e.to_string())?;
        let e = jsf::generate::parse_entity(&text).ok_or("Không đọc được @Entity")?;
        let s = entity.display().to_string();
        let src_root = PathBuf::from(&s[..s.find("/src/main/java/").ok_or("File không nằm trong src/main/java")? + "/src/main/java".len()]);
        let project = self.project.clone().ok_or("Chưa có Project Model")?;
        // an existing <Entity>Repository anywhere in the project is reused
        let repo_name = format!("{}Repository.java", e.class_name);
        let repo_fqn = spring::java_files(&project).into_iter().find(|f| f.file_name().is_some_and(|n| n.to_string_lossy() == repo_name)).and_then(|f| {
            let t = std::fs::read_to_string(&f).ok()?;
            let pkg = t.lines().find_map(|l| l.trim().strip_prefix("package ").map(|p| p.trim_end_matches(';').trim().to_string()))?;
            Some(format!("{pkg}.{}Repository", e.class_name))
        });
        let poms: Vec<PathBuf> = project.modules.iter().map(|m| PathBuf::from(&m.content_root).join("pom.xml")).collect();
        let ns = jsf::generate::ee_namespace(&poms);
        let webroot = jsf::beans::webapp_roots(&project, Some(entity)).into_iter().next();
        let files = jsf::generate::generate(&e, pkg, &src_root, repo_fqn.as_deref(), ns, crud, webroot.as_deref());
        let (mut created, mut skipped) = (Vec::new(), Vec::new());
        for (path, content) in files {
            if path.exists() {
                skipped.push(crate::tree::file_name(&path));
                continue;
            }
            if let Some(d) = path.parent() {
                std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
            }
            std::fs::write(&path, content).map_err(|e| e.to_string())?;
            created.push(path);
        }
        let changes: Vec<_> = created.iter().map(|p| (p.clone(), crate::lsp_workspace::FileChange::Created)).collect();
        self.notify_files(&changes);
        self.jsf_index = None;
        self.message = Some(format!(
            "Đã tạo {} file{}",
            created.len(),
            if skipped.is_empty() { String::new() } else { format!(" (bỏ qua, đã có: {})", skipped.join(", ")) }
        ));
        Ok(created.last().cloned())
    }

    /// Moves the selected lines of a page into resources/<lib>/<name>.xhtml and puts the tag
    /// there instead.
    pub(crate) fn jsf_extract(&mut self, page: &std::path::Path, start: usize, end: usize, name: &str, cx: &mut Context<Self>) -> Result<Option<PathBuf>, String> {
        let t = self.tabs.iter().find(|t| t.read(cx).path().map(|p| p.as_path()) == Some(page)).cloned().ok_or("Trang không mở")?;
        let text = t.read(cx).editor.buffer.text();
        let s = page.display().to_string();
        let webroot = ["/src/main/webapp/", "/META-INF/resources/"]
            .iter()
            .find_map(|m| s.find(m).map(|i| PathBuf::from(&s[..i + m.len() - 1])))
            .ok_or("Trang không nằm trong src/main/webapp hay META-INF/resources")?;
        let (path, comp, replacement, decl) = jsf::generate::extract_component(&text, &text[start..end], &webroot, name);
        if path.exists() {
            return Err(format!("{} đã tồn tại", path.display()));
        }
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, comp).map_err(|e| e.to_string())?;
        // the root element (first tag that is not <?xml / <!DOCTYPE / a comment)
        let root_name_end = decl.as_ref().and_then(|_| {
            let mut i = 0;
            while let Some(k) = text[i..].find('<').map(|k| i + k) {
                let rest = &text[k + 1..];
                if rest.starts_with(['?', '!']) {
                    i = k + 1;
                    continue;
                }
                return Some(k + 1 + rest.find(|c: char| c.is_whitespace() || c == '>' || c == '/').unwrap_or(0));
            }
            None
        });
        t.update(cx, |v, cx| {
            v.editor.buffer.break_group();
            v.editor.anchor = start;
            v.editor.head = end;
            v.editor.insert(&replacement);
            if let (Some(at), Some(d)) = (root_name_end.filter(|a| *a < start), decl.as_ref()) {
                v.editor.set_cursor(at, false);
                v.editor.insert(d);
            }
            v.editor.buffer.break_group();
            cx.notify()
        });
        self.notify_files(&[(path.clone(), crate::lsp_workspace::FileChange::Created)]);
        self.message = Some(format!("Đã tách thành {} - khai báo <composite:attribute> cho giá trị cần truyền vào", crate::tree::file_name(&path)));
        Ok(Some(path))
    }

    /// Re-checks every open .xhtml (jdtls just became ready, or the beans changed).
    pub(crate) fn jsf_check_all(&mut self, cx: &mut Context<Self>) {
        for t in self.tabs.clone() {
            self.jsf_check(t.clone(), cx);
            self.jsf_lens(t, cx);
        }
    }

    fn tab_text(t: &Entity<EditorView>, cx: &Context<Self>) -> Option<(String, PathBuf)> {
        let v = t.read(cx);
        Some((v.editor.buffer.text(), v.path()?.clone()))
    }

    /// Ctrl+B in an .xhtml page.
    pub(crate) fn jsf_goto(&mut self, t: Entity<EditorView>, at: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((text, path)) = Self::tab_text(&t, cx) else { return };
        let Some(project) = self.project.clone() else {
            self.message = Some("Chưa có Project Model".into());
            return cx.notify();
        };
        if let Some(rel) = el::parse_include_path(&text, at) {
            match jsf::beans::resolve_include(&project, &path, &rel) {
                Some(f) => {
                    self.push_nav(cx);
                    self.open_file(f, window, cx)
                }
                None => self.message = Some(format!("Không tìm thấy file: {rel}")),
            }
            return cx.notify();
        }
        if let Some((lib, tag)) = el::parse_composite_tag(&text, at, &el::composite_namespaces(&text)) {
            match jsf::beans::resolve_composite(&project, &path, &lib, &tag) {
                Some(f) => {
                    self.push_nav(cx);
                    self.open_file(f, window, cx)
                }
                None => self.message = Some(format!("Không tìm thấy composite component resources/{lib}/{tag}.xhtml")),
            }
            return cx.notify();
        }
        let Some(r) = el::parse_el(&text, at) else {
            self.message = Some("Không có gì để nhảy tới (EL #{…}, include/template, composite tag)".into());
            return cx.notify();
        };
        let Some(client) = self.lsp.clone() else {
            self.message = Some("jdtls chưa chạy - cần để resolve EL".into());
            return cx.notify();
        };
        self.message = Some(format!("Đang tìm {}…", r.member.as_deref().unwrap_or(&r.bean)));
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let Some(index) = Self::jsf_index(this.clone(), cx).await else { return };
            let mut ctx = Ctx::new(&client, &index, &text, &path);
            let res = ctx.resolve(&r, at).await;
            let _ = this.update_in(cx, |ws, window, cx| {
                ws.message = None;
                match res {
                    Ok(Target::Java(loc)) => {
                        ws.push_nav(cx);
                        ws.open_location(loc, window, cx);
                    }
                    Ok(Target::PageVar(off)) => t.update(cx, |v, cx| {
                        v.editor.set_cursor(off, false);
                        cx.notify()
                    }),
                    Err(e) => ws.message = Some(format!("JSF: {e}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Completion in an .xhtml page; the editor's popup is open and waiting.
    pub(crate) fn jsf_complete(&mut self, t: Entity<EditorView>, at: usize, explicit: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some((text, path)) = Self::tab_text(&t, cx) else { return };
        let Some(cc) = el::completion_context(&text, at) else {
            // not in EL / include / composite: attribute names of the tag being written
            if let Some(jsf::tags::TagSite::AttrName { prefix, tag, start, present }) = jsf::tags::tag_site(&text, at) {
                cx.spawn_in(window, async move |this, cx| {
                    let libs = Self::jsf_taglibs(this.clone(), cx).await;
                    let ns = jsf::tags::prefixes(&text).get(&prefix).cloned().unwrap_or_default();
                    let items = libs.get(&ns).and_then(|l| l.get(&tag)).map(|d| attr_items(d, &present)).unwrap_or_default();
                    let _ = t.update(cx, |v, cx| v.set_jsf_completion(at, start, items, explicit, cx));
                })
                .detach();
                return;
            }
            return t.update(cx, |v, cx| v.set_jsf_completion(at, at, Vec::new(), explicit, cx));
        };
        let project = self.project.clone();
        let client = self.lsp.clone();
        cx.spawn_in(window, async move |this, cx| {
            let items: Vec<Value> = match &cc.kind {
                CompletionKind::Include => project.map(|p| include_items(&p, &path)).unwrap_or_default(),
                CompletionKind::Composite(prefix) => {
                    // composite components of the page's libraries + tags of taglibs (p:, h:, f:…)
                    let mut items = project.map(|p| composite_items(&p, &path, &text, prefix)).unwrap_or_default();
                    let libs = Self::jsf_taglibs(this.clone(), cx).await;
                    if let Some(lib) = jsf::tags::prefixes(&text).get(prefix).and_then(|ns| libs.get(ns)) {
                        items.extend(lib.values().map(tag_item));
                    }
                    items
                }
                CompletionKind::Head => match Self::jsf_index(this.clone(), cx).await {
                    Some(index) => jsf::resolve::head_items(&index, &text, at),
                    None => Vec::new(),
                },
                CompletionKind::Member(r) => match (client, Self::jsf_index(this.clone(), cx).await) {
                    (Some(client), Some(index)) => {
                        let mut ctx = Ctx::new(&client, &index, &text, &path);
                        match ctx.type_of(r, cc.typed_s).await {
                            Ok(ty) => match ctx.members_of(&ty).await {
                                Ok(m) => jsf::resolve::member_items(&m),
                                Err(_) => Vec::new(),
                            },
                            Err(e) => {
                                let _ = this.update(cx, |ws, cx| {
                                    if explicit {
                                        ws.message = Some(format!("JSF: {e}"));
                                        cx.notify()
                                    }
                                });
                                Vec::new()
                            }
                        }
                    }
                    _ => Vec::new(),
                },
            };
            let _ = t.update(cx, |v, cx| v.set_jsf_completion(at, cc.typed_s, items, explicit, cx));
        })
        .detach();
    }

    /// Ctrl+Q in an .xhtml page: the Java declaration behind the EL segment.
    pub(crate) fn jsf_hover(&mut self, t: Entity<EditorView>, at: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((text, path)) = Self::tab_text(&t, cx) else { return };
        let (Some(r), Some(client)) = (el::parse_el(&text, at), self.lsp.clone()) else {
            return t.update(cx, |v, cx| {
                v.message = Some("Không có tài liệu".into());
                cx.notify()
            });
        };
        cx.spawn_in(window, async move |this, cx| {
            let Some(index) = Self::jsf_index(this.clone(), cx).await else { return };
            let mut ctx = Ctx::new(&client, &index, &text, &path);
            let res = ctx.hover(&r, at).await;
            let _ = t.update(cx, |v, cx| {
                match res {
                    Ok(h) => v.hover = Some(h),
                    Err(e) => v.message = Some(format!("JSF: {e}")),
                }
                cx.notify()
            });
        })
        .detach();
    }

    /// Find Usages / Ctrl+B on a declaration in Java: jdtls' references plus the .xhtml EL
    /// usages when the file is a JSF/Spring bean (or by property name for other classes).
    pub(crate) fn usages_with_xhtml(&mut self, locs: Vec<Value>, uri: String, line: usize, col: usize, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(client), Some(project)) = (self.lsp.clone(), self.project.clone()) else {
            return self.show_usages(locs, Vec::new(), String::new(), window, cx);
        };
        // a library class (jdt://…) is never a bean of this project: jdtls' references only
        let Some(file) = lsp_client::uri_to_path(&uri) else {
            return self.show_usages(locs, Vec::new(), String::new(), window, cx);
        };
        cx.spawn_in(window, async move |this, cx| {
            let index = Self::jsf_index(this.clone(), cx).await;
            let syms = client.request("textDocument/documentSymbol", json!({ "textDocument": { "uri": uri } })).response().await.unwrap_or(Value::Null);
            let syms = jsf::resolve::flatten_symbols(&syms);
            let member = jsf::resolve::member_at(&syms, line, col);
            let beans: Vec<String> = index.as_ref().map(|i| i.by_file(&file).into_iter().map(|b| b.name.clone()).collect()).unwrap_or_default();
            let prop = member.as_deref().map(|m| el::property_name_for(m).0);
            let is_accessor = member.as_deref().is_some_and(|m| el::property_name_for(m).1 && !m.starts_with("set"));
            // a non-bean class is only reachable through a chain (#{x.address.city}), so search it
            // by name for properties (getters) only - an action method there is a bean's own
            let searches: Vec<(Option<String>, Option<String>)> = if !beans.is_empty() {
                beans.iter().map(|b| (Some(b.clone()), prop.clone())).collect()
            } else if is_accessor {
                vec![(None, prop.clone())]
            } else {
                vec![]
            };
            let all_beans: Vec<String> = index.as_ref().map(|i| i.candidates.keys().cloned().collect()).unwrap_or_default();
            let label = searches.iter().map(|(b, p)| format!("{}{}", b.as_deref().unwrap_or("*"), p.as_deref().map(|p| format!(".{p}")).unwrap_or_default())).collect::<Vec<_>>().join(", ");
            let hits = cx
                .background_executor()
                .spawn(async move {
                    if searches.is_empty() {
                        return Vec::new();
                    }
                    let files = jsf::beans::xhtml_files(&project);
                    let mut all = Vec::new();
                    for (b, p) in &searches {
                        all.extend(jsf::beans::find_usages(&files, b.as_deref(), p.as_deref(), &all_beans));
                    }
                    all
                })
                .await;
            let _ = this.update_in(cx, |ws, window, cx| ws.show_usages(locs, hits, label, window, cx));
        })
        .detach();
    }

    fn show_usages(&mut self, mut locs: Vec<Value>, xhtml: Vec<jsf::beans::Usage>, label: String, window: &mut Window, cx: &mut Context<Self>) {
        let n_xhtml = xhtml.len();
        for (path, line, col, _) in xhtml {
            let line_text = std::fs::read_to_string(&path).ok().and_then(|s| s.lines().nth(line).map(String::from)).unwrap_or_default();
            let ch = lsp_client::text::byte_to_utf16_col(&line_text, col.min(line_text.len()));
            locs.push(json!({ "uri": lsp_client::path_to_uri(&path), "range": { "start": { "line": line, "character": ch }, "end": { "line": line, "character": ch } } }));
        }
        if locs.is_empty() {
            self.message = Some("Không có chỗ dùng nào".into());
            return cx.notify();
        }
        let title = if n_xhtml > 0 { format!("Chỗ dùng (Java + {n_xhtml} trong .xhtml: {label})") } else { "Chỗ dùng".to_string() };
        self.show_locations(&title, &locs, window, cx);
    }
}

fn first_sentence(s: &str) -> String {
    let s = s.split(". ").next().unwrap_or(s);
    s.chars().take(90).collect()
}

fn tag_item(d: &jsf::tags::TagDef) -> Value {
    json!({ "label": d.name, "kind": 10, "detail": first_sentence(&d.description), "documentation": d.description, "data": { "local": true } })
}

/// Attributes of a tag not written yet; required ones first. Inserts `name=""`, caret inside.
fn attr_items(d: &jsf::tags::TagDef, present: &[String]) -> Vec<Value> {
    d.attrs
        .iter()
        .filter(|a| !present.iter().any(|p| *p == a.name))
        .map(|a| {
            let ty = a.ty.rsplit('.').next().unwrap_or("");
            let detail = match (a.required, ty.is_empty()) {
                (true, false) => format!("{ty} · bắt buộc"),
                (true, true) => "bắt buộc".to_string(),
                (false, _) => ty.to_string(),
            };
            json!({
                "label": a.name,
                "filterText": a.name,
                "insertText": format!("{}=\"\"", a.name),
                "kind": 5,
                "detail": detail,
                "documentation": a.description,
                "sortText": format!("{}{}", if a.required { 0 } else { 1 }, a.name),
                "data": { "local": true, "caretBack": 1 },
            })
        })
        .collect()
}

/// .xhtml paths for include/template: webapp-absolute ("/WEB-INF/x.xhtml") plus paths
/// relative to the page for files below its folder.
fn include_items(project: &project_model::Project, file: &std::path::Path) -> Vec<Value> {
    let here = file.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let mut items = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for root in jsf::beans::webapp_roots(project, Some(file)) {
        let mut files = Vec::new();
        walk_xhtml(&root, &mut files);
        for f in files.into_iter().filter(|f| f != file) {
            let mut paths = vec![format!("/{}", f.strip_prefix(&root).unwrap_or(&f).display())];
            if let Ok(rel) = f.strip_prefix(&here) {
                paths.push(rel.display().to_string());
            }
            for p in paths {
                if seen.insert(p.clone()) {
                    let sort = format!("{}{p}", if p.starts_with('/') { 1 } else { 0 });
                    items.push(json!({ "label": p, "kind": 17, "insertText": p, "filterText": p, "sortText": sort }));
                }
            }
        }
    }
    items
}

fn walk_xhtml(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk_xhtml(&p, out);
        } else if p.extension().is_some_and(|x| x == "xhtml") {
            out.push(p);
        }
    }
}

fn composite_items(project: &project_model::Project, file: &std::path::Path, text: &str, prefix: &str) -> Vec<Value> {
    let Some(lib) = el::composite_namespaces(text).get(prefix).cloned() else { return Vec::new() };
    let mut items = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for root in jsf::beans::webapp_roots(project, Some(file)) {
        let Ok(rd) = std::fs::read_dir(root.join("resources").join(&lib)) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "xhtml") {
                let tag = p.file_stem().unwrap_or_default().to_string_lossy().to_string();
                if seen.insert(tag.clone()) {
                    items.push(json!({ "label": tag, "kind": 9, "detail": format!("resources/{lib}/{tag}.xhtml") }));
                }
            }
        }
    }
    items
}
