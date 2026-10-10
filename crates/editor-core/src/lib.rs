//! UI-independent editor core: rope buffer, undo, grapheme-aware movement, IME
//! composition and the IntelliJ-style command set.
pub mod buffer;
pub mod editor;
pub mod vim;

pub use buffer::{Buffer, TextChange};
pub use editor::{intellij_keymap, Command, Editor, Effect};

#[cfg(test)]
mod tests {
    use super::*;
    use Command::*;

    fn ed(text: &str) -> Editor {
        Editor::new(Buffer::from_text(text))
    }

    #[test]
    fn vietnamese_graphemes_move_as_one() {
        // "ệ" written decomposed: e + U+0323 + U+0302 - three chars, one grapheme, one column
        let mut e = ed("Vie\u{323}\u{302}t");
        e.run(DocEnd);
        e.run(Left);
        e.run(Left);
        assert_eq!(e.head, 2);
        assert_eq!(e.buffer.display_col(e.buffer.len()), 4);
        e.run(Backspace);
        assert_eq!(e.buffer.text(), "Ve\u{323}\u{302}t");
        // deleting forward removes the whole grapheme, never half a letter
        e.run(Delete);
        assert_eq!(e.buffer.text(), "Vt");
    }

    #[test]
    fn ime_preedit_then_commit() {
        // Telex "tieengs" → fcitx sends preedit updates, then a commit
        let mut e = ed("x ");
        e.run(DocEnd);
        for p in ["t", "ti", "tie", "tiê", "tiên", "tiến"] {
            e.set_marked_text(p);
        }
        assert_eq!(e.buffer.text(), "x tiến");
        assert!(e.marked.is_some());
        e.commit_text("tiếng");
        assert_eq!(e.buffer.text(), "x tiếng");
        assert_eq!(e.marked, None);
        assert_eq!(e.head, e.buffer.len());
        // the whole composition undoes as one step
        e.run(Undo);
        assert_eq!(e.buffer.text(), "x ");
    }

    #[test]
    fn typing_groups_and_redo() {
        let mut e = ed("");
        for c in ["a", "b", "c"] {
            e.insert(c);
        }
        e.run(Newline);
        e.insert("d");
        assert_eq!(e.buffer.text(), "abc\nd");
        e.run(Undo);
        assert_eq!(e.buffer.text(), "abc\n");
        e.run(Undo);
        assert_eq!(e.buffer.text(), "abc");
        e.run(Undo);
        assert_eq!(e.buffer.text(), "");
        e.run(Redo);
        assert_eq!(e.buffer.text(), "abc");
    }

    #[test]
    fn vertical_keeps_column_and_newline_indents() {
        let mut e = ed("    foo();\nx\n    barbaz();");
        e.set_cursor(8, false); // after "    foo("
        e.run(Down);
        assert_eq!(e.head, 12); // end of "x"
        e.run(Down);
        assert_eq!(e.buffer.display_col(e.head), 8);
        e.run(LineEnd);
        e.run(Newline);
        assert!(e.buffer.text().ends_with("barbaz();\n    "));
    }

    #[test]
    fn home_toggles_first_nonblank_and_copy_line() {
        let mut e = ed("  ab\ncd");
        e.set_cursor(4, false);
        e.run(LineStart);
        assert_eq!(e.head, 2);
        e.run(LineStart);
        assert_eq!(e.head, 0);
        assert_eq!(e.run(Copy), Effect::CopyToClipboard("  ab\n".into()));
        e.run(DuplicateLine);
        assert_eq!(e.buffer.text(), "  ab\n  ab\ncd");
    }

    #[test]
    fn utf16_roundtrip_and_find() {
        let e = ed("aệ😀b");
        let b = &e.buffer;
        let end = b.len();
        assert_eq!(b.byte_to_utf16(end), 5); // a(1) ệ(1) 😀(2) b(1)
        assert_eq!(b.utf16_to_byte(5), end);
        assert_eq!(b.find("B", 0, false), Some(end - 1..end));
    }

    /// Replaying the recorded LSP changes on a copy must give the same text - this is
    /// exactly what the language server does with incremental didChange.
    #[test]
    fn lsp_changes_replay_to_same_text() {
        fn apply(text: &str, c: &TextChange) -> String {
            let to_byte = |(l, col): (usize, usize)| {
                let line_start: usize = text.split_inclusive('\n').take(l).map(str::len).sum();
                let line = text[line_start..].split('\n').next().unwrap_or("");
                line_start + lsp_client_like_col(line, col)
            };
            let (s, e) = (to_byte(c.start), to_byte(c.end));
            format!("{}{}{}", &text[..s], c.text, &text[e..])
        }
        fn lsp_client_like_col(line: &str, col: usize) -> usize {
            let mut u = 0;
            for (i, ch) in line.char_indices() {
                if u >= col {
                    return i;
                }
                u += ch.len_utf16();
            }
            line.len()
        }
        let start = "class A {\n  // 😀 tiếng\n}\n";
        let mut e = ed(start);
        e.buffer.set_track_changes(true);
        e.set_cursor(start.find("tiếng").unwrap(), false);
        e.set_marked_text("vi");
        e.commit_text("Việt ");
        e.run(Command::LineEnd);
        e.run(Command::Newline);
        e.insert("int x = 1;");
        e.run(Command::DuplicateLine);
        e.run(Command::Backspace);
        e.run(Command::Undo);
        let mut mirror = start.to_string();
        for c in e.buffer.take_changes() {
            mirror = apply(&mirror, &c);
        }
        assert_eq!(mirror, e.buffer.text());
        let b = &e.buffer;
        let at = b.text().find("tiếng").unwrap();
        let (l, c) = b.lsp_position(at);
        assert_eq!(b.byte_of_lsp_position(l, c), at);
    }

    #[test]
    fn reload_from_disk_is_clean_and_tracked() {
        let mut b = Buffer::from_text("a\nb\n");
        b.set_track_changes(true);
        b.reload_text("a\nB đổi\n");
        assert_eq!(b.text(), "a\nB đổi\n");
        assert!(!b.is_dirty());
        assert_eq!(b.take_changes().len(), 1);
    }

    #[test]
    fn comments_move_indent_join_case() {
        let mut e = ed("class A {\n    int a;\n    int b;\n}");
        e.set_cursor(15, false); // in "int a"
        e.run(CommentLine);
        assert_eq!(e.buffer.text(), "class A {\n    // int a;\n    int b;\n}");
        assert_eq!(e.buffer.line_of(e.head), 2); // caret moved to the next line
        e.run(Undo);
        assert_eq!(e.buffer.text(), "class A {\n    int a;\n    int b;\n}");
        // two lines selected: comment both, then uncomment both
        e.anchor = 12;
        e.head = e.buffer.line_end(2);
        e.run(CommentLine);
        assert_eq!(e.buffer.text(), "class A {\n    // int a;\n    // int b;\n}");
        e.run(CommentLine);
        assert_eq!(e.buffer.text(), "class A {\n    int a;\n    int b;\n}");
        // xml: no line comment → <!-- … --> per line
        let mut x = ed("<a/>\n");
        x.line_comment = None;
        x.block_comment = Some(("<!--".into(), "-->".into()));
        x.run(CommentLine);
        assert_eq!(x.buffer.text(), "<!-- <a/> -->\n");
        x.set_cursor(0, false);
        x.run(CommentLine);
        assert_eq!(x.buffer.text(), "<a/>\n");
        // block comment around a selection and back
        let mut b = ed("f(x);");
        b.anchor = 2;
        b.head = 3;
        b.run(CommentBlock);
        assert_eq!(b.buffer.text(), "f(/*x*/);");
        b.run(CommentBlock);
        assert_eq!(b.buffer.text(), "f(x);");
        // move line down / up keeps the caret on the moved line
        let mut m = ed("a\nb\nc");
        m.set_cursor(0, false);
        m.run(MoveLineDown);
        assert_eq!(m.buffer.text(), "b\na\nc");
        assert_eq!(m.buffer.line_of(m.head), 1);
        m.run(MoveLineUp);
        assert_eq!(m.buffer.text(), "a\nb\nc");
        // indent / unindent a multi-line selection
        let mut i = ed("x\ny\n");
        i.anchor = 0;
        i.head = 3;
        i.run(Tab);
        assert_eq!(i.buffer.text(), "    x\n    y\n");
        i.run(Unindent);
        assert_eq!(i.buffer.text(), "x\ny\n");
        // join lines
        let mut j = ed("foo(\n    a,\n    b)");
        j.set_cursor(0, false);
        j.run(JoinLines);
        assert_eq!(j.buffer.text(), "foo(a,\n    b)");
        // toggle case of the word at the caret
        let mut c = ed("int max = 1;");
        c.set_cursor(5, false);
        c.run(ToggleCase);
        assert_eq!(c.buffer.text(), "int MAX = 1;");
    }

    #[test]
    fn extend_and_shrink_selection() {
        let mut e = ed("call(foo(\"bar baz\"), x);");
        e.set_cursor(12, false); // in "bar"
        e.run(ExtendSelection);
        assert_eq!(e.buffer.slice(e.selection()), "bar");
        e.run(ExtendSelection);
        assert_eq!(e.buffer.slice(e.selection()), "bar baz");
        e.run(ExtendSelection);
        assert_eq!(e.buffer.slice(e.selection()), "\"bar baz\"");
        e.run(ExtendSelection);
        assert_eq!(e.buffer.slice(e.selection()), "(\"bar baz\")");
        e.run(ShrinkSelection);
        assert_eq!(e.buffer.slice(e.selection()), "\"bar baz\"");
    }

    #[test]
    fn folding_hides_lines_and_follows_edits() {
        let src = "import a;\nimport b;\nclass A {\n    void f() {\n        x();\n        y();\n    }\n    int z;\n}\n";
        let mut e = ed(src);
        let regions = editor::fold_regions(&e.buffer);
        assert!(regions.contains(&(3, 5)), "{regions:?}"); // method body
        assert!(regions.contains(&(0, 1))); // imports
        e.set_cursor(e.buffer.line_start(4), false);
        e.run(FoldCollapse);
        assert_eq!(e.buffer.folds, [(3, 5)]);
        assert_eq!(e.buffer.line_of(e.head), 3); // caret on the header
        assert!(e.buffer.is_hidden(4) && e.buffer.is_hidden(5) && !e.buffer.is_hidden(6));
        assert_eq!(e.buffer.row_of_line(6), 4);
        assert_eq!(e.buffer.line_of_row(4), 6);
        assert_eq!(e.buffer.visible_line_count(), e.buffer.line_count() - 2);
        // Down steps over the hidden lines
        e.run(Down);
        assert_eq!(e.buffer.line_of(e.head), 6);
        // an edit above shifts the fold
        e.set_cursor(0, false);
        e.insert("// c\n");
        assert_eq!(e.buffer.folds, [(4, 6)]);
        // moving the caret into the region opens it
        e.set_cursor(e.buffer.line_start(5), false);
        e.run(Left);
        e.run(Right);
        assert!(e.buffer.folds.is_empty());
        e.run(FoldAll);
        assert!(!e.buffer.folds.is_empty());
        e.run(UnfoldAll);
        assert!(e.buffer.folds.is_empty());
    }

    #[test]
    fn multiple_carets_edit_together() {
        let mut e = ed("let foo = foo + bar(foo);");
        e.set_cursor(5, false); // in the first "foo"
        e.run(AddNextOccurrence); // selects the word
        assert_eq!(e.buffer.slice(e.selection()), "foo");
        e.run(AddNextOccurrence);
        e.run(AddNextOccurrence);
        assert_eq!(e.extra.len(), 2);
        e.commit_text("x");
        assert_eq!(e.buffer.text(), "let x = x + bar(x);");
        e.commit_text("y");
        assert_eq!(e.buffer.text(), "let xy = xy + bar(xy);");
        e.run(Backspace);
        e.run(Backspace);
        e.commit_text("n");
        assert_eq!(e.buffer.text(), "let n = n + bar(n);");
        // one undo step per multi-edit
        e.run(Undo);
        assert_eq!(e.buffer.text(), "let  =  + bar();");
        // select all occurrences, then a caret move drops the extras
        let mut a = ed("a.b a.b a.b");
        a.anchor = 0;
        a.head = 3;
        a.run(SelectAllOccurrences);
        assert_eq!(a.extra.len(), 2);
        a.paste("c");
        assert_eq!(a.buffer.text(), "c c c");
        a.run(Left);
        assert!(a.extra.is_empty());
    }

    #[test]
    fn save_is_atomic_and_clears_dirty() {
        let dir = std::env::temp_dir().join(format!("ec-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("A.java");
        std::fs::write(&p, "class A {}\n").unwrap();
        let mut e = Editor::new(Buffer::open(&p).unwrap());
        e.insert("// Tiếng Việt\n");
        assert!(e.buffer.is_dirty());
        e.buffer.save().unwrap();
        assert!(!e.buffer.is_dirty());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "// Tiếng Việt\nclass A {}\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
