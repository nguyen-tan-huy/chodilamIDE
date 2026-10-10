//! Tree-sitter parsing + highlight spans. Languages for now: Java, XML (pom.xml,
//! JSF .xhtml). Highlight queries are the ones bundled with each grammar.
use std::ops::Range;
use std::path::Path;
use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Parser, Query, QueryCursor, Tree};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Java,
    Xml,
}

impl Lang {
    pub fn from_path(path: &Path) -> Option<Lang> {
        match path.extension()?.to_str()? {
            "java" => Some(Lang::Java),
            "xml" | "xhtml" | "pom" | "xsd" | "wsdl" | "jsf" => Some(Lang::Xml),
            _ => None,
        }
    }

    fn language(self) -> Language {
        match self {
            Lang::Java => tree_sitter_java::LANGUAGE.into(),
            Lang::Xml => tree_sitter_xml::LANGUAGE_XML.into(),
        }
    }

    fn highlights(self) -> &'static str {
        match self {
            Lang::Java => tree_sitter_java::HIGHLIGHTS_QUERY,
            Lang::Xml => tree_sitter_xml::XML_HIGHLIGHT_QUERY,
        }
    }
}

/// Coarse style classes the UI theme maps to colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Style {
    Keyword,
    Type,
    Function,
    String,
    Number,
    Comment,
    Constant,
    Attribute,
    Tag,
    Property,
    Operator,
    Punctuation,
}

fn style_for_capture(name: &str) -> Option<Style> {
    let head = name.split('.').next().unwrap_or(name);
    Some(match head {
        "keyword" | "boolean" | "include" | "conditional" | "repeat" | "exception" => Style::Keyword,
        "type" => Style::Type,
        "function" | "method" | "constructor" => Style::Function,
        "string" | "character" | "escape" | "markup" => Style::String,
        "number" | "float" => Style::Number,
        "comment" => Style::Comment,
        "constant" | "embedded" => Style::Constant,
        "attribute" | "label" => Style::Attribute,
        "tag" => Style::Tag,
        "property" | "field" => Style::Property,
        "operator" => Style::Operator,
        "punctuation" | "delimiter" => Style::Punctuation,
        "variable" if name == "variable.builtin" => Style::Keyword,
        _ => return None,
    })
}

pub struct Highlighter {
    lang: Lang,
    parser: Parser,
    query: Query,
    styles: Vec<Option<Style>>,
    tree: Option<Tree>,
    text: String,
    version: Option<usize>,
}

impl Highlighter {
    pub fn new(lang: Lang) -> Option<Self> {
        let language = lang.language();
        let mut parser = Parser::new();
        parser.set_language(&language).ok()?;
        let query = Query::new(&language, lang.highlights()).ok()?;
        let styles = query.capture_names().iter().map(|n| style_for_capture(n)).collect();
        Some(Self { lang, parser, query, styles, tree: None, text: String::new(), version: None })
    }

    pub fn lang(&self) -> Lang {
        self.lang
    }

    /// Re-parses when `version` changed. A full parse of a 5k-line Java file takes a
    /// few ms; incremental `Tree::edit` can come later if profiling asks for it.
    pub fn update(&mut self, version: usize, text: impl FnOnce() -> String) {
        if self.version == Some(version) {
            return;
        }
        self.text = text();
        self.tree = self.parser.parse(&self.text, None);
        self.version = Some(version);
    }

    /// Non-overlapping styled spans intersecting `range`, sorted by start.
    /// Later query patterns win over earlier ones (nvim-treesitter convention).
    pub fn spans(&self, range: Range<usize>) -> Vec<(Range<usize>, Style)> {
        let Some(tree) = &self.tree else { return Vec::new() };
        let range = range.start.min(self.text.len())..range.end.min(self.text.len());
        if range.is_empty() {
            return Vec::new();
        }
        let mut per_byte: Vec<Option<(usize, Style)>> = vec![None; range.len()];
        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(range.clone());
        let mut matches = cursor.matches(&self.query, tree.root_node(), self.text.as_bytes());
        while let Some(m) = matches.next() {
            for cap in m.captures {
                let Some(style) = self.styles[cap.index as usize] else { continue };
                let r = cap.node.byte_range();
                let s = r.start.max(range.start) - range.start;
                let e = r.end.min(range.end).saturating_sub(range.start);
                for slot in per_byte.iter_mut().take(e).skip(s) {
                    if slot.is_none_or(|(p, _)| m.pattern_index >= p) {
                        *slot = Some((m.pattern_index, style));
                    }
                }
            }
        }
        let mut out: Vec<(Range<usize>, Style)> = Vec::new();
        for (i, slot) in per_byte.iter().enumerate() {
            let Some((_, style)) = slot else { continue };
            let at = range.start + i;
            match out.last_mut() {
                Some((r, s)) if r.end == at && s == style => r.end = at + 1,
                _ => out.push((at..at + 1, *style)),
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style_at(h: &Highlighter, text: &str, word: &str) -> Option<Style> {
        let at = text.find(word).unwrap();
        h.spans(0..text.len()).into_iter().find(|(r, _)| r.contains(&at)).map(|(_, s)| s)
    }

    #[test]
    fn java_highlights() {
        let src = "package a;\n// Chú thích tiếng Việt\npublic class Foo { String s = \"xin chào\"; int n = 42; void run() {} }";
        let mut h = Highlighter::new(Lang::Java).unwrap();
        h.update(1, || src.to_string());
        assert_eq!(style_at(&h, src, "public"), Some(Style::Keyword));
        assert_eq!(style_at(&h, src, "// Chú"), Some(Style::Comment));
        assert_eq!(style_at(&h, src, "\"xin"), Some(Style::String));
        assert_eq!(style_at(&h, src, "42"), Some(Style::Number));
        assert_eq!(style_at(&h, src, "run"), Some(Style::Function));
        // spans never split a UTF-8 char
        for (r, _) in h.spans(0..src.len()) {
            assert!(src.is_char_boundary(r.start) && src.is_char_boundary(r.end));
        }
    }

    #[test]
    fn xml_highlights_pom_and_xhtml() {
        let src = "<project><artifactId>demo</artifactId></project>";
        let mut h = Highlighter::new(Lang::from_path(Path::new("pom.xml")).unwrap()).unwrap();
        h.update(1, || src.to_string());
        assert_eq!(style_at(&h, src, "project"), Some(Style::Tag));
        assert_eq!(Lang::from_path(Path::new("index.xhtml")), Some(Lang::Xml));
    }
}
