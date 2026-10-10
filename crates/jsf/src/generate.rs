//! JSF generators (port of java-debug-model's jsf/refactor.lua): an `@Entity` → a Spring Data
//! repository, a PrimeFaces `LazyDataModel`, a `@ViewScoped` list bean and `list.xhtml`; and
//! "extract composite component" from selected lines of a page. Pure: they return the files
//! to write, the caller writes them (never overwriting).
use crate::el::capitalize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Entity {
    pub package: String,
    pub class_name: String,
    pub id_type: String,
    pub id_name: String,
    /// (type, name) of the non-static private fields
    pub fields: Vec<(String, String)>,
}

pub fn parse_entity(text: &str) -> Option<Entity> {
    if !text.contains("@Entity") {
        return None;
    }
    let mut e = Entity::default();
    let mut id_next = false;
    for l in text.lines() {
        let t = l.trim();
        if e.package.is_empty() {
            if let Some(p) = t.strip_prefix("package ") {
                e.package = p.trim_end_matches(';').trim().to_string();
            }
        }
        if e.class_name.is_empty() && t.starts_with("public") {
            if let Some(i) = t.find(" class ") {
                e.class_name = t[i + 7..].split(|c: char| !(c.is_alphanumeric() || c == '_')).next().unwrap_or("").to_string();
            }
        }
        if t.starts_with("@Id") && !t.starts_with("@IdClass") || t.starts_with("@EmbeddedId") {
            id_next = true;
        }
        if let Some(rest) = t.strip_prefix("private ") {
            if rest.contains("static ") {
                continue;
            }
            let decl = rest.split(['=', ';']).next().unwrap_or("").trim();
            if let Some((ty, name)) = decl.rsplit_once(char::is_whitespace) {
                let (ty, name) = (ty.trim().to_string(), name.trim().to_string());
                if id_next {
                    e.id_type = ty.clone();
                    e.id_name = name.clone();
                    id_next = false;
                }
                e.fields.push((ty, name));
            }
        }
    }
    if e.id_type.is_empty() {
        e.id_type = "Long".into();
        e.id_name = "id".into();
    }
    (!e.class_name.is_empty()).then_some(e)
}

/// `jakarta` when any pom of the project already uses jakarta.faces, else `javax`.
pub fn ee_namespace(poms: &[PathBuf]) -> &'static str {
    let jakarta = poms.iter().any(|p| std::fs::read_to_string(p).is_ok_and(|s| s.contains("jakarta.faces")));
    if jakarta { "jakarta" } else { "javax" }
}

fn parent_pkg(pkg: &str) -> &str {
    pkg.rsplit_once('.').map_or(pkg, |(p, _)| p)
}

pub fn default_view_package(e: &Entity) -> String {
    format!("{}.view", parent_pkg(&e.package))
}

/// Files to create for `e` into view package `pkg` under `src_root` (…/src/main/java).
/// `repo_fqn`: an existing `<Entity>Repository` (fully qualified) or None to create one.
/// `webroot`: where list.xhtml goes (CRUD only).
pub fn generate(e: &Entity, pkg: &str, src_root: &Path, repo_fqn: Option<&str>, ns: &str, crud: bool, webroot: Option<&Path>) -> Vec<(PathBuf, String)> {
    let c = &e.class_name;
    let dir = src_root.join(pkg.replace('.', "/"));
    let mut out = Vec::new();
    let repo_pkg = format!("{}.repository", parent_pkg(&e.package));
    let repo_import = match repo_fqn {
        Some(f) => format!("import {f};"),
        None => {
            out.push((
                src_root.join(repo_pkg.replace('.', "/")).join(format!("{c}Repository.java")),
                format!(
                    "package {repo_pkg};\n\nimport org.springframework.data.jpa.repository.JpaRepository;\n\nimport {}.{c};\n\npublic interface {c}Repository extends JpaRepository<{c}, {}> {{\n}}\n",
                    e.package, e.id_type
                ),
            ));
            format!("import {repo_pkg}.{c}Repository;")
        }
    };
    let id_getter = format!("get{}", capitalize(&e.id_name));
    out.push((
        dir.join(format!("{c}LazyDataModel.java")),
        format!(
            r##"package {pkg};

import java.util.List;
import java.util.Map;

import org.primefaces.model.FilterMeta;
import org.primefaces.model.LazyDataModel;
import org.primefaces.model.SortMeta;
import org.springframework.data.domain.Page;
import org.springframework.data.domain.PageRequest;
import org.springframework.data.domain.Sort;

import {entity_pkg}.{c};
{repo_import}

/** Loads only the visible page of {c} rows (p:dataTable lazy="true"). */
public class {c}LazyDataModel extends LazyDataModel<{c}> {{

    private final transient {c}Repository repository;

    public {c}LazyDataModel({c}Repository repository) {{
        this.repository = repository;
    }}

    @Override
    public int count(Map<String, FilterMeta> filterBy) {{
        return (int) repository.count();
    }}

    @Override
    public List<{c}> load(int first, int pageSize, Map<String, SortMeta> sortBy,
            Map<String, FilterMeta> filterBy) {{
        Sort sort = Sort.unsorted();
        for (SortMeta s : sortBy.values()) {{
            sort = sort.and(Sort.by(s.getOrder().isAscending() ? Sort.Direction.ASC : Sort.Direction.DESC,
                    s.getField()));
        }}
        Page<{c}> page = repository.findAll(PageRequest.of(first / pageSize, pageSize, sort));
        setRowCount((int) page.getTotalElements());
        return page.getContent();
    }}

    @Override
    public String getRowKey({c} row) {{
        return String.valueOf(row.{id_getter}());
    }}
}}
"##,
            entity_pkg = e.package
        ),
    ));
    if !crud {
        return out;
    }
    let bean_class = format!("{c}ListBean");
    let bean_name = format!("{}{}ListBean", c[..1].to_lowercase(), &c[1..]);
    out.push((
        dir.join(format!("{bean_class}.java")),
        format!(
            r##"package {pkg};

import java.io.Serializable;

import {ns}.annotation.PostConstruct;
import {ns}.faces.view.ViewScoped;
import {ns}.inject.Named;

import {entity_pkg}.{c};
{repo_import}

@Named
@ViewScoped
public class {bean_class} implements Serializable {{

    private static final long serialVersionUID = 1L;

    private final transient {c}Repository repository;
    private {c}LazyDataModel model;
    private {c} selected;

    public {bean_class}({c}Repository repository) {{
        this.repository = repository;
    }}

    @PostConstruct
    public void init() {{
        model = new {c}LazyDataModel(repository);
    }}

    public void newItem() {{
        selected = new {c}();
    }}

    public void save() {{
        repository.save(selected);
    }}

    public void delete({c} item) {{
        repository.delete(item);
    }}

    public {c}LazyDataModel getModel() {{ return model; }}
    public {c} getSelected() {{ return selected; }}
    public void setSelected({c} selected) {{ this.selected = selected; }}
}}
"##,
            entity_pkg = e.package
        ),
    ));
    if let Some(web) = webroot {
        let simple = |t: &str| !t.contains('<') && !t.contains("List") && !t.contains("Set");
        let cols: String = e
            .fields
            .iter()
            .filter(|(t, _)| simple(t))
            .map(|(_, n)| format!("            <p:column headerText=\"{n}\" sortBy=\"#{{row.{n}}}\"><h:outputText value=\"#{{row.{n}}}\"/></p:column>\n"))
            .collect();
        let form: String = e
            .fields
            .iter()
            .filter(|(t, n)| simple(t) && *n != e.id_name)
            .map(|(_, n)| format!("                <p:outputLabel for=\"{n}\" value=\"{n}\"/><p:inputText id=\"{n}\" value=\"#{{{bean_name}.selected.{n}}}\"/>\n"))
            .collect();
        let (fl, h, core) = if ns == "jakarta" {
            ("jakarta.faces.facelets", "jakarta.faces.html", "jakarta.faces.core")
        } else {
            ("http://java.sun.com/jsf/facelets", "http://java.sun.com/jsf/html", "http://java.sun.com/jsf/core")
        };
        let page = format!(
            r##"<ui:composition xmlns="http://www.w3.org/1999/xhtml"
        xmlns:ui="{fl}"
        xmlns:h="{h}"
        xmlns:f="{core}"
        xmlns:p="http://primefaces.org/ui">
    <h:form id="listForm">
        <p:commandButton value="Thêm" action="#{{{bean_name}.newItem}}" update=":editForm" oncomplete="PF('editDlg').show()" process="@this"/>
        <p:dataTable id="table" value="#{{{bean_name}.model}}" var="row" lazy="true"
                paginator="true" rows="20" rowKey="#{{row.{id}}}">
{cols}        </p:dataTable>
    </h:form>
    <p:dialog widgetVar="editDlg" header="{c}" modal="true">
        <h:form id="editForm">
            <p:panelGrid columns="2">
{form}            </p:panelGrid>
            <p:commandButton value="Lưu" action="#{{{bean_name}.save}}" process="@form"
                    update=":listForm:table" oncomplete="PF('editDlg').hide()"/>
        </h:form>
    </p:dialog>
</ui:composition>
"##,
            id = e.id_name
        );
        out.push((web.join(c.to_lowercase()).join("list.xhtml"), page));
    }
    out
}

/// Extracts `lines` (the selected lines of a page) into `resources/<lib>/<name>.xhtml`.
/// Returns (component file, component text, replacement line for the selection, namespace
/// declaration to add to the page's root element when the page has no composite prefix yet).
pub fn extract_component(page_text: &str, lines: &str, webroot: &Path, name: &str) -> (PathBuf, String, String, Option<String>) {
    let ns = crate::el::composite_namespaces(page_text);
    let (prefix, lib, decl) = match ns.iter().next() {
        Some((p, l)) => (p.clone(), l.clone(), None),
        None => ("cc".to_string(), "component".to_string(), Some(" xmlns:cc=\"http://java.sun.com/jsf/composite/component\"".to_string())),
    };
    let indent = lines.lines().filter(|l| !l.trim().is_empty()).map(|l| l.len() - l.trim_start().len()).min().unwrap_or(0);
    let body: String = lines.lines().map(|l| format!("        {}\n", l.get(indent..).unwrap_or(l.trim_start()))).collect();
    let comp = format!(
        r##"<ui:component xmlns="http://www.w3.org/1999/xhtml"
        xmlns:ui="http://java.sun.com/jsf/facelets"
        xmlns:h="http://java.sun.com/jsf/html"
        xmlns:f="http://java.sun.com/jsf/core"
        xmlns:p="http://primefaces.org/ui"
        xmlns:composite="http://java.sun.com/jsf/composite">
    <composite:interface>
        <!-- <composite:attribute name="value" required="true"/> -->
    </composite:interface>
    <composite:implementation>
{body}    </composite:implementation>
</ui:component>
"##
    );
    let replacement = format!("{}<{prefix}:{name}/>", " ".repeat(indent));
    (webroot.join("resources").join(&lib).join(format!("{name}.xhtml")), comp, replacement, decl)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_to_crud() {
        let src = "package vn.demo.domain;\n\n@Entity\npublic class Product {\n    @Id\n    @GeneratedValue\n    private Long productId;\n    private String name;\n    private static final long X = 1;\n    private List<Tag> tags;\n}\n";
        let e = parse_entity(src).unwrap();
        assert_eq!((e.package.as_str(), e.class_name.as_str(), e.id_type.as_str(), e.id_name.as_str()), ("vn.demo.domain", "Product", "Long", "productId"));
        assert_eq!(e.fields.len(), 3);
        assert_eq!(default_view_package(&e), "vn.demo.view");
        let files = generate(&e, "vn.demo.view", Path::new("/p/src/main/java"), None, "jakarta", true, Some(Path::new("/p/src/main/webapp")));
        let paths: Vec<String> = files.iter().map(|(p, _)| p.display().to_string()).collect();
        assert_eq!(
            paths,
            [
                "/p/src/main/java/vn/demo/repository/ProductRepository.java",
                "/p/src/main/java/vn/demo/view/ProductLazyDataModel.java",
                "/p/src/main/java/vn/demo/view/ProductListBean.java",
                "/p/src/main/webapp/product/list.xhtml",
            ]
        );
        assert!(files[1].1.contains("row.getProductId()") && files[1].1.contains("import vn.demo.repository.ProductRepository;"));
        assert!(files[2].1.contains("import jakarta.faces.view.ViewScoped;"));
        let page = &files[3].1;
        assert!(page.contains("#{row.name}") && !page.contains("row.tags") && page.contains("#{productListBean.selected.name}") && !page.contains("selected.productId"));
        assert!(page.contains("jakarta.faces.html"));
        // an existing repository is imported, not recreated
        let files = generate(&e, "vn.demo.view", Path::new("/p/src/main/java"), Some("vn.demo.repo.ProductRepository"), "javax", false, None);
        assert_eq!(files.len(), 1);
        assert!(files[0].1.contains("import vn.demo.repo.ProductRepository;"));
    }

    #[test]
    fn extract() {
        let page = "<ui:composition xmlns:h=\"x\">\n    <h:panelGroup>\n      <h:outputText value=\"#{b.x}\"/>\n    </h:panelGroup>\n</ui:composition>\n";
        let sel = "    <h:panelGroup>\n      <h:outputText value=\"#{b.x}\"/>\n    </h:panelGroup>";
        let (path, comp, rep, decl) = extract_component(page, sel, Path::new("/w"), "card");
        assert_eq!(path, Path::new("/w/resources/component/card.xhtml"));
        assert!(comp.contains("        <h:panelGroup>\n          <h:outputText"));
        assert_eq!(rep, "    <cc:card/>");
        assert!(decl.is_some());
    }
}
