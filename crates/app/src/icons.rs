//! File and folder icons drawn from boxes (no image assets), in the spirit of IntelliJ's:
//! a Java file shows its kind (C class, I interface, E enum, @ annotation, R record) as a
//! colored badge, folders are folder shapes colored by role.
use crate::theme::c;
use crate::tree::Icon;
use gpui::{AnyElement, FontWeight, div, prelude::*, px};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum JavaKind {
    Class,
    AbstractClass,
    Interface,
    Enum,
    Annotation,
    Record,
}

/// The first top-level type declared in Java source.
pub fn java_kind(src: &str) -> JavaKind {
    for line in src.lines() {
        let l = line.trim_start();
        if l.starts_with("//") || l.starts_with('*') || l.starts_with("/*") || l.starts_with("import ") || l.starts_with("package ") {
            continue;
        }
        let words: Vec<&str> = l.split(|ch: char| ch.is_whitespace() || ch == '{' || ch == '<').filter(|w| !w.is_empty()).collect();
        for (i, w) in words.iter().enumerate() {
            match *w {
                "@interface" => return JavaKind::Annotation,
                "interface" => return JavaKind::Interface,
                "enum" => return JavaKind::Enum,
                "record" if words.get(i + 1).is_some_and(|n| n.chars().next().is_some_and(char::is_uppercase)) => return JavaKind::Record,
                "class" => return if words[..i].contains(&"abstract") { JavaKind::AbstractClass } else { JavaKind::Class },
                _ => {}
            }
        }
    }
    JavaKind::Class
}

static CACHE: Mutex<Option<HashMap<PathBuf, (Option<SystemTime>, JavaKind)>>> = Mutex::new(None);

/// Kind of a Java file, cached by modification time (the tree re-renders often).
fn java_kind_of(path: &Path) -> JavaKind {
    let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let mut guard = CACHE.lock().unwrap();
    let cache = guard.get_or_insert_with(HashMap::new);
    if let Some((t, k)) = cache.get(path) {
        if *t == mtime {
            return *k;
        }
    }
    // the declaration is near the top: 16 KB is plenty
    let src = std::fs::read(path).map(|b| String::from_utf8_lossy(&b[..b.len().min(16 * 1024)]).to_string()).unwrap_or_default();
    let k = java_kind(&src);
    cache.insert(path.to_path_buf(), (mtime, k));
    k
}

const SIZE: f32 = 15.;

/// Round (or square) colored badge with a short white label.
fn badge(label: &'static str, bg: u32, round: bool) -> AnyElement {
    div()
        .flex_none()
        .size(px(SIZE))
        .flex()
        .items_center()
        .justify_center()
        .bg(c(bg))
        .when(round, |d| d.rounded_full())
        .when(!round, |d| d.rounded(px(3.)))
        .text_size(px(9.5))
        .font_weight(FontWeight::BOLD)
        .text_color(gpui::white())
        .line_height(px(SIZE))
        .child(label)
        .into_any_element()
}

/// Colored text glyph in the icon box (for file types).
fn glyph(label: &'static str, color: u32, size: f32) -> AnyElement {
    div()
        .flex_none()
        .w(px(SIZE))
        .flex()
        .justify_center()
        .text_size(px(size))
        .font_weight(FontWeight::BOLD)
        .text_color(c(color))
        .child(label)
        .into_any_element()
}

/// A folder shape; `mark` = a small square in the corner (module), `dot` = package dot.
fn folder(color: u32, mark: Option<u32>, dot: bool) -> AnyElement {
    div()
        .flex_none()
        .relative()
        .w(px(SIZE))
        .h(px(12.))
        .child(div().absolute().top_0().left_0().w(px(6.)).h(px(4.)).rounded_t(px(1.5)).bg(c(color)))
        .child(div().absolute().top(px(2.)).left_0().w(px(SIZE)).h(px(10.)).rounded(px(2.)).bg(c(color)))
        .when(dot, |d| d.child(div().absolute().top(px(5.)).left(px(5.5)).size(px(4.)).rounded_full().bg(gpui::white().opacity(0.85))))
        .when_some(mark, |d, m| d.child(div().absolute().bottom(px(-1.)).right(px(-1.)).size(px(6.)).rounded(px(1.)).bg(c(m)).border_1().border_color(c(crate::theme::PANEL_BG))))
        .into_any_element()
}

/// Icon for a file by name/extension (Java files by their declared kind).
pub fn file_icon(path: &Path) -> AnyElement {
    let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    if name == "pom.xml" {
        return badge("m", 0xc75450, false);
    }
    if name == "dockerfile" || name.starts_with("docker-compose") {
        return glyph("🐳", 0x2496ed, 11.);
    }
    if name == "mvnw" || name == "mvnw.cmd" {
        return glyph(">_", 0x8c8f96, 9.);
    }
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "java" => {
            let test = path.components().any(|c| c.as_os_str() == "test");
            match java_kind_of(path) {
                JavaKind::Interface => badge("I", 0x5fb865, true),
                JavaKind::Enum => badge("E", 0xc58c3c, true),
                JavaKind::Annotation => badge("@", 0x6aab73, true),
                JavaKind::Record => badge("R", 0x9876d6, true),
                JavaKind::AbstractClass => badge("C", 0x6b8fc7, true),
                JavaKind::Class if test => badge("C", 0x3f9e8f, true),
                JavaKind::Class => badge("C", 0x3574f0, true),
            }
        }
        "kt" | "kts" => badge("K", 0x9876d6, false),
        "xml" | "xsd" | "wsdl" => glyph("</>", 0xe08a3c, 8.5),
        "xhtml" | "html" | "htm" | "jsp" | "jsf" => glyph("</>", 0x3d8fd1, 8.5),
        "properties" | "yml" | "yaml" | "toml" | "ini" | "conf" | "cfg" => glyph("⚙", 0x6aab73, 12.),
        "json" => glyph("{}", 0xd5b778, 10.),
        "js" | "mjs" | "ts" => badge(if ext == "ts" { "TS" } else { "JS" }, if ext == "ts" { 0x3178c6 } else { 0xc9a22c }, false),
        "css" | "scss" | "less" => badge("#", 0x6c7fd8, false),
        "sql" => glyph("DB", 0xc77dbb, 8.5),
        "sh" | "bash" | "zsh" | "bat" | "cmd" | "ps1" => glyph(">_", 0x8c8f96, 9.),
        "md" | "adoc" => glyph("M↓", 0x6f9fd8, 8.5),
        "txt" | "log" => glyph("≡", 0x8c8f96, 12.),
        "jar" | "war" | "ear" | "zip" => badge("▤", 0xb3875a, false),
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "ico" | "webp" => glyph("▣", 0x5fb865, 11.),
        "gitignore" => glyph("◇", 0xe06c4c, 11.),
        _ if name.starts_with(".git") => glyph("◇", 0xe06c4c, 11.),
        _ => page(),
    }
}

/// Plain document shape.
fn page() -> AnyElement {
    div()
        .flex_none()
        .w(px(SIZE))
        .flex()
        .justify_center()
        .child(div().w(px(10.)).h(px(12.)).rounded(px(1.5)).border_1().border_color(c(0x8c8f96)))
        .into_any_element()
}

/// Icon for a project tree row.
pub fn tree_icon(icon: Icon, path: Option<&Path>) -> AnyElement {
    match icon {
        Icon::Java | Icon::Xml | Icon::File => match path {
            Some(p) => file_icon(p),
            None => page(),
        },
        Icon::Module => folder(0x6f8fb5, Some(0x3574f0), false),
        Icon::ModuleIndependent => folder(0x6f8fb5, Some(0x8c8f96), false),
        Icon::SourceMain => folder(0x3d8fd1, None, false),
        Icon::SourceTest => folder(0x4fa35a, None, false),
        Icon::Resources => folder(0xc9a22c, None, false),
        Icon::Package => folder(0x7f8b99, None, true),
        Icon::Folder => folder(0x8a93a0, None, false),
        Icon::Library => glyph("▥", 0xc77dbb, 12.),
        Icon::Jar => badge("▤", 0xb3875a, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn java_kinds() {
        assert_eq!(java_kind("package a;\nimport b.C;\n/** doc with class word */\npublic class A {}"), JavaKind::Class);
        assert_eq!(java_kind("public abstract class Base<T> {}"), JavaKind::AbstractClass);
        assert_eq!(java_kind("@FunctionalInterface\npublic interface Svc {}"), JavaKind::Interface);
        assert_eq!(java_kind("public enum ConfigType {\n A, B }"), JavaKind::Enum);
        assert_eq!(java_kind("@Retention(RUNTIME)\npublic @interface Mine {}"), JavaKind::Annotation);
        assert_eq!(java_kind("public record Point(int x, int y) {}"), JavaKind::Record);
    }
}
