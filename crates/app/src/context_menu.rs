//! Right-click menus (like IntelliJ's) for the editor, project tree, editor tabs, console
//! and Maven tree, plus the small name prompt used by New / Rename.
use crate::dialogs::{self, DialogCancel, DialogConfirm};
use crate::input::TextInput;
use crate::theme::{self, c};
use crate::tree::NodeId;
use crate::workspace::Workspace;
use gpui::{AnyElement, ClipboardItem, Context, Entity, Focusable, MouseButton, Pixels, Point, SharedString, Window, anchored, deferred, div, prelude::*, px};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub enum MenuAction {
    // editor
    GotoDefinition,
    FindUsages,
    Rename,
    ContextActions,
    Format,
    OrganizeImports,
    GotoBean,
    SpringEndpoints,
    JsfGenerate(PathBuf, bool),
    JsfExtract(PathBuf),
    JsfPageMap,
    RunCoverage(PathBuf),
    GitCommit,
    GitFetch,
    GitAccounts,
    GitPanel,
    GitPrompt(PromptKind),
    /// apply / pop / drop stash@{index}
    GitStash(&'static str, usize),
    GitPush,
    GitPull,
    GitLog,
    GitDiffAll,
    GitNewBranch,
    GitCheckout(String),
    GitRollbackHunk,
    GitRollbackFile(PathBuf),
    GitAnnotate,
    GitHistory(PathBuf),
    GitDiffFile(PathBuf),
    GitCompareWith(PathBuf),
    /// on the selection of the Git window's file list
    GitSelOpen,
    GitSelStage,
    GitSelUnstage,
    GitSelDiscard,
    GitForcePush,
    GitLogOp(crate::git_log::LogOp),
    LocalHistory,
    RestoreVersion(PathBuf),
    Cut,
    Copy,
    Paste,
    ToggleBreakpoint(usize),
    EditBreakpoint(usize),
    // files
    OpenFile(PathBuf),
    NewJavaClass(PathBuf),
    NewFile(PathBuf),
    NewDir(PathBuf),
    RenamePath(PathBuf),
    DeletePath(PathBuf),
    CopyPath(PathBuf),
    CopyRelativePath(PathBuf),
    RevealInFiles(PathBuf),
    RevealInTree(PathBuf),
    OpenTerminal(PathBuf),
    RunFile(PathBuf, bool),
    ToggleHidden,
    ReloadModel,
    /// the "Module Spring Boot mới" dialog (Maven window's + button)
    NewSpringModule,
    // dependencies
    DeleteJar(PathBuf),
    // tabs
    CloseTab(usize),
    CloseOthers(usize),
    CloseAll,
    // console
    CopyConsole,
    ClearConsole,
    ToggleWrap,
    // maven
    MavenPhase(String, String),
    DependencyTree(String),
    RemoveModule(String),
}

pub struct MenuItem {
    pub label: String,
    pub shortcut: &'static str,
    pub action: Option<MenuAction>,
}

fn item(label: impl Into<String>, shortcut: &'static str, action: MenuAction) -> MenuItem {
    MenuItem { label: label.into(), shortcut, action: Some(action) }
}

fn sep() -> MenuItem {
    MenuItem { label: String::new(), shortcut: "", action: None }
}

pub(crate) fn menu_item(label: impl Into<String>, shortcut: &'static str, action: MenuAction) -> MenuItem {
    item(label, shortcut, action)
}

pub(crate) fn menu_sep() -> MenuItem {
    sep()
}

pub struct ContextMenu {
    at: Point<Pixels>,
    items: Vec<MenuItem>,
}

#[derive(Clone, Debug)]
pub enum PromptKind {
    NewJavaClass(PathBuf),
    NewFile(PathBuf),
    NewDir(PathBuf),
    Rename(PathBuf),
    /// @Entity → LazyDataModel (+ list bean + list.xhtml when `crud`); input = view package
    JsfGenerate { entity: PathBuf, crud: bool },
    /// selected lines [start, end) of a page → composite component; input = component name
    JsfExtract { page: PathBuf, start: usize, end: usize },
    /// git checkout -b <name>
    NewBranch,
    /// host:port of a JVM listening for a debugger
    AttachJvm,
    /// git stash push -m <message>; true: also untracked files
    Stash(bool),
    /// new branch at a commit (and switch to it)
    BranchAt(String),
    TagAt(String),
    RenameBranch(String),
    MergeBranch,
    RebaseBranch,
    /// compare this file with another branch / commit
    CompareWith(PathBuf),
}

#[derive(Clone)]
pub enum ConfirmAction {
    DeletePath(PathBuf),
    DeleteJar(PathBuf),
    GitDiscard(Vec<vcs::Change>),
    GitResetHard(String),
    /// branch, forced (-D)
    GitDeleteBranch(String, bool),
    GitForcePush,
    /// undo the last commit although it is already on the remote
    GitUndoCommit,
}

impl ConfirmAction {
    fn ok_label(&self) -> &'static str {
        match self {
            ConfirmAction::GitDiscard(_) => "Hủy thay đổi (Enter)",
            ConfirmAction::GitResetHard(_) => "Reset --hard (Enter)",
            ConfirmAction::GitForcePush => "Force push (Enter)",
            ConfirmAction::GitUndoCommit => "Undo commit (Enter)",
            _ => "Xóa (Enter)",
        }
    }
}

pub struct Confirm {
    title: String,
    detail: String,
    action: ConfirmAction,
    focus: gpui::FocusHandle,
}

/// The local Maven repository (CHODILAMIDE_M2_REPO overrides it, for tests).
pub fn m2_repo() -> PathBuf {
    if let Some(p) = std::env::var_os("CHODILAMIDE_M2_REPO") {
        return PathBuf::from(p);
    }
    PathBuf::from(std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).unwrap_or_default()).join(".m2/repository")
}

/// The version folder holding `jar` (…/group/artifact/1.2.3/artifact-1.2.3.jar → …/1.2.3),
/// only if it lies inside `repo` and is at least group/artifact/version deep.
pub fn jar_version_dir(jar: &Path, repo: &Path) -> Option<PathBuf> {
    let dir = jar.parent()?;
    let rel = dir.strip_prefix(repo).ok()?;
    (rel.components().count() >= 3 && !rel.components().any(|c| matches!(c, std::path::Component::ParentDir))).then(|| dir.to_path_buf())
}

pub fn delete_jar(jar: &Path, repo: &Path) -> Result<PathBuf, String> {
    let dir = jar_version_dir(jar, repo).ok_or_else(|| format!("{} không nằm trong {}", jar.display(), repo.display()))?;
    std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(dir)
}

pub struct NamePrompt {
    kind: PromptKind,
    input: Entity<TextInput>,
    error: Option<String>,
}

impl Workspace {
    pub(crate) fn show_menu(&mut self, at: Point<Pixels>, items: Vec<MenuItem>, cx: &mut Context<Self>) {
        self.context_menu = Some(ContextMenu { at, items });
        cx.notify();
    }

    // ---- menu builders ----------------------------------------------------------

    pub(crate) fn editor_menu(&mut self, at: Point<Pixels>, line: usize, cx: &mut Context<Self>) {
        let Some(t) = self.active.and_then(|i| self.tabs.get(i)).cloned() else { return };
        let v = t.read(cx);
        let path = v.path().cloned();
        let has_bp = path.as_ref().is_some_and(|p| self.breakpoints.get(p).is_some_and(|l| l.contains(&line)));
        let mut items = vec![
            item("Go to Definition", "Ctrl+B", MenuAction::GotoDefinition),
            item("Find Usages", "Alt+F7", MenuAction::FindUsages),
            item("Show Context Actions", "Alt+Enter", MenuAction::ContextActions),
            sep(),
            item("Rename…", "Shift+F6", MenuAction::Rename),
            item("Reformat Code", "Ctrl+Alt+L", MenuAction::Format),
            item("Optimize Imports", "Ctrl+Alt+O", MenuAction::OrganizeImports),
            sep(),
            item("Cut", "Ctrl+X", MenuAction::Cut),
            item("Copy", "Ctrl+C", MenuAction::Copy),
            item("Paste", "Ctrl+V", MenuAction::Paste),
            sep(),
            item(if has_bp { "Remove Breakpoint" } else { "Toggle Breakpoint" }, "Ctrl+F8", MenuAction::ToggleBreakpoint(line)),
            item("Breakpoint Condition…", "Ctrl+Shift+F8", MenuAction::EditBreakpoint(line)),
        ];
        let text = v.editor.buffer.text();
        let spring_file = text.contains("org.springframework");
        if let Some(p) = path.as_ref().filter(|_| text.contains("@Entity")) {
            items.push(sep());
            items.push(item("JSF: Generate CRUD (bean + LazyDataModel + list.xhtml)…", "", MenuAction::JsfGenerate(p.clone(), true)));
            items.push(item("JSF: Generate LazyDataModel…", "", MenuAction::JsfGenerate(p.clone(), false)));
        }
        if let Some(p) = path.as_ref().filter(|p| p.extension().is_some_and(|e| e == "xhtml")) {
            items.push(sep());
            if v.editor.has_selection() {
                items.push(item("JSF: Extract Component…", "", MenuAction::JsfExtract(p.clone())));
            }
            items.push(item("JSF: Page Map", "", MenuAction::JsfPageMap));
        }
        if spring_file {
            items.push(sep());
            items.push(item("Spring: tới bean được inject", "Ctrl+Alt+Shift+B", MenuAction::GotoBean));
            items.push(item("Spring: Endpoints", "Alt+8", MenuAction::SpringEndpoints));
        }
        if let (Some(p), true) = (path.as_ref(), self.git.root.is_some()) {
            items.push(sep());
            if v.git_marks.iter().any(|(l, _)| *l == line) {
                items.push(item("Git: Rollback khối thay đổi này", "", MenuAction::GitRollbackHunk));
            }
            items.push(item("Git: So sánh với HEAD", "", MenuAction::GitDiffFile(p.clone())));
            items.push(item("Git: So sánh với nhánh / commit…", "", MenuAction::GitCompareWith(p.clone())));
            items.push(item(if v.annotations.is_empty() { "Git: Annotate (ai sửa dòng nào)" } else { "Git: Tắt Annotate" }, "", MenuAction::GitAnnotate));
            items.push(item("Git: History file này", "", MenuAction::GitHistory(p.clone())));
        }
        if let Some(p) = path.as_ref() {
            items.push(sep());
            if crate::code_tools::version_origin(p).is_some() {
                items.push(item("Khôi phục phiên bản này", "", MenuAction::RestoreVersion(p.clone())));
            }
            items.push(item("Local History…", "", MenuAction::LocalHistory));
        }
        if let Some(p) = path {
            items.extend(self.run_items(&p));
            items.push(sep());
            items.push(item("Hiện trong cây project", "Alt+F1", MenuAction::RevealInTree(p.clone())));
            items.push(item("Copy Path", "", MenuAction::CopyPath(p.clone())));
            items.push(item("Copy Relative Path", "", MenuAction::CopyRelativePath(p)));
        }
        self.show_menu(at, items, cx);
    }

    /// Run/Debug entries for a Java file with a main() or tests.
    fn run_items(&self, path: &Path) -> Vec<MenuItem> {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let runnable = path.extension().is_some_and(|e| e == "java") && (text.contains("static void main(") || text.contains("@Test"));
        if !runnable {
            return vec![];
        }
        let name = crate::tree::file_name(path).trim_end_matches(".java").to_string();
        vec![
            sep(),
            item(format!("▶ Run '{name}'"), "", MenuAction::RunFile(path.to_path_buf(), false)),
            item(format!("🐞 Debug '{name}'"), "", MenuAction::RunFile(path.to_path_buf(), true)),
            item(format!("☂ Run '{name}' with Coverage"), "", MenuAction::RunCoverage(path.to_path_buf())),
        ]
    }

    pub(crate) fn tree_menu(&mut self, at: Point<Pixels>, id: &NodeId, cx: &mut Context<Self>) {
        let hidden_label = if self.tree.show_hidden { "Ẩn file ẩn / target" } else { "Hiện file ẩn / target" };
        let items = match id {
            NodeId::File(p) => {
                let dir = p.parent().map(Path::to_path_buf).unwrap_or_default();
                let mut v = vec![item("Open", "Enter", MenuAction::OpenFile(p.clone()))];
                v.extend(self.run_items(p));
                v.extend([
                    sep(),
                    item("New Java Class…", "", MenuAction::NewJavaClass(dir.clone())),
                    item("New File…", "", MenuAction::NewFile(dir.clone())),
                    sep(),
                    item("Rename…", "Shift+F6", MenuAction::RenamePath(p.clone())),
                    item("Delete…", "Delete", MenuAction::DeletePath(p.clone())),
                    sep(),
                    item("Copy Path", "", MenuAction::CopyPath(p.clone())),
                    item("Copy Relative Path", "", MenuAction::CopyRelativePath(p.clone())),
                    item("Mở trong trình quản lý file", "", MenuAction::RevealInFiles(dir.clone())),
                    item("Mở terminal ở đây", "", MenuAction::OpenTerminal(dir)),
                ]);
                if let Some(st) = self.git.status.get(p).copied() {
                    v.push(sep());
                    v.push(item("Git: So sánh với HEAD", "", MenuAction::GitDiffFile(p.clone())));
                    if st != vcs::FileStatus::Untracked && st != vcs::FileStatus::Added {
                        v.push(item("Git: Rollback file về HEAD", "", MenuAction::GitRollbackFile(p.clone())));
                    }
                }
                if self.git.root.is_some() {
                    v.push(item("Git: History", "", MenuAction::GitHistory(p.clone())));
                }
                v
            }
            NodeId::Dir(d) => vec![
                item("New Java Class…", "", MenuAction::NewJavaClass(d.clone())),
                item("New File…", "", MenuAction::NewFile(d.clone())),
                item("New Directory / Package…", "", MenuAction::NewDir(d.clone())),
                sep(),
                item("Rename…", "", MenuAction::RenamePath(d.clone())),
                item("Delete…", "", MenuAction::DeletePath(d.clone())),
                sep(),
                item("Copy Path", "", MenuAction::CopyPath(d.clone())),
                item("Mở trong trình quản lý file", "", MenuAction::RevealInFiles(d.clone())),
                item("Mở terminal ở đây", "", MenuAction::OpenTerminal(d.clone())),
                sep(),
                item(hidden_label, "", MenuAction::ToggleHidden),
            ],
            NodeId::Module(m) => {
                let mp = m.display().to_string();
                vec![
                    item("New File…", "", MenuAction::NewFile(m.clone())),
                    item("New Directory…", "", MenuAction::NewDir(m.clone())),
                    item("Module Spring Boot mới…", "", MenuAction::NewSpringModule),
                    sep(),
                    item("Maven: clean install", "", MenuAction::MavenPhase(mp.clone(), "clean install".into())),
                    item("Maven: compile", "", MenuAction::MavenPhase(mp.clone(), "compile".into())),
                    item("Dependency tree", "", MenuAction::DependencyTree(mp.clone())),
                    item("Reload Maven model", "Ctrl+Shift+O", MenuAction::ReloadModel),
                    sep(),
                    item("Copy Path", "", MenuAction::CopyPath(m.clone())),
                    item("Mở trong trình quản lý file", "", MenuAction::RevealInFiles(m.clone())),
                    item("Mở terminal ở đây", "", MenuAction::OpenTerminal(m.clone())),
                    sep(),
                    item("Bỏ module khỏi project", "", MenuAction::RemoveModule(mp)),
                    item(hidden_label, "", MenuAction::ToggleHidden),
                ]
            }
            NodeId::Jar(j) => vec![
                item("Copy Path", "", MenuAction::CopyPath(j.clone())),
                item("Mở thư mục trong trình quản lý file", "", MenuAction::RevealInFiles(j.parent().map(Path::to_path_buf).unwrap_or_default())),
                sep(),
                item("Xóa khỏi ~/.m2 (tải lại khi Reload)…", "", MenuAction::DeleteJar(j.clone())),
            ],
            NodeId::Deps(_) => vec![item("Reload Maven model", "Ctrl+Shift+O", MenuAction::ReloadModel)],
            NodeId::RootDir => vec![
                item("New File…", "", MenuAction::NewFile(self.root.clone())),
                item("New Directory…", "", MenuAction::NewDir(self.root.clone())),
                item("Module Spring Boot mới…", "", MenuAction::NewSpringModule),
                sep(),
                item("Mở trong trình quản lý file", "", MenuAction::RevealInFiles(self.root.clone())),
                item("Mở terminal ở đây", "", MenuAction::OpenTerminal(self.root.clone())),
                item(hidden_label, "", MenuAction::ToggleHidden),
            ],
        };
        self.show_menu(at, items, cx);
    }

    pub(crate) fn tab_menu(&mut self, at: Point<Pixels>, i: usize, cx: &mut Context<Self>) {
        let path = self.tabs.get(i).and_then(|t| t.read(cx).path().cloned());
        let mut items = vec![
            item("Close", "Ctrl+W", MenuAction::CloseTab(i)),
            item("Close Other Tabs", "", MenuAction::CloseOthers(i)),
            item("Close All Tabs", "", MenuAction::CloseAll),
        ];
        if let Some(p) = path {
            items.push(sep());
            items.push(item("Hiện trong cây project", "Alt+F1", MenuAction::RevealInTree(p.clone())));
            items.push(item("Copy Path", "", MenuAction::CopyPath(p.clone())));
            items.push(item("Mở trong trình quản lý file", "", MenuAction::RevealInFiles(p.parent().map(Path::to_path_buf).unwrap_or_default())));
        }
        self.show_menu(at, items, cx);
    }

    pub(crate) fn console_menu(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let wrap = if self.console_wrap { "Tắt xuống dòng" } else { "Bật xuống dòng" };
        self.show_menu(
            at,
            vec![item("Copy toàn bộ log", "", MenuAction::CopyConsole), item("Xóa log", "", MenuAction::ClearConsole), sep(), item(wrap, "", MenuAction::ToggleWrap)],
            cx,
        );
    }

    pub(crate) fn maven_module_menu(&mut self, at: Point<Pixels>, module: String, cx: &mut Context<Self>) {
        let mut items: Vec<MenuItem> = ["clean", "compile", "test", "package", "install"]
            .iter()
            .map(|p| item(format!("▶ {p}"), "", MenuAction::MavenPhase(module.clone(), p.to_string())))
            .collect();
        items.push(item("▶ clean install", "", MenuAction::MavenPhase(module.clone(), "clean install".into())));
        items.push(sep());
        items.push(item("Dependency tree", "", MenuAction::DependencyTree(module.clone())));
        items.push(item("Mở terminal ở đây", "", MenuAction::OpenTerminal(PathBuf::from(&module))));
        items.push(sep());
        items.push(item("Bỏ module khỏi project", "", MenuAction::RemoveModule(module)));
        self.show_menu(at, items, cx);
    }

    // ---- actions ----------------------------------------------------------------

    fn run_menu_action(&mut self, a: MenuAction, window: &mut Window, cx: &mut Context<Self>) {
        self.context_menu = None;
        let active = self.active.and_then(|i| self.tabs.get(i)).cloned();
        let editor_cmd = |cmd: editor_core::Command, window: &mut Window, cx: &mut Context<Self>| {
            if let Some(t) = &active {
                t.update(cx, |v, cx| v.run(cmd, window, cx));
            }
        };
        match a {
            MenuAction::GotoDefinition => active.iter().for_each(|t| t.update(cx, |v, cx| v.goto_definition(cx))),
            MenuAction::FindUsages => window.dispatch_action(Box::new(crate::lsp_editor::FindUsages), cx),
            MenuAction::Rename => window.dispatch_action(Box::new(crate::lsp_editor::Rename), cx),
            MenuAction::ContextActions => window.dispatch_action(Box::new(crate::lsp_editor::ShowCodeActions), cx),
            MenuAction::Format => window.dispatch_action(Box::new(crate::lsp_editor::FormatCode), cx),
            MenuAction::OrganizeImports => window.dispatch_action(Box::new(crate::lsp_editor::OrganizeImports), cx),
            MenuAction::GotoBean => self.goto_bean(window, cx),
            MenuAction::JsfPageMap => self.jsf_page_map(window, cx),
            MenuAction::RunCoverage(p) => {
                self.coverage_next = true;
                self.run_file(p, false, window, cx)
            }
            MenuAction::GitCommit => self.open_commit(window, cx),
            MenuAction::GitFetch => self.git_fetch(cx),
            MenuAction::GitPanel => self.git_log(None, window, cx),
            MenuAction::GitPrompt(k) => self.open_name_prompt(k, window, cx),
            MenuAction::GitStash(op, i) => self.run_git("Stash", move |r| vcs::stash_op(r, op, i), move |ws, r, cx| {
                if let Err(e) = r {
                    ws.message = Some(format!("Stash {op} lỗi: {}", crate::git_support::first_line(&e)));
                    ws.open_git_if_conflicts(cx);
                }
                ws.reload_clean_tabs(cx);
            }, cx),
            MenuAction::GitAccounts => self.open_git_accounts(window, cx),
            MenuAction::GitPush => self.git_push(cx),
            MenuAction::GitPull => self.git_pull(cx),
            MenuAction::GitLog => self.git_log(None, window, cx),
            MenuAction::GitDiffAll => self.git_show_diff(None, window, cx),
            MenuAction::GitNewBranch => self.open_name_prompt(PromptKind::NewBranch, window, cx),
            MenuAction::GitCheckout(b) => {
                if let Err(e) = self.git_checkout(&b, false, cx) {
                    self.message = Some(format!("Checkout lỗi: {e}"));
                }
            }
            MenuAction::GitRollbackHunk => self.git_rollback_hunk(cx),
            MenuAction::GitRollbackFile(p) => self.git_rollback_file(p, cx),
            MenuAction::GitAnnotate => self.git_annotate(cx),
            MenuAction::GitHistory(p) => self.git_log(Some(p), window, cx),
            MenuAction::GitDiffFile(p) => self.open_diff(p, None, window, cx),
            MenuAction::GitCompareWith(p) => self.open_name_prompt(PromptKind::CompareWith(p), window, cx),
            MenuAction::GitSelOpen => self.git_open_sel(window, cx),
            MenuAction::GitSelStage => self.git_stage_sel(cx),
            MenuAction::GitSelUnstage => self.git_unstage_sel(cx),
            MenuAction::GitSelDiscard => self.git_discard_sel(window, cx),
            MenuAction::GitLogOp(op) => self.run_log_op(op, window, cx),
            MenuAction::GitForcePush => self.open_confirm(
                "Push --force-with-lease?".into(),
                format!("Ghi đè nhánh {} trên remote bằng bản local (sau amend / rebase).\nGit từ chối nếu remote có commit bạn chưa fetch - không mất việc của người khác.", self.git.branch.clone().unwrap_or_default()),
                ConfirmAction::GitForcePush,
                window,
                cx,
            ),
            MenuAction::LocalHistory => self.show_local_history(window, cx),
            MenuAction::RestoreVersion(p) => self.restore_version(p, window, cx),
            MenuAction::JsfGenerate(p, crud) => {
                let text = std::fs::read_to_string(&p).unwrap_or_default();
                match jsf::generate::parse_entity(&text) {
                    Some(_) => self.open_name_prompt(PromptKind::JsfGenerate { entity: p, crud }, window, cx),
                    None => self.message = Some("Không đọc được @Entity trong file này".into()),
                }
            }
            MenuAction::JsfExtract(p) => {
                // whole lines of the selection
                if let Some(t) = self.active.and_then(|i| self.tabs.get(i)) {
                    let v = t.read(cx);
                    let b = &v.editor.buffer;
                    let sel = v.editor.selection();
                    let start = b.line_start(b.line_of(sel.start));
                    let last = b.line_of(sel.end.saturating_sub(1).max(sel.start));
                    let end = b.line_end(last);
                    self.open_name_prompt(PromptKind::JsfExtract { page: p, start, end }, window, cx);
                }
            }
            MenuAction::SpringEndpoints => self.show_endpoints(window, cx),
            MenuAction::Cut => editor_cmd(editor_core::Command::Cut, window, cx),
            MenuAction::Copy => editor_cmd(editor_core::Command::Copy, window, cx),
            MenuAction::Paste => editor_cmd(editor_core::Command::Paste, window, cx),
            MenuAction::ToggleBreakpoint(line) => {
                if let Some(p) = active.as_ref().and_then(|t| t.read(cx).path().cloned()) {
                    self.toggle_breakpoint(p, line, cx);
                }
            }
            MenuAction::EditBreakpoint(line) => {
                if let Some(p) = active.as_ref().and_then(|t| t.read(cx).path().cloned()) {
                    self.open_bp_dialog(p, line, window, cx);
                }
            }
            MenuAction::OpenFile(p) => self.open_file(p, window, cx),
            MenuAction::NewJavaClass(d) => self.open_name_prompt(PromptKind::NewJavaClass(d), window, cx),
            MenuAction::NewFile(d) => self.open_name_prompt(PromptKind::NewFile(d), window, cx),
            MenuAction::NewDir(d) => self.open_name_prompt(PromptKind::NewDir(d), window, cx),
            MenuAction::RenamePath(p) => self.open_name_prompt(PromptKind::Rename(p), window, cx),
            MenuAction::DeletePath(p) => self.confirm_delete(p, window, cx),
            MenuAction::CopyPath(p) => cx.write_to_clipboard(ClipboardItem::new_string(p.display().to_string())),
            MenuAction::CopyRelativePath(p) => {
                let rel = p.strip_prefix(&self.root).unwrap_or(&p).display().to_string();
                cx.write_to_clipboard(ClipboardItem::new_string(rel))
            }
            MenuAction::RevealInTree(p) => self.reveal_in_tree(&p, cx),
            MenuAction::RevealInFiles(d) => {
                let opener = if cfg!(target_os = "macos") { "open" } else if cfg!(windows) { "explorer" } else { "xdg-open" };
                if let Err(e) = std::process::Command::new(opener).arg(&d).spawn() {
                    self.message = Some(format!("Không mở được {}: {e}", d.display()));
                }
            }
            MenuAction::OpenTerminal(d) => self.new_terminal_in(d, window, cx),
            MenuAction::RunFile(p, debug) => self.run_file(p, debug, window, cx),
            MenuAction::ToggleHidden => {
                self.tree.show_hidden = !self.tree.show_hidden;
                self.tree.refresh();
            }
            MenuAction::ReloadModel => self.reload_model(cx),
            MenuAction::NewSpringModule => self.open_new_module(window, cx),
            MenuAction::DeleteJar(j) => self.confirm_delete_jar(j, window, cx),
            MenuAction::CloseTab(i) => self.close_tab(i, window, cx),
            MenuAction::CloseOthers(i) => {
                let keep = self.tabs.get(i).cloned();
                self.close_tabs_where(move |t| Some(t) != keep.as_ref(), window, cx);
            }
            MenuAction::CloseAll => self.close_tabs_where(|_| true, window, cx),
            MenuAction::CopyConsole => {
                if let Some(s) = self.active_session() {
                    let text = s.console.iter().map(|(l, _)| l.as_str()).collect::<Vec<_>>().join("\n");
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
            }
            MenuAction::ClearConsole => {
                if let Some(id) = self.active_session {
                    if let Some(s) = self.sessions.iter_mut().find(|s| s.id == id) {
                        s.console.clear();
                    }
                }
            }
            MenuAction::ToggleWrap => self.console_wrap = !self.console_wrap,
            MenuAction::MavenPhase(m, goals) => {
                self.maven_tab = false;
                self.run_maven(&m, goals.split_whitespace().map(String::from).collect(), cx)
            }
            MenuAction::DependencyTree(m) => {
                self.maven_side = true;
                self.open_dependency_tree_pub(m, cx)
            }
            MenuAction::RemoveModule(m) => self.remove_module_pub(m, cx),
        }
        cx.notify();
    }

    /// Closes every tab matching `pred`; unsaved ones ask first (one prompt per file).
    fn close_tabs_where(&mut self, pred: impl Fn(&Entity<crate::editor_view::EditorView>) -> bool, window: &mut Window, cx: &mut Context<Self>) {
        let targets: Vec<_> = self.tabs.iter().filter(|t| pred(t)).cloned().collect();
        for t in targets {
            if let Some(i) = self.tabs.iter().position(|x| *x == t) {
                self.close_tab(i, window, cx);
            }
        }
    }

    /// Run/Debug a file from the tree or editor menu: its main() or its tests.
    pub(crate) fn run_file_pub(&mut self, p: PathBuf, debug: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.run_file(p, debug, window, cx)
    }

    fn run_file(&mut self, p: PathBuf, debug: bool, window: &mut Window, cx: &mut Context<Self>) {
        let text = std::fs::read_to_string(&p).unwrap_or_default();
        let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let pkg = text.lines().find_map(|l| l.trim().strip_prefix("package ").map(|r| r.trim_end_matches(';').trim().to_string()));
        let class = match pkg {
            Some(pk) => format!("{pk}.{stem}"),
            None => stem,
        };
        if text.contains("static void main(") {
            self.run_main(&p, &class, debug, window, cx);
        } else if let Some(m) = self.project.as_ref().and_then(|pr| pr.find_module_for_file(&p.display().to_string())).map(|m| m.path.clone()) {
            self.remember_test_profile(&p, &class, None, debug);
            self.run_tests(&m, vec![runner::test::TestTarget { class, methods: vec![] }], debug, cx);
        }
    }

    fn confirm_delete(&mut self, p: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let what = if p.is_dir() { "thư mục (cả nội dung bên trong)" } else { "file" };
        self.open_confirm(format!("Xóa {}?", crate::tree::file_name(&p)), format!("Xóa {what}:\n{}", p.display()), ConfirmAction::DeletePath(p), window, cx);
    }

    /// Deletes a jar's version folder from ~/.m2 (never anything outside the local repository).
    fn confirm_delete_jar(&mut self, jar: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let repo = m2_repo();
        match jar_version_dir(&jar, &repo) {
            Some(dir) => self.open_confirm(
                format!("Xóa {} khỏi ~/.m2?", crate::tree::file_name(&jar)),
                format!("Xóa thư mục:\n{}\n\nJar mất hẳn cho tới khi bấm Reload (Maven tải lại, jdtls cập nhật classpath).\nBuild offline sẽ lỗi tới lúc đó.", dir.display()),
                ConfirmAction::DeleteJar(jar),
                window,
                cx,
            ),
            None => self.message = Some("Chỉ xóa được jar nằm trong ~/.m2/repository".into()),
        }
    }

    pub(crate) fn open_confirm_pub(&mut self, title: String, detail: String, action: ConfirmAction, window: &mut Window, cx: &mut Context<Self>) {
        self.open_confirm(title, detail, action, window, cx)
    }

    fn open_confirm(&mut self, title: String, detail: String, action: ConfirmAction, window: &mut Window, cx: &mut Context<Self>) {
        let focus = cx.focus_handle();
        crate::dialogs::focus_later(focus.clone(), window, cx);
        self.confirm = Some(Confirm { title, detail, action, focus });
        cx.notify();
    }

    fn close_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm = None;
        if let Some(i) = self.active {
            self.activate(i, window, cx);
        }
        cx.notify();
    }

    fn do_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(conf) = self.confirm.take() else { return };
        match conf.action {
            ConfirmAction::DeletePath(p) => {
                let res = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
                match res {
                    Ok(()) => {
                        let gone: Vec<usize> = self.tabs.iter().enumerate().filter(|(_, t)| t.read(cx).path().is_some_and(|x| x.starts_with(&p))).map(|(i, _)| i).rev().collect();
                        for i in gone {
                            self.discard_tab(i, window, cx);
                        }
                        self.message = Some(format!("Đã xóa {}", p.display()));
                        self.notify_files(&[(p.clone(), crate::lsp_workspace::FileChange::Deleted)]);
                        // a deleted pom (or a folder holding one) changes the module list
                        if p.file_name().is_some_and(|n| n == "pom.xml") || (p.is_dir() || !p.exists()) && self.project.as_ref().is_some_and(|pr| pr.modules.iter().any(|m| std::path::Path::new(&m.path).starts_with(&p))) {
                            self.restart_lsp(cx);
                            self.reload_model(cx);
                        }
                    }
                    Err(e) => self.message = Some(format!("Không xóa được {}: {e}", p.display())),
                }
            }
            ConfirmAction::GitDiscard(list) => self.git_discard(list, cx),
            ConfirmAction::GitResetHard(h) => self.git_reset_hard(h, cx),
            ConfirmAction::GitDeleteBranch(b, force) => self.git_delete_branch(b, force, window, cx),
            ConfirmAction::GitForcePush => self.git_push_with(true, cx),
            ConfirmAction::GitUndoCommit => self.git_undo_last_commit_now(cx),
            ConfirmAction::DeleteJar(jar) => {
                self.message = Some(match delete_jar(&jar, &m2_repo()) {
                    // jdtls is NOT told here on purpose: it (m2e) would re-download the jar at
                    // once (seen 30/9: deleted jars came back within a minute). The jar stays
                    // deleted until Reload, which runs Maven online and then updates jdtls.
                    Ok(dir) => format!("Đã xóa {} - bấm Reload (Maven) để tải lại và cập nhật jdtls", dir.display()),
                    Err(e) => format!("Không xóa được: {e}"),
                });
            }
        }
        self.tree.refresh();
        if let Some(i) = self.active {
            self.activate(i, window, cx);
        }
        cx.notify();
    }

    pub(crate) fn render_confirm(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let conf = self.confirm.as_ref()?;
        let ok = conf.action.ok_label();
        let lines: Vec<AnyElement> = conf.detail.lines().map(|l| div().min_h(px(18.)).text_color(c(theme::FG)).child(SharedString::from(l.to_string())).into_any_element()).collect();
        let footer = div()
            .flex()
            .gap_2()
            .child(dialogs::button("cf-cancel", "Hủy (Esc)", false, cx, |ws: &mut Workspace, window, cx| ws.close_confirm(window, cx)))
            .child(dialogs::button("cf-ok", ok, true, cx, |ws: &mut Workspace, window, cx| ws.do_confirm(window, cx)));
        Some(
            // focus inside the "Dialog" key context, so Enter / Esc reach this dialog
            dialogs::modal("cf-body", &conf.title, 620., div().track_focus(&conf.focus).flex().flex_col().children(lines), footer)
                .on_action(cx.listener(|ws, _: &DialogConfirm, window, cx| ws.do_confirm(window, cx)))
                .on_action(cx.listener(|ws, _: &DialogCancel, window, cx| ws.close_confirm(window, cx)))
                .into_any_element(),
        )
    }

    // ---- name prompt (New / Rename) -----------------------------------------------

    pub(crate) fn open_name_prompt(&mut self, kind: PromptKind, window: &mut Window, cx: &mut Context<Self>) {
        let (hint, initial) = match &kind {
            PromptKind::NewJavaClass(_) => ("Tên class, vd: OrderService", String::new()),
            PromptKind::NewFile(_) => ("Tên file, vd: application-local.yml", String::new()),
            PromptKind::NewDir(_) => ("Tên thư mục; trong source root gõ package: vn.longvan.order", String::new()),
            PromptKind::Rename(p) => ("Tên mới", crate::tree::file_name(p)),
            PromptKind::JsfGenerate { entity, .. } => {
                let text = std::fs::read_to_string(entity).unwrap_or_default();
                ("Package cho bean / LazyDataModel", jsf::generate::parse_entity(&text).map(|e| jsf::generate::default_view_package(&e)).unwrap_or_default())
            }
            PromptKind::JsfExtract { .. } => ("Tên component, vd: productCard", String::new()),
            PromptKind::NewBranch => ("Tên nhánh mới, vd: feature/gui-mail", String::new()),
            PromptKind::Stash(_) => ("Ghi chú cho stash (có thể bỏ trống)", String::new()),
            PromptKind::BranchAt(_) => ("Tên nhánh mới, vd: hotfix/loi-mail", String::new()),
            PromptKind::TagAt(_) => ("Tên tag, vd: v2.6.3", String::new()),
            PromptKind::RenameBranch(b) => ("Tên mới của nhánh", b.clone()),
            PromptKind::MergeBranch => ("Tên nhánh, vd: origin/dev-2.6.2", String::new()),
            PromptKind::RebaseBranch => ("Tên nhánh, vd: origin/production", String::new()),
            PromptKind::CompareWith(_) => ("Nhánh / commit, vd: origin/dev-2.6.2, HEAD~3, a1b2c3d", String::new()),
            PromptKind::AttachJvm => ("host:port, vd: localhost:5005 (JVM chạy với -agentlib:jdwp=transport=dt_socket,server=y,suspend=n,address=*:5005)", "localhost:5005".to_string()),
        };
        let input = cx.new(|cx| TextInput::new(hint, cx));
        input.update(cx, |i, cx| i.set_text(&initial, cx));
        crate::dialogs::focus_later(input.read(cx).focus_handle(cx), window, cx);
        self.name_prompt = Some(NamePrompt { kind, input, error: None });
        cx.notify();
    }

    fn close_name_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.name_prompt = None;
        if let Some(i) = self.active {
            self.activate(i, window, cx);
        }
        cx.notify();
    }

    /// Package for a directory under a source root ("…/src/main/java/vn/a" → "vn.a").
    fn package_of(&self, dir: &Path) -> Option<String> {
        let roots = self.project.as_ref()?.modules.iter().flat_map(|m| m.source_roots.iter().map(|s| PathBuf::from(&s.path))).collect::<Vec<_>>();
        let root = roots.iter().filter(|r| dir.starts_with(r)).max_by_key(|r| r.as_os_str().len())?;
        let rel = dir.strip_prefix(root).ok()?;
        Some(rel.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect::<Vec<_>>().join("."))
    }

    fn confirm_name_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(np) = self.name_prompt.as_ref() else { return };
        let name = np.input.read(cx).content.trim().to_string();
        let kind = np.kind.clone();
        let free_text = matches!(
            kind,
            PromptKind::Stash(_) | PromptKind::MergeBranch | PromptKind::RebaseBranch | PromptKind::CompareWith(_) | PromptKind::NewBranch | PromptKind::BranchAt(_) | PromptKind::TagAt(_) | PromptKind::RenameBranch(_)
        );
        if (name.is_empty() && !matches!(kind, PromptKind::Stash(_))) || (name.contains('/') && !free_text && !matches!(kind, PromptKind::NewFile(_) | PromptKind::NewDir(_))) {
            if let Some(np) = self.name_prompt.as_mut() {
                np.error = Some("Tên không hợp lệ".into());
            }
            cx.notify();
            return;
        }
        let result: Result<Option<PathBuf>, String> = match &kind {
            PromptKind::JsfGenerate { entity, crud } => self.jsf_generate(entity, &name, *crud, cx),
            PromptKind::JsfExtract { page, start, end } => self.jsf_extract(page, *start, *end, &name, cx),
            PromptKind::NewBranch => self.git_checkout(&name, true, cx).map(|_| None),
            PromptKind::AttachJvm => self.attach_jvm(&name, cx).map(|_| None),
            PromptKind::Stash(_) | PromptKind::MergeBranch | PromptKind::RebaseBranch | PromptKind::CompareWith(_) | PromptKind::BranchAt(_) | PromptKind::TagAt(_) | PromptKind::RenameBranch(_) => {
                self.git_prompt(&kind, &name, window, cx).map(|_| None)
            }
            PromptKind::NewJavaClass(dir) => {
                let class = name.trim_end_matches(".java").to_string();
                let path = dir.join(format!("{class}.java"));
                let pkg = self.package_of(dir).filter(|p| !p.is_empty());
                let body = match pkg {
                    Some(p) => format!("package {p};\n\npublic class {class} {{\n\n}}\n"),
                    None => format!("public class {class} {{\n\n}}\n"),
                };
                create_new(&path, &body).map(Some)
            }
            PromptKind::NewFile(dir) => create_new(&dir.join(&name), "").map(Some),
            PromptKind::NewDir(dir) => {
                // "a.b.c" inside a source root means nested packages
                let rel = if self.package_of(dir).is_some() && !name.contains('/') { name.replace('.', "/") } else { name.clone() };
                std::fs::create_dir_all(dir.join(rel)).map(|_| None).map_err(|e| e.to_string())
            }
            PromptKind::Rename(p) => {
                let to = p.with_file_name(&name);
                if to.exists() {
                    Err(format!("{} đã tồn tại", to.display()))
                } else {
                    std::fs::rename(p, &to).map_err(|e| e.to_string()).map(|_| {
                        self.retarget_tabs(p, &to, cx);
                        None
                    })
                }
            }
        };
        // tell jdtls what appeared / moved on disk
        match (&kind, &result) {
            (PromptKind::NewJavaClass(_) | PromptKind::NewFile(_), Ok(Some(p))) => self.notify_files(&[(p.clone(), crate::lsp_workspace::FileChange::Created)]),
            (PromptKind::NewDir(d), Ok(_)) => {
                let rel = if self.package_of(d).is_some() && !name.contains('/') { name.replace('.', "/") } else { name.clone() };
                self.notify_files(&[(d.join(rel), crate::lsp_workspace::FileChange::Created)]);
            }
            (PromptKind::Rename(p), Ok(_)) => {
                let to = p.with_file_name(&name);
                self.notify_files(&[(p.clone(), crate::lsp_workspace::FileChange::Deleted), (to, crate::lsp_workspace::FileChange::Created)]);
            }
            _ => {}
        }
        match result {
            Ok(open) => {
                self.name_prompt = None;
                self.tree.refresh();
                match open {
                    Some(p) => self.open_file(p, window, cx),
                    None => self.close_name_prompt(window, cx),
                }
            }
            Err(e) => {
                if let Some(np) = self.name_prompt.as_mut() {
                    np.error = Some(e);
                }
            }
        }
        cx.notify();
    }

    /// Open editors follow a renamed file / folder.
    fn retarget_tabs(&mut self, from: &Path, to: &Path, cx: &mut Context<Self>) {
        let client = self.lsp.clone();
        for t in self.tabs.clone() {
            let Some(p) = t.read(cx).path().cloned() else { continue };
            let Ok(rest) = p.strip_prefix(from) else { continue };
            let new = if rest.as_os_str().is_empty() { to.to_path_buf() } else { to.join(rest) };
            t.update(cx, |v, _| {
                v.detach_lsp();
                v.editor.buffer.path = Some(new.clone());
                if let (Some(c), true) = (client.clone(), new.extension().is_some_and(|e| e == "java")) {
                    v.attach_lsp(c, lsp_client::path_to_uri(&new), true);
                }
            });
        }
    }

    pub(crate) fn render_name_prompt(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let np = self.name_prompt.as_ref()?;
        let title = match &np.kind {
            PromptKind::NewJavaClass(d) => format!("New Java Class trong {}", self.package_of(d).filter(|p| !p.is_empty()).unwrap_or_else(|| crate::tree::file_name(d))),
            PromptKind::NewFile(d) => format!("New File trong {}", crate::tree::file_name(d)),
            PromptKind::NewDir(d) => format!("New Directory trong {}", crate::tree::file_name(d)),
            PromptKind::Rename(p) => format!("Đổi tên {}", crate::tree::file_name(p)),
            PromptKind::JsfGenerate { entity, crud } => format!("{} từ {}", if *crud { "JSF CRUD" } else { "LazyDataModel" }, crate::tree::file_name(entity)),
            PromptKind::JsfExtract { .. } => "Tách thành composite component".to_string(),
            PromptKind::NewBranch => format!("Nhánh mới từ {}", self.git.branch.clone().unwrap_or_default()),
            PromptKind::AttachJvm => "Debug: Attach vào JVM đang chạy".to_string(),
            PromptKind::Stash(u) => format!("Stash thay đổi{}", if *u { " (cả file chưa theo dõi)" } else { "" }),
            PromptKind::BranchAt(h) => format!("Nhánh mới tại {}", &h[..h.len().min(8)]),
            PromptKind::TagAt(h) => format!("Tag mới tại {}", &h[..h.len().min(8)]),
            PromptKind::RenameBranch(b) => format!("Đổi tên nhánh {b}"),
            PromptKind::MergeBranch => format!("Merge vào {}", self.git.branch.clone().unwrap_or_default()),
            PromptKind::RebaseBranch => format!("Rebase {} lên", self.git.branch.clone().unwrap_or_default()),
            PromptKind::CompareWith(p) => format!("So sánh {} với nhánh / commit", crate::tree::file_name(p)),
        };
        let body = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(np.input.clone())
            .when_some(np.error.clone(), |d, e| d.child(div().text_color(c(theme::ERROR)).child(SharedString::from(e))));
        let footer = div()
            .flex()
            .gap_2()
            .child(dialogs::button("np-cancel", "Hủy (Esc)", false, cx, |ws: &mut Workspace, window, cx| ws.close_name_prompt(window, cx)))
            .child(dialogs::button("np-ok", "OK (Enter)", true, cx, |ws: &mut Workspace, window, cx| ws.confirm_name_prompt(window, cx)));
        Some(
            dialogs::modal("np-body", &title, 520., body, footer)
                .on_action(cx.listener(|ws, _: &DialogConfirm, window, cx| ws.confirm_name_prompt(window, cx)))
                .on_action(cx.listener(|ws, _: &DialogCancel, window, cx| ws.close_name_prompt(window, cx)))
                .into_any_element(),
        )
    }

    // ---- rendering -------------------------------------------------------------

    pub(crate) fn render_context_menu(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.context_menu.as_ref()?;
        let at = menu.at;
        let rows: Vec<AnyElement> = menu
            .items
            .iter()
            .enumerate()
            .map(|(i, it)| match &it.action {
                None => div().my_1().h(px(1.)).bg(c(theme::BORDER)).into_any_element(),
                Some(a) => {
                    let a = a.clone();
                    let label = it.label.clone();
                    div()
                        .id(("menu-item", i))
                        .debug_selector(move || format!("menu:{label}"))
                        .flex()
                        .flex_row()
                        .justify_between()
                        .gap_6()
                        .px_3()
                        .py(px(2.))
                        .whitespace_nowrap()
                        .mx_1()
                        .rounded_md()
                        .hover(|s| s.bg(c(theme::ROW_ACTIVE)))
                        .child(div().text_color(c(theme::FG)).child(it.label.clone()))
                        .child(div().text_color(c(theme::FG_DIM)).child(it.shortcut))
                        .on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, window, cx| {
                            cx.stop_propagation();
                            ws.run_menu_action(a.clone(), window, cx)
                        }))
                        .into_any_element()
                }
            })
            .collect();
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                // click anywhere else closes the menu
                .on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| {
                    ws.context_menu = None;
                    cx.notify()
                }))
                .on_mouse_down(MouseButton::Right, cx.listener(|ws, _, _, cx| {
                    ws.context_menu = None;
                    cx.notify()
                }))
                .child(deferred(
                    anchored().position(at).child(
                        div()
                            .min_w(px(240.))
                            .py_1()
                            .bg(c(theme::POPUP_BG))
                            .border_1()
                            .border_color(c(theme::BORDER))
                            .rounded_md()
                            .shadow_lg().occlude().rounded_lg()
                            .children(rows),
                    ),
                ))
                .into_any_element(),
        )
    }
}

fn create_new(path: &Path, content: &str) -> Result<PathBuf, String> {
    if path.exists() {
        return Err(format!("{} đã tồn tại", path.display()));
    }
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, content).map_err(|e| e.to_string())?;
    Ok(path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delete_jar_only_inside_repo() {
        let base = std::env::temp_dir().join(format!("m2-test-{}", std::process::id()));
        let repo = base.join("repository");
        let ver = repo.join("org/demo/lib/1.0");
        std::fs::create_dir_all(&ver).unwrap();
        let jar = ver.join("lib-1.0.jar");
        std::fs::write(&jar, "x").unwrap();
        std::fs::write(ver.join("lib-1.0.pom"), "x").unwrap();
        // outside the repo / too shallow / path tricks are refused
        assert!(jar_version_dir(&base.join("other/lib.jar"), &repo).is_none());
        assert!(jar_version_dir(&repo.join("lib.jar"), &repo).is_none());
        assert!(jar_version_dir(&repo.join("org/demo/../../x/1/a.jar"), &repo).is_none());
        assert_eq!(delete_jar(&jar, &repo).unwrap(), ver);
        assert!(!ver.exists() && repo.join("org/demo/lib").exists());
        std::fs::remove_dir_all(&base).unwrap();
    }
}
