//! LSP positions (line + UTF-16 column) ↔ byte offsets in a line of text.

/// Byte offset within `line` of UTF-16 column `character` (clamped to the line end).
pub fn utf16_col_to_byte(line: &str, character: usize) -> usize {
    let mut units = 0;
    for (i, ch) in line.char_indices() {
        if units >= character {
            return i;
        }
        units += ch.len_utf16();
    }
    line.len()
}

/// UTF-16 column of byte offset `byte` within `line`.
pub fn byte_to_utf16_col(line: &str, byte: usize) -> usize {
    line[..byte.min(line.len())].encode_utf16().count()
}

/// Applies LSP TextEdits (`[{range, newText}]`, all relative to `text`) to a string -
/// used for files a WorkspaceEdit touches that are not open in an editor.
pub fn apply_edits(text: &str, edits: &[serde_json::Value]) -> String {
    let line_starts: Vec<usize> = std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect();
    let offset = |p: &serde_json::Value| {
        let line = p["line"].as_u64().unwrap_or(0) as usize;
        let Some(&start) = line_starts.get(line) else { return text.len() };
        let end = text[start..].find('\n').map(|i| start + i).unwrap_or(text.len());
        let line_text = text[start..end].strip_suffix('\r').unwrap_or(&text[start..end]);
        start + utf16_col_to_byte(line_text, p["character"].as_u64().unwrap_or(0) as usize)
    };
    let mut resolved: Vec<(usize, usize, &str)> = edits
        .iter()
        .map(|e| (offset(&e["range"]["start"]), offset(&e["range"]["end"]), e["newText"].as_str().unwrap_or("")))
        .collect();
    resolved.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    let mut out = text.to_string();
    for (s, e, t) in resolved {
        out.replace_range(s..e.max(s), t);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_edits_bottom_up() {
        let text = "class Foo {\n  Foo f; // chào\n}\n";
        let pos = |l: u64, c: u64| serde_json::json!({ "line": l, "character": c });
        let edits = vec![
            serde_json::json!({ "range": { "start": pos(0, 6), "end": pos(0, 9) }, "newText": "Bar" }),
            serde_json::json!({ "range": { "start": pos(1, 2), "end": pos(1, 5) }, "newText": "Bar" }),
            serde_json::json!({ "range": { "start": pos(0, 0), "end": pos(0, 0) }, "newText": "import x.Y;\n" }),
        ];
        assert_eq!(apply_edits(text, &edits), "import x.Y;\nclass Bar {\n  Bar f; // chào\n}\n");
    }

    #[test]
    fn vietnamese_and_astral_columns() {
        let line = "a ệ 😀 b";
        // a(1) space(1) ệ(1) space(1) 😀(2) space(1) b
        assert_eq!(utf16_col_to_byte(line, 7), line.find('b').unwrap());
        assert_eq!(byte_to_utf16_col(line, line.find('b').unwrap()), 7);
        assert_eq!(utf16_col_to_byte(line, 999), line.len());
    }
}
