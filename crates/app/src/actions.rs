//! gpui actions: one per `editor_core::Command`, bound from `intellij_keymap()`,
//! plus workspace-level actions.
use crate::editor_view::EditorView;
use editor_core::{Command, intellij_keymap};
use gpui::{App, Context, InteractiveElement, KeyBinding, Stateful, Div, actions};

macro_rules! editor_actions {
    ($($name:ident),* $(,)?) => {
        pub mod editor {
            gpui::actions!(editor, [$($name),*]);
        }

        pub fn bind_editor_keys(cx: &mut App) {
            for (keys, cmd) in intellij_keymap() {
                match cmd {
                    $(Command::$name => cx.bind_keys([KeyBinding::new(keys, editor::$name, Some("Editor"))]),)*
                }
            }
        }

        pub fn register_editor_actions(div: Stateful<Div>, cx: &mut Context<EditorView>) -> Stateful<Div> {
            div $(.on_action(cx.listener(|this, _: &editor::$name, window, cx| this.run(Command::$name, window, cx))))*
        }
    };
}

editor_actions!(
    Left, Right, Up, Down, WordLeft, WordRight, LineStart, LineEnd, DocStart, DocEnd, PageUp, PageDown,
    SelectLeft, SelectRight, SelectUp, SelectDown, SelectWordLeft, SelectWordRight, SelectLineStart,
    SelectLineEnd, SelectAll, Backspace, Delete, DeleteWordLeft, Newline, Tab, DuplicateLine, DeleteLine,
    Undo, Redo, Save, Copy, Cut, Paste, Find, FindNext, CommentLine, CommentBlock, MoveLineUp, MoveLineDown,
    Unindent, JoinLines, ToggleCase, ExtendSelection, ShrinkSelection, FoldCollapse, FoldExpand, FoldAll, UnfoldAll,
    AddNextOccurrence, RemoveLastOccurrence, SelectAllOccurrences, SingleCaret,
);

actions!(workspace, [CloseTab, NextTab, PrevTab, FocusTree, ReloadModel, GoToFile, FindInFiles, CloseFind, FindNextInBar, NavBack, NavForward, Problems, GotoSymbol,
    RunActive, DebugActive, StopSession, Resume, StepOver, StepInto, StepOut, HotSwap, TogglePanel, ToggleTerminal, EditConfigurations, EvaluateExpression, FindAction, SelectOpenedFile, SpringEndpoints, SpringBeans, GotoBean, JsfPageMap, OpenReplace, ReplaceNext, ReplaceAll, ReplaceInFiles, RecentFiles, SearchEverywhere,
    GitCommit, GitPush, GitPull, GitLog, GitAnnotate, GitRollbackHunk, GitFileHistory, GitDiffFile, VcsPopup,
    ToggleBookmark, ShowBookmarks, TodoList, LocalHistory, AttachJvm, PauseSession, RunHttpRequest, RunWithCoverage, HideCoverage, VimCmdRun, VimCmdCancel, GitPanelToggle, DiffNext, DiffPrev, CloseDiff,
    GitUp, GitDown, GitSelUp, GitSelDown, GitToggleStage, GitOpenSel, GitDiscardSel, GitCommitNow, DiffStageBlock, DiffRevertBlock, ConflictTakeOurs, ConflictTakeTheirs, ConflictUndo]);

pub fn bind_workspace_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-w", CloseTab, None),
        KeyBinding::new("ctrl-f4", CloseTab, None),
        KeyBinding::new("alt-right", NextTab, None),
        KeyBinding::new("alt-left", PrevTab, None),
        // quit is Ctrl+Shift+Q (shell.rs); Ctrl+Q is IntelliJ's Quick Documentation (hover)
        KeyBinding::new("ctrl-alt-left", NavBack, None),
        KeyBinding::new("ctrl-alt-right", NavForward, None),
        KeyBinding::new("alt-6", Problems, None),
        KeyBinding::new("ctrl-n", GotoSymbol, None),
        // IntelliJ run/debug keys
        KeyBinding::new("shift-f10", RunActive, None),
        KeyBinding::new("shift-f9", DebugActive, None),
        KeyBinding::new("ctrl-f2", StopSession, None),
        KeyBinding::new("f9", Resume, None),
        KeyBinding::new("f8", StepOver, None),
        KeyBinding::new("f7", StepInto, None),
        KeyBinding::new("shift-f8", StepOut, None),
        KeyBinding::new("ctrl-f9", HotSwap, None),
        KeyBinding::new("alt-4", TogglePanel, None),
        KeyBinding::new("alt-f12", ToggleTerminal, None),
        KeyBinding::new("alt-shift-f10", EditConfigurations, None),
        KeyBinding::new("alt-f8", EvaluateExpression, None),
        KeyBinding::new("alt-1", FocusTree, None),
        KeyBinding::new("alt-f1", SelectOpenedFile, None),
        KeyBinding::new("alt-8", SpringEndpoints, None),
        KeyBinding::new("ctrl-alt-shift-b", GotoBean, None),
        KeyBinding::new("ctrl-shift-o", ReloadModel, None),
        KeyBinding::new("ctrl-shift-n", GoToFile, None),
        // IntelliJ Find Action
        KeyBinding::new("ctrl-shift-a", FindAction, None),
        KeyBinding::new("ctrl-shift-f", FindInFiles, None),
        KeyBinding::new("escape", CloseFind, Some("FindBar")),
        KeyBinding::new("escape", CloseFind, Some("Editor")),
        KeyBinding::new("escape", editor::SingleCaret, Some("Editor && multicaret")),
        KeyBinding::new("enter", FindNextInBar, Some("FindBar")),
        KeyBinding::new("f3", FindNextInBar, Some("FindBar")),
        KeyBinding::new("ctrl-r", OpenReplace, Some("Editor")),
        KeyBinding::new("enter", ReplaceNext, Some("ReplaceBar")),
        KeyBinding::new("alt-a", ReplaceAll, Some("ReplaceBar")),
        KeyBinding::new("alt-a", ReplaceAll, Some("FindBar")),
        KeyBinding::new("escape", CloseFind, Some("ReplaceBar")),
        KeyBinding::new("ctrl-shift-r", ReplaceInFiles, None),
        KeyBinding::new("ctrl-e", RecentFiles, None),
        KeyBinding::new("ctrl-k", GitCommit, None),
        KeyBinding::new("f11", ToggleBookmark, Some("Editor")),
        KeyBinding::new("ctrl-enter", RunHttpRequest, Some("Editor")),
        KeyBinding::new("enter", VimCmdRun, Some("VimCmd")),
        KeyBinding::new("escape", VimCmdCancel, Some("VimCmd")),
        KeyBinding::new("shift-f11", ShowBookmarks, None),
        KeyBinding::new("ctrl-shift-k", GitPush, None),
        KeyBinding::new("ctrl-t", GitPull, None),
        KeyBinding::new("alt-9", GitPanelToggle, None),
        KeyBinding::new("f7", DiffNext, Some("DiffView")),
        KeyBinding::new("shift-f7", DiffPrev, Some("DiffView")),
        KeyBinding::new("escape", CloseDiff, Some("DiffView")),
        KeyBinding::new("ctrl-alt-a", DiffStageBlock, Some("DiffView")),
        KeyBinding::new("ctrl-alt-z", DiffRevertBlock, Some("DiffView")),
        KeyBinding::new("f7", DiffNext, Some("ConflictView")),
        KeyBinding::new("shift-f7", DiffPrev, Some("ConflictView")),
        KeyBinding::new("escape", CloseDiff, Some("ConflictView")),
        KeyBinding::new("alt-left", ConflictTakeOurs, Some("ConflictView")),
        KeyBinding::new("alt-right", ConflictTakeTheirs, Some("ConflictView")),
        KeyBinding::new("ctrl-z", ConflictUndo, Some("ConflictView")),
        KeyBinding::new("up", GitUp, Some("GitPanel")),
        KeyBinding::new("down", GitDown, Some("GitPanel")),
        KeyBinding::new("shift-up", GitSelUp, Some("GitPanel")),
        KeyBinding::new("shift-down", GitSelDown, Some("GitPanel")),
        KeyBinding::new("space", GitToggleStage, Some("GitPanel")),
        KeyBinding::new("enter", GitOpenSel, Some("GitPanel")),
        KeyBinding::new("delete", GitDiscardSel, Some("GitPanel")),
        KeyBinding::new("enter", GitCommitNow, Some("CommitBox")),
        KeyBinding::new("ctrl-enter", GitCommitNow, Some("CommitBox")),
        KeyBinding::new("up", GitUp, Some("GitLog")),
        KeyBinding::new("down", GitDown, Some("GitLog")),
        KeyBinding::new("escape", CloseDiff, Some("GitLog")),
        KeyBinding::new("alt-`", VcsPopup, None),
        KeyBinding::new("ctrl-alt-z", GitRollbackHunk, Some("Editor")),
    ]);
}
