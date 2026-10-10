//! Git tool window (Alt+9, like SmartGit's main window / IntelliJ's Commit pane): branch and
//! upstream, an ongoing merge/rebase with Continue/Skip/Abort, the changed files grouped
//! (conflicts, staged, changes, untracked) with multi-selection, checkboxes = staged, and the
//! commit message next to them - Commit commits exactly what is staged. Plus the side-by-side
//! diff (index ↔ work tree, HEAD ↔ index, a revision ↔ now) with stage / unstage / revert of
//! single blocks, and the three-way conflict resolver.
use crate::context_menu::{MenuAction, menu_item, menu_sep};
use crate::dialogs::Field;
use crate::git_style::icon_btn;
use crate::ui_icons::{Ico, icon, tip};
use crate::theme::{self, c};
use crate::workspace::Workspace;
use gpui::{
    AnyElement, Context, FocusHandle, MouseButton, MouseDownEvent, ScrollStrategy, SharedString, UniformListScrollHandle, Window, div, prelude::*, px,
    uniform_list,
};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use vcs::{Change, Conflict, DiffRow, Hunk, Pick, RepoState, RowKind, Stash};

/// What the Git panel shows (filled by refresh_git in the background).
#[derive(Default, Clone)]
pub struct GitTree {
    pub changes: Vec<Change>,
    pub ahead_behind: Option<(usize, usize)>,
    pub state: Option<RepoState>,
    pub stashes: Vec<Stash>,
    /// local then remote branches, current marked (the branch menu)
    pub branches: Vec<(String, bool)>,
    pub remotes: Vec<String>,
}

pub fn load_tree(root: &Path) -> GitTree {
    let changes = vcs::changes(root);
    let state = vcs::effective_state(root, &changes);
    GitTree {
        changes,
        ahead_behind: vcs::ahead_behind(root),
        state: Some(state),
        stashes: vcs::stashes(root),
        branches: vcs::branches(root),
        remotes: vcs::remote_names(root),
    }
}

/// Selection, commit message… of the Git window (kept while it is closed).
pub struct GitUi {
    pub msg: Field,
    pub amend: bool,
    /// (group, path): a file both staged and changed is two rows
    pub selected: BTreeSet<(char, PathBuf)>,
    pub anchor: Option<usize>,
    pub cursor: Option<usize>,
    /// files under their folder instead of full paths
    pub by_dir: bool,
    pub focus: FocusHandle,
}

/// Height of every row of the file list (uniform_list measures one row for all).
const CHANGE_ROW_H: f32 = 28.;

/// A row of the file list before it is built into an element.
#[derive(Clone)]
enum ChangeItem {
    /// title, count, color
    Section(&'static str, usize, u32),
    Dir(String),
    /// index into the entries, group (C/S/W/U), the change
    File(usize, char, Change),
    Stash(usize, String),
    Empty,
}

/// unchanged lines kept around each change when the rest is folded
const DIFF_CONTEXT: usize = 3;

/// A theme color at `a` opacity: diff and conflict tints that read on dark and light alike.
fn tint(color: u32, a: f32) -> gpui::Hsla {
    let mut h = c(color);
    h.a = a;
    h
}
const ADD_COLOR: u32 = theme::GIT_ADDED;
const DEL_COLOR: u32 = theme::ERROR;

/// A line of the diff list: a row, or a run of unchanged rows folded away.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Disp {
    Row(usize),
    /// first row, count
    Fold(usize, usize),
}

/// The visible lines: unchanged runs longer than the context folded (unless `opened`).
fn diff_display(rows: &[DiffRow], fold: bool, opened: &BTreeSet<usize>) -> Vec<Disp> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < rows.len() {
        if rows[i].kind != RowKind::Same {
            out.push(Disp::Row(i));
            i += 1;
            continue;
        }
        let start = i;
        while i < rows.len() && rows[i].kind == RowKind::Same {
            i += 1;
        }
        let head = if start == 0 { 0 } else { DIFF_CONTEXT };
        let tail = if i == rows.len() { 0 } else { DIFF_CONTEXT };
        if fold && !opened.contains(&start) && i - start > head + tail + 1 {
            out.extend((start..start + head).map(Disp::Row));
            out.push(Disp::Fold(start, i - start - head - tail));
            out.extend((i - tail..i).map(Disp::Row));
        } else {
            out.extend((start..i).map(Disp::Row));
        }
    }
    out
}

/// What the two sides of a diff are.
#[derive(Clone, Debug, PartialEq)]
pub enum DiffMode {
    /// index (staged) ↔ work tree: stage / revert blocks
    WorkTree,
    /// HEAD ↔ index: unstage blocks
    Staged,
    /// a revision ↔ the editor text: revert blocks
    Rev(String),
    /// a commit of the log: read-only
    Commit,
}

pub struct DiffView {
    pub path: PathBuf,
    pub left_title: String,
    pub right_title: String,
    pub rows: Vec<DiffRow>,
    /// each change block: the row it starts at and its hunk (old = left, new = right)
    pub blocks: Vec<(usize, Hunk)>,
    pub current: usize,
    pub scroll: UniformListScrollHandle,
    pub mode: DiffMode,
    pub old_text: String,
    pub new_text: String,
    pub focus: FocusHandle,
    pub needs_focus: bool,
    /// hide unchanged lines away from the changes
    pub fold: bool,
    /// folds the user expanded (their first row)
    pub opened: BTreeSet<usize>,
    pub display: Vec<Disp>,
    /// lines added / removed
    pub stats: (usize, usize),
}

impl DiffView {
    fn redisplay(&mut self) {
        self.display = diff_display(&self.rows, self.fold, &self.opened);
    }

    fn scroll_to_current(&self) {
        let row = self.blocks[self.current].0;
        let at = self.display.iter().position(|d| *d == Disp::Row(row)).unwrap_or(0);
        self.scroll.scroll_to_item(at, ScrollStrategy::Center);
    }
}

pub struct ConflictView {
    pub path: PathBuf,
    /// the file's status entry (DU / UD: one side deleted it)
    pub change: Option<Change>,
    pub conflicts: Vec<Conflict>,
    /// the same conflicts re-merged with diff3 (their base when the markers lack it)
    pub d3: Vec<Conflict>,
    pub current: usize,
    /// file texts before each pick (Undo)
    pub history: Vec<String>,
    pub lines: Vec<String>,
    pub state: RepoState,
    pub error: Option<String>,
    pub scroll: UniformListScrollHandle,
    pub focus: FocusHandle,
    pub needs_focus: bool,
    /// conflicts when it was opened (progress)
    pub total: usize,
}

impl Workspace {
    pub(crate) fn git_ui(&mut self, cx: &mut Context<Self>) -> &mut GitUi {
        if self.git_ui.is_none() {
            self.git_ui = Some(GitUi {
                msg: Field::new("Message", "Commit message - Enter để commit", "", cx),
                amend: false,
                selected: BTreeSet::new(),
                anchor: None,
                cursor: None,
                by_dir: true,
                focus: cx.focus_handle(),
            });
        }
        self.git_ui.as_mut().unwrap()
    }

    /// After a refresh: selected rows that are gone are dropped.
    pub(crate) fn git_ui_sync(&mut self) {
        let keys: BTreeSet<(char, PathBuf)> = self.git_entries().into_iter().map(|(g, c)| (g, c.path)).collect();
        let n = keys.len();
        if let Some(ui) = self.git_ui.as_mut() {
            ui.selected.retain(|k| keys.contains(k));
            ui.cursor = ui.cursor.filter(|&i| i < n);
            ui.anchor = ui.anchor.filter(|&i| i < n);
        }
    }

    /// The rows of the file list in display order: (group, change). Groups: C conflict,
    /// S staged, W changed (not staged), U untracked.
    pub(crate) fn git_entries(&self) -> Vec<(char, Change)> {
        let by_dir = self.git_ui.as_ref().is_none_or(|u| u.by_dir);
        let mut out = Vec::new();
        for g in ['C', 'S', 'W', 'U'] {
            let mut list: Vec<Change> = self
                .git
                .tree
                .changes
                .iter()
                .filter(|c| match g {
                    'C' => c.conflicted(),
                    'S' => c.staged(),
                    'W' => c.unstaged(),
                    _ => c.untracked(),
                })
                .cloned()
                .collect();
            if by_dir {
                list.sort_by(|a, b| (a.path.parent(), a.path.file_name()).cmp(&(b.path.parent(), b.path.file_name())));
            } else {
                list.sort_by(|a, b| a.path.cmp(&b.path));
            }
            out.extend(list.into_iter().map(|c| (g, c)));
        }
        out
    }

    /// The selected rows (or the row under the cursor when nothing is selected).
    fn git_selection(&self) -> Vec<(char, Change)> {
        let Some(ui) = self.git_ui.as_ref() else { return Vec::new() };
        let entries = self.git_entries();
        let sel: Vec<(char, Change)> = entries.iter().filter(|(g, c)| ui.selected.contains(&(*g, c.path.clone()))).cloned().collect();
        if !sel.is_empty() {
            return sel;
        }
        ui.cursor.and_then(|i| entries.get(i).cloned()).into_iter().collect()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn open_git_tab(&mut self, cx: &mut Context<Self>) {
        self.panel_open = true;
        self.git_tab = true;
        self.maven_tab = false;
        self.terminal_tab = false;
        self.git_ui(cx);
        self.refresh_git(cx);
        cx.notify();
    }

    /// Stash / merge / rebase / compare-with / branch names from the name prompt.
    pub(crate) fn git_prompt(&mut self, kind: &crate::context_menu::PromptKind, text: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        use crate::context_menu::PromptKind;
        self.git.root.clone().ok_or("Không có git repository")?;
        if !matches!(kind, PromptKind::CompareWith(_)) {
            // like IntelliJ: unsaved edits first
            self.save_all_for_git(cx);
        }
        let t = text.to_string();
        match kind {
            PromptKind::Stash(untracked) => {
                let u = *untracked;
                self.run_git("Stash", move |r| vcs::stash_push(r, &t, u), |ws, r, _| {
                    ws.message = Some(match r {
                        Ok(_) => "Đã stash".into(),
                        Err(e) => format!("Stash lỗi: {}", crate::git_support::first_line(&e)),
                    })
                }, cx);
            }
            PromptKind::MergeBranch | PromptKind::RebaseBranch => {
                let merge = matches!(kind, PromptKind::MergeBranch);
                self.run_git(if merge { "Merge" } else { "Rebase" }, move |r| if merge { vcs::merge(r, &t) } else { vcs::rebase(r, &t) }, move |ws, r, cx| match r {
                    Ok(_) => ws.message = Some(format!("{} xong", if merge { "Merge" } else { "Rebase" })),
                    Err(e) => {
                        ws.message = Some(format!("{} dừng: {}", if merge { "Merge" } else { "Rebase" }, crate::git_support::first_line(&e)));
                        // conflicts: the Git window shows them with Continue / Abort
                        ws.open_git_if_conflicts(cx);
                    }
                }, cx);
            }
            PromptKind::BranchAt(hash) => {
                let h = hash.clone();
                self.run_git("Tạo nhánh", move |r| vcs::create_branch_at(r, &t, &h, true), |ws, r, _| {
                    ws.message = Some(match r {
                        Ok(_) => format!("Đã tạo và chuyển sang nhánh {}", ws.git.root.as_deref().and_then(vcs::branch).unwrap_or_default()),
                        Err(e) => format!("Tạo nhánh lỗi: {}", crate::git_support::first_line(&e)),
                    })
                }, cx);
            }
            PromptKind::TagAt(hash) => {
                let h = hash.clone();
                self.run_git_simple("Tạo tag", move |r| vcs::create_tag(r, &t, &h), cx);
            }
            PromptKind::RenameBranch(old) => {
                let o = old.clone();
                self.run_git_simple("Đổi tên nhánh", move |r| vcs::rename_branch(r, &o, &t), cx);
            }
            PromptKind::CompareWith(p) => {
                self.open_diff(p.clone(), Some(text.to_string()), window, cx);
            }
            _ => {}
        }
        Ok(())
    }

    // ---- the panel ---------------------------------------------------------------------

    pub(crate) fn render_git_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(root) = self.git.root.clone() else {
            return div().p_2().text_color(c(theme::FG_DIM)).child("Project không nằm trong git repository").into_any_element();
        };
        self.git_ui(cx);
        let tree = self.git.tree.clone();
        let branch = self.git.branch.clone().unwrap_or_default();
        let state = tree.state.unwrap_or(RepoState::Clean);
        let tool = |id: &'static str, label: &'static str| {
            div().id(id).px_3().rounded_full().whitespace_nowrap().text_size(px(12.)).text_color(c(theme::FG_DIM)).hover(|s| s.bg(c(theme::ROW_HOVER)).text_color(c(theme::FG))).child(label)
        };
        use crate::git_style::icon_btn;
        use crate::ui_icons::{Ico, icon, tip};
        // ahead / behind as small pills; nothing when in sync
        let (ahead, behind, no_upstream) = match tree.ahead_behind {
            Some((a, b)) => (a, b, false),
            None => (0, 0, true),
        };
        let pill = |text: String, color: u32| {
            let mut bg = c(color);
            bg.a = 0.16;
            div().flex_none().px(px(7.)).rounded_full().bg(bg).text_size(px(11.)).text_color(c(color)).child(SharedString::from(text))
        };
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_3()
            .h(px(40.))
            .border_b_1()
            .border_color(c(theme::BORDER))
            .child(icon(Ico::Branch, 15., c(theme::ACCENT)))
            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(theme::FG)).child(SharedString::from(branch.clone())))
            .when(ahead > 0, |d| d.child(pill(format!("↑ {ahead}"), theme::GIT_ADDED)))
            .when(behind > 0, |d| d.child(pill(format!("↓ {behind}"), theme::GIT_MODIFIED)))
            .when(no_upstream, |d| d.child(div().id("git-no-upstream").child(pill("local".into(), theme::FG_DIM)).tooltip(tip("Nhánh chưa có upstream"))))
            .when(state != RepoState::Clean, |d| {
                d.child(div().ml_2().px_2().rounded_sm().bg(c(theme::WARNING)).text_color(gpui::black()).child(state.label()))
                    .child(tool("git-continue", "▶ Continue").on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, cx| {
                        ws.run_git("Continue", move |r| vcs::state_action(r, state, "continue"), |ws, r, cx| {
                            if let Err(e) = r {
                                ws.message = Some(format!("Continue lỗi: {}", crate::git_support::first_line(&e)));
                                // the next commit of a rebase can conflict again
                                ws.open_git_if_conflicts(cx);
                            }
                        }, cx)
                    })))
                    .when(state == RepoState::Rebasing, |d| {
                        d.child(tool("git-skip", "⤼ Skip").on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, cx| {
                            ws.run_git_simple("Skip", move |r| vcs::state_action(r, state, "skip"), cx)
                        })))
                    })
                    .child(tool("git-abort", "✕ Abort").on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, cx| {
                        ws.run_git_simple("Abort", move |r| vcs::state_action(r, state, "abort"), cx)
                    })))
            })
            .when_some(self.git.busy.clone(), |d, b| d.child(div().ml_2().text_color(c(theme::ACCENT)).child(SharedString::from(format!("⏳ {b}…")))))
            .child(div().flex_1())
            .child(icon_btn("git-refresh", Ico::Refresh, "Tải lại").on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.refresh_git(cx))))
            .child(icon_btn("git-stage-all", Ico::Plus, "Stage tất cả").on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| {
                let paths: Vec<PathBuf> = ws.git.tree.changes.iter().filter(|c| c.unstaged() || c.untracked()).flat_map(|c| c.paths()).collect();
                if !paths.is_empty() {
                    ws.run_git_simple("Stage", move |r| vcs::stage(r, &paths), cx)
                }
            })))
            .child(icon_btn("git-unstage-all", Ico::Minus, "Unstage tất cả").on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| {
                let paths: Vec<PathBuf> = ws.git.tree.changes.iter().filter(|c| c.staged()).flat_map(|c| c.paths()).collect();
                if !paths.is_empty() {
                    ws.run_git_simple("Unstage", move |r| vcs::unstage(r, &paths), cx)
                }
            })))
            .child(icon_btn("git-stash", Ico::Stash, "Stash…").on_mouse_down(MouseButton::Left, cx.listener(|ws, ev: &MouseDownEvent, _, cx| {
                use crate::context_menu::PromptKind;
                let items = vec![
                    menu_item("Stash file đã theo dõi…", "", MenuAction::GitPrompt(PromptKind::Stash(false))),
                    menu_item("Stash cả file chưa theo dõi…", "", MenuAction::GitPrompt(PromptKind::Stash(true))),
                ];
                ws.show_menu(ev.position, items, cx)
            })))
            .child(if self.git_ui.as_ref().is_some_and(|u| u.by_dir) { icon_btn("git-dir", Ico::Tree, "Đang nhóm theo thư mục - bấm để xem danh sách") } else { icon_btn("git-dir", Ico::List, "Đang xem danh sách - bấm để nhóm theo thư mục") }.on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| {
                let ui = ws.git_ui(cx);
                ui.by_dir = !ui.by_dir;
                ui.selected.clear();
                ui.cursor = None;
                cx.notify()
            })))
            .child(icon_btn("git-log2", Ico::History, "Git Log").on_mouse_down(MouseButton::Left, cx.listener(|ws, _, window, cx| ws.git_log(None, window, cx))));

        let rows = self.git_change_list(&root, "git-rows-list", true, cx);
        let focus = self.git_ui.as_ref().unwrap().focus.clone();
        let list = div()
            .id("git-rows")
            .key_context("GitPanel")
            .track_focus(&focus)
            .flex_1()
            .min_w_0()
            .pb_2()
            .on_action(cx.listener(|ws, _: &crate::actions::GitUp, _, cx| ws.git_move(-1, false, cx)))
            .on_action(cx.listener(|ws, _: &crate::actions::GitDown, _, cx| ws.git_move(1, false, cx)))
            .on_action(cx.listener(|ws, _: &crate::actions::GitSelUp, _, cx| ws.git_move(-1, true, cx)))
            .on_action(cx.listener(|ws, _: &crate::actions::GitSelDown, _, cx| ws.git_move(1, true, cx)))
            .on_action(cx.listener(|ws, _: &crate::actions::GitToggleStage, _, cx| ws.git_toggle_stage_sel(cx)))
            .on_action(cx.listener(|ws, _: &crate::actions::GitOpenSel, window, cx| ws.git_open_sel(window, cx)))
            .on_action(cx.listener(|ws, _: &crate::actions::GitDiscardSel, window, cx| ws.git_discard_sel(window, cx)))
            .child(rows);
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(header)
            .child(div().flex().flex_row().flex_1().min_h_0().child(list).child(self.render_commit_box(&branch, &tree, false, cx)))
            .into_any_element()
    }

    /// The file list (conflicts / staged / changed / untracked, optionally stashes) shared by the
    /// bottom panel and the Git window. Virtualized: rows are built only for what is on screen,
    /// from light descriptors; every row is `CHANGE_ROW_H` tall.
    pub(crate) fn git_change_list(&mut self, root: &Path, id: &'static str, with_stashes: bool, cx: &mut Context<Self>) -> AnyElement {
        let items = std::sync::Arc::new(self.git_change_items(root, with_stashes));
        let root = root.to_path_buf();
        let n = items.len();
        gpui::uniform_list(id, n, cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
            range.map(|i| this.render_change_item(&root, &items[i], cx)).collect::<Vec<_>>()
        }))
        .size_full()
        .into_any_element()
    }

    fn git_change_items(&mut self, root: &Path, with_stashes: bool) -> Vec<ChangeItem> {
        let entries = self.git_entries();
        let by_dir = self.git_ui.as_ref().is_some_and(|u| u.by_dir);
        let mut items = Vec::new();
        let mut last_group = ' ';
        let mut last_dir: Option<PathBuf> = None;
        for (k, (g, ch)) in entries.iter().enumerate() {
            if *g != last_group {
                last_group = *g;
                last_dir = None;
                let n = entries.iter().filter(|(x, _)| x == g).count();
                let (title, color) = match g {
                    'C' => ("Xung đột", theme::ERROR),
                    'S' => ("Đã stage", theme::GIT_ADDED),
                    'W' => ("Thay đổi", theme::GIT_MODIFIED),
                    _ => ("Chưa theo dõi", theme::GIT_UNTRACKED),
                };
                items.push(ChangeItem::Section(title, n, color));
            }
            if by_dir {
                let dir = ch.path.parent().map(Path::to_path_buf);
                if dir != last_dir {
                    let rel = dir.as_deref().and_then(|d| d.strip_prefix(root).ok()).map(|d| d.display().to_string()).unwrap_or_default();
                    items.push(ChangeItem::Dir(if rel.is_empty() { "gốc repo".to_string() } else { rel }));
                    last_dir = dir;
                }
            }
            items.push(ChangeItem::File(k, *g, ch.clone()));
        }
        if entries.is_empty() {
            items.push(ChangeItem::Empty);
        }
        if with_stashes && !self.git.tree.stashes.is_empty() {
            items.push(ChangeItem::Section("Stash", self.git.tree.stashes.len(), theme::FG_DIM));
            items.extend(self.git.tree.stashes.iter().map(|s| ChangeItem::Stash(s.index, s.message.clone())));
        }
        items
    }

    fn render_change_item(&mut self, root: &Path, item: &ChangeItem, cx: &mut Context<Self>) -> AnyElement {
        use crate::ui_icons::{Ico, icon};
        let row = || div().flex().flex_row().items_center().h(px(CHANGE_ROW_H));
        match item {
            ChangeItem::Section(title, n, color) => row()
                .gap_2()
                .px_3()
                .pt_2()
                .child(div().flex_none().size(px(7.)).rounded_full().bg(c(*color)))
                .child(div().text_size(px(11.)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(theme::FG_DIM)).child(SharedString::from(title.to_uppercase())))
                .child(crate::git_style::count_pill(*n))
                .into_any_element(),
            ChangeItem::Dir(rel) => row()
                .gap_2()
                .pl_4()
                .pr_3()
                .child(icon(Ico::Folder, 13., c(theme::FG_DIM)))
                .child(div().flex_1().min_w_0().truncate().text_size(px(11.5)).text_color(c(theme::FG_DIM)).child(SharedString::from(rel.clone())))
                .into_any_element(),
            ChangeItem::Empty => row()
                .gap_2()
                .px_3()
                .child(icon(Ico::Check, 14., c(theme::GIT_ADDED)))
                .child(div().text_color(c(theme::FG_DIM)).child("Không có thay đổi nào"))
                .into_any_element(),
            ChangeItem::File(k, g, ch) => {
                let (selected, cursor, by_dir) = {
                    let ui = self.git_ui.as_ref().unwrap();
                    (ui.selected.contains(&(*g, ch.path.clone())), ui.cursor, ui.by_dir)
                };
                self.change_row(root, ch, *g, *k, selected, cursor == Some(*k), by_dir, cx)
            }
            ChangeItem::Stash(i, message) => {
                let i = *i;
                let op = |label: &'static str, op: &'static str, id: usize| {
                    div()
                        .id(("stash-op", id))
                        .px_2()
                        .rounded_full()
                        .text_size(px(11.))
                        .text_color(c(theme::FG_DIM))
                        .hover(|s| s.bg(c(theme::ROW_HOVER)).text_color(c(theme::FG)))
                        .child(label)
                        .on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, cx| {
                            ws.run_git("Stash", move |r| vcs::stash_op(r, op, i), move |ws, r, cx| {
                                if let Err(e) = r {
                                    ws.message = Some(format!("Stash {op} lỗi: {}", crate::git_support::first_line(&e)));
                                    ws.open_git_if_conflicts(cx);
                                }
                                ws.reload_clean_tabs(cx);
                            }, cx)
                        }))
                };
                row()
                    .gap_2()
                    .mx_1()
                    .px_3()
                    .rounded_md()
                    .hover(|s| s.bg(c(theme::ROW_HOVER)))
                    .child(icon(Ico::Stash, 13., c(theme::FG_DIM)))
                    .child(div().flex_1().min_w_0().truncate().text_color(c(theme::FG)).child(SharedString::from(message.clone())))
                    .child(div().flex_none().text_size(px(11.)).text_color(c(theme::FG_DIM)).child(SharedString::from(format!("@{{{i}}}"))))
                    .child(op("Áp dụng", "apply", i * 3))
                    .child(op("Pop", "pop", i * 3 + 1))
                    .child(op("Xóa", "drop", i * 3 + 2))
                    .into_any_element()
            }
        }
    }

    /// The commit message and buttons beside the file list.
    pub(crate) fn render_commit_box(&mut self, branch: &str, tree: &GitTree, full: bool, cx: &mut Context<Self>) -> AnyElement {
        let ui = self.git_ui.as_ref().unwrap();
        let staged = tree.changes.iter().filter(|c| c.staged()).count();
        let tracked = tree.changes.iter().filter(|c| c.unstaged()).count();
        let merging = tree.state == Some(RepoState::Merging);
        let note = if tree.changes.iter().any(|c| c.conflicted()) {
            "Còn xung đột - giải quyết trước khi commit".to_string()
        } else if merging {
            "Commit sẽ kết thúc merge".to_string()
        } else if staged > 0 {
            format!("{staged} file đã stage sẽ được commit")
        } else if ui.amend {
            "Sửa commit trước (không thêm file)".to_string()
        } else if !ui.selected.is_empty() {
            format!("Chưa stage gì: commit {} file đang chọn", ui.selected.len())
        } else if tracked > 0 {
            format!("Chưa stage gì: commit mọi thay đổi đã theo dõi ({tracked} file)")
        } else {
            "Chưa có gì để commit".to_string()
        };
        let amend = ui.amend;
        let input = ui.msg.input.clone();
        let btn = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .flex_1()
                .h(px(30.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .whitespace_nowrap()
                .when(primary, |d| d.bg(c(theme::ACCENT)).text_color(gpui::white()).font_weight(gpui::FontWeight::SEMIBOLD))
                .when(!primary, |d| d.border_1().border_color(c(theme::BORDER)).text_color(c(theme::FG)))
                .hover(|s| s.opacity(0.85))
                .child(label)
        };
        div()
            .key_context("CommitBox")
            .on_action(cx.listener(|ws, _: &crate::actions::GitCommitNow, _, cx| ws.do_commit(false, cx)))
            .when(!full, |d| d.w(px(360.)).border_l_1().border_color(c(theme::BORDER)))
            .when(full, |d| d.w_full().border_t_1().border_color(c(theme::BORDER)))
            .flex_none()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(crate::ui_icons::icon(crate::ui_icons::Ico::Commit, 15., c(theme::FG_DIM)))
                    .child(div().font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(theme::FG)).child("Commit"))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(11.5)).text_color(c(theme::FG_DIM)).child(SharedString::from(format!("vào {branch}")))),
            )
            .child(div().min_h(px(64.)).px_3().py_2().rounded(px(10.)).border_1().border_color(c(theme::BORDER)).child(input))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id("git-amend")
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .text_size(px(12.))
                            .text_color(c(theme::FG_DIM))
                            .hover(|s| s.text_color(c(theme::FG)))
                            .tooltip(crate::ui_icons::tip("Sửa commit trước thay vì tạo commit mới"))
                            .child(crate::git_style::check_box(amend, false))
                            .child("Amend")
                            .on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| {
                                let root = ws.git.root.clone();
                                let ui = ws.git_ui(cx);
                                ui.amend = !ui.amend;
                                // like IntelliJ: amend starts from the previous message
                                if ui.amend && ui.msg.text(cx).is_empty() {
                                    let last = root.as_deref().map(vcs::last_message).unwrap_or_default();
                                    ui.msg.set(last.lines().next().unwrap_or(""), cx);
                                }
                                cx.notify()
                            })),
                    )
                    .child(div().flex_1()),
            )
            // what Commit will do, on a line of its own (it used to be squeezed next to Amend)
            .child(div().w_full().text_size(px(11.)).text_color(c(theme::FG_DIM)).child(SharedString::from(note)))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .child(btn("git-commit-go", "Commit", true).tooltip(crate::ui_icons::tip("Commit (Enter)")).on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.do_commit(false, cx))))
                    .child(btn("git-commit-push", "Commit & Push", false).on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.do_commit(true, cx)))),
            )
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn change_row(&mut self, root: &Path, ch: &Change, group: char, k: usize, selected: bool, is_cursor: bool, by_dir: bool, cx: &mut Context<Self>) -> AnyElement {
        let name = if by_dir { crate::tree::file_name(&ch.path) } else { ch.path.strip_prefix(root).unwrap_or(&ch.path).display().to_string() };
        let label = match &ch.orig {
            Some(o) => format!("{} → {name}", o.strip_prefix(root).unwrap_or(o).display()),
            None => name,
        };
        let letter = if group == 'S' { ch.index } else if group == 'U' { '?' } else if group == 'C' { '!' } else { ch.worktree };
        let color = match group {
            'C' => theme::ERROR,
            'U' => theme::GIT_UNTRACKED,
            _ if letter == 'A' => theme::GIT_ADDED,
            _ if letter == 'D' => theme::FG_DIM,
            _ => theme::GIT_MODIFIED,
        };
        let path = ch.path.clone();
        let paths = ch.paths();
        // flat list: name first, its folder dimmed after it
        let (name, dir) = if by_dir || ch.orig.is_some() {
            (label, String::new())
        } else {
            let rel = ch.path.strip_prefix(root).unwrap_or(&ch.path);
            (crate::tree::file_name(&ch.path), rel.parent().map(|d| d.display().to_string()).unwrap_or_default())
        };
        let deleted = letter == 'D';
        div()
            .id(("git-row", k))
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .mx_1()
            .h(px(28.))
            .pl(px(if by_dir { 30. } else { 12. }))
            .pr_3()
            .rounded_md()
            .when(selected, |d| d.bg(c(theme::SELECTION)))
            .when(is_cursor && !selected, |d| d.border_1().border_color(c(theme::BORDER)))
            .hover(|s| s.bg(c(theme::ROW_HOVER)))
            .child(
                div()
                    .id(("git-check", k))
                    .flex_none()
                    .p(px(3.))
                    .rounded_sm()
                    .hover(|s| s.bg(c(theme::ROW_HOVER)))
                    .tooltip(crate::ui_icons::tip(match group {
                        'S' => "Unstage",
                        'C' => "Xung đột",
                        _ => "Stage",
                    }))
                    .child(crate::git_style::check_box(group == 'S', group == 'C'))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, cx| {
                        cx.stop_propagation();
                        let p = paths.clone();
                        match group {
                            'S' => ws.run_git_simple("Unstage", move |r| vcs::unstage(r, &p), cx),
                            'C' => {}
                            _ => ws.run_git_simple("Stage", move |r| vcs::stage(r, &p), cx),
                        }
                    })),
            )
            .child(crate::icons::file_icon(&ch.path))
            .child(div().flex_none().max_w(px(320.)).truncate().text_color(c(if deleted { theme::FG_DIM } else { theme::FG })).when(deleted, |d| d.line_through()).child(SharedString::from(name)))
            .child(div().flex_1().min_w_0().truncate().text_size(px(11.5)).text_color(c(theme::FG_DIM)).child(SharedString::from(dir)))
            .child(crate::git_style::status_badge(letter, color))
            .when(group == 'C', |d| {
                let p = path.clone();
                d.child(div().id(("git-resolve", k)).px_1().rounded_sm().text_color(c(theme::FG_DIM)).hover(|s| s.bg(c(theme::ROW_HOVER)).text_color(c(theme::FG))).child("Giải quyết…").on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |ws, _, _, cx| {
                        cx.stop_propagation();
                        ws.open_conflicts(p.clone(), cx)
                    }),
                ))
            })
            .on_mouse_down(MouseButton::Left, cx.listener(move |ws, ev: &MouseDownEvent, window, cx| ws.git_click(k, ev, window, cx)))
            .on_mouse_down(MouseButton::Right, cx.listener(move |ws, ev: &MouseDownEvent, _, cx| ws.git_row_menu(k, ev, cx)))
            .into_any_element()
    }

    fn git_click(&mut self, k: usize, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let entries = self.git_entries();
        let Some((g, ch)) = entries.get(k).cloned() else { return };
        let ui = self.git_ui(cx);
        window.focus(&ui.focus);
        let key = (g, ch.path.clone());
        if let (true, Some(a)) = (ev.modifiers.shift, ui.anchor) {
            ui.selected = entries[a.min(k)..=a.max(k)].iter().map(|(g, c)| (*g, c.path.clone())).collect();
        } else if ev.modifiers.control || ev.modifiers.platform {
            if !ui.selected.remove(&key) {
                ui.selected.insert(key);
            }
            ui.anchor = Some(k);
        } else {
            ui.selected = [key].into();
            ui.anchor = Some(k);
        }
        ui.cursor = Some(k);
        if ev.click_count >= 2 {
            self.git_open_entry(g, &ch, window, cx);
        }
        cx.notify();
    }

    fn git_row_menu(&mut self, k: usize, ev: &MouseDownEvent, cx: &mut Context<Self>) {
        let entries = self.git_entries();
        let Some((g, ch)) = entries.get(k).cloned() else { return };
        let ui = self.git_ui(cx);
        if !ui.selected.contains(&(g, ch.path.clone())) {
            ui.selected = [(g, ch.path.clone())].into();
            ui.anchor = Some(k);
        }
        ui.cursor = Some(k);
        let sel = self.git_selection();
        let has = |x: char| sel.iter().any(|(g, _)| *g == x);
        let mut items = Vec::new();
        if sel.len() == 1 {
            items.push(menu_item(if g == 'C' { "Giải quyết xung đột…" } else { "So sánh (diff)" }, "Enter", MenuAction::GitSelOpen));
        }
        if has('W') || has('U') {
            items.push(menu_item("+ Stage", "Space", MenuAction::GitSelStage));
        }
        if has('C') {
            items.push(menu_item("Đánh dấu đã giải quyết (stage)", "", MenuAction::GitSelStage));
        }
        if has('S') {
            items.push(menu_item("− Unstage", "Space", MenuAction::GitSelUnstage));
        }
        if has('W') || has('U') || has('S') {
            items.push(menu_item("↺ Hủy thay đổi…", "Delete", MenuAction::GitSelDiscard));
        }
        if sel.len() == 1 {
            items.push(menu_sep());
            if ch.path.exists() {
                items.push(menu_item("Mở file", "", MenuAction::OpenFile(ch.path.clone())));
                items.push(menu_item("Hiện trong cây project", "", MenuAction::RevealInTree(ch.path.clone())));
            }
            if g != 'U' {
                items.push(menu_item("History file này", "", MenuAction::GitHistory(ch.path.clone())));
            }
            items.push(menu_item("Copy Path", "", MenuAction::CopyPath(ch.path.clone())));
        }
        self.show_menu(ev.position, items, cx);
    }

    fn git_move(&mut self, d: isize, extend: bool, cx: &mut Context<Self>) {
        let entries = self.git_entries();
        if entries.is_empty() {
            return;
        }
        let ui = self.git_ui(cx);
        let k = match ui.cursor {
            Some(i) => (i as isize + d).clamp(0, entries.len() as isize - 1) as usize,
            None => 0,
        };
        ui.cursor = Some(k);
        if extend {
            let a = *ui.anchor.get_or_insert(k);
            ui.selected = entries[a.min(k)..=a.max(k)].iter().map(|(g, c)| (*g, c.path.clone())).collect();
        } else {
            ui.selected = [(entries[k].0, entries[k].1.path.clone())].into();
            ui.anchor = Some(k);
        }
        cx.notify();
    }

    /// Space: stage the selection, or unstage it when all of it is staged.
    pub(crate) fn git_toggle_stage_sel(&mut self, cx: &mut Context<Self>) {
        let sel = self.git_selection();
        if sel.is_empty() {
            return;
        }
        if sel.iter().all(|(g, _)| *g == 'S') {
            self.git_unstage_sel(cx)
        } else {
            self.git_stage_sel(cx)
        }
    }

    pub(crate) fn git_stage_sel(&mut self, cx: &mut Context<Self>) {
        let paths: Vec<PathBuf> = self.git_selection().iter().filter(|(g, _)| *g != 'S').flat_map(|(_, c)| c.paths()).collect();
        if !paths.is_empty() {
            self.run_git_simple("Stage", move |r| vcs::stage(r, &paths), cx);
        }
    }

    pub(crate) fn git_unstage_sel(&mut self, cx: &mut Context<Self>) {
        let paths: Vec<PathBuf> = self.git_selection().iter().filter(|(g, _)| *g == 'S').flat_map(|(_, c)| c.paths()).collect();
        if !paths.is_empty() {
            self.run_git_simple("Unstage", move |r| vcs::unstage(r, &paths), cx);
        }
    }

    pub(crate) fn git_open_sel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((g, ch)) = self.git_selection().into_iter().next() {
            self.git_open_entry(g, &ch, window, cx);
        }
    }

    fn git_open_entry(&mut self, g: char, ch: &Change, _window: &mut Window, cx: &mut Context<Self>) {
        match g {
            'C' => self.open_conflicts(ch.path.clone(), cx),
            'S' => self.load_diff(ch.path.clone(), DiffMode::Staged, None, cx),
            _ => self.load_diff(ch.path.clone(), DiffMode::WorkTree, None, cx),
        }
    }

    pub(crate) fn git_discard_sel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut list: Vec<Change> = Vec::new();
        for (g, c) in self.git_selection() {
            if g != 'C' && !list.iter().any(|x| x.path == c.path) {
                list.push(c);
            }
        }
        if list.is_empty() {
            return;
        }
        let new_files = list.iter().filter(|c| c.untracked() || c.index == 'A').count();
        let names: Vec<String> = list.iter().take(12).map(|c| format!("  {}", c.path.display())).collect();
        let mut detail = format!(
            "Mọi thay đổi chưa commit (cả phần đã stage) sẽ mất, về bản HEAD{}:\n{}",
            if new_files > 0 { format!("; {new_files} file mới sẽ bị XÓA") } else { String::new() },
            names.join("\n")
        );
        if list.len() > 12 {
            detail.push_str(&format!("\n  … và {} file nữa", list.len() - 12));
        }
        let title = if list.len() == 1 { format!("Hủy thay đổi {}?", crate::tree::file_name(&list[0].path)) } else { format!("Hủy thay đổi {} file?", list.len()) };
        self.open_confirm_pub(title, detail, crate::context_menu::ConfirmAction::GitDiscard(list), window, cx);
    }

    pub(crate) fn git_discard(&mut self, list: Vec<Change>, cx: &mut Context<Self>) {
        self.run_git("Hủy thay đổi", move |root| {
            let errs: Vec<String> = list.iter().filter_map(|c| vcs::discard(root, c).err()).collect();
            if errs.is_empty() { Ok(String::new()) } else { Err(errs.join("\n")) }
        }, |ws, r, cx| {
            if let Err(e) = r {
                ws.message = Some(format!("Hủy thay đổi lỗi: {}", crate::git_support::first_line(&e)));
            }
            ws.reload_clean_tabs(cx);
        }, cx);
    }

    /// Tabs whose file git just rewrote: reload the ones without unsaved edits.
    pub(crate) fn reload_clean_tabs(&mut self, cx: &mut Context<Self>) {
        for t in self.tabs.clone() {
            let Some(p) = t.read(cx).path().cloned() else { continue };
            if t.read(cx).is_dirty() || t.read(cx).read_only {
                continue;
            }
            let Ok(disk) = std::fs::read_to_string(&p) else { continue };
            if t.read(cx).editor.buffer.text() != disk {
                t.update(cx, |v, cx| {
                    v.editor.buffer.reload_text(&disk);
                    cx.notify()
                });
            }
        }
    }

    // ---- side-by-side diff -------------------------------------------------------------------

    /// Compares `path` at `rev` (None = HEAD) with the text in the editor / on disk.
    pub(crate) fn open_diff(&mut self, path: PathBuf, rev: Option<String>, _window: &mut Window, cx: &mut Context<Self>) {
        self.load_diff(path, DiffMode::Rev(rev.unwrap_or_else(|| "HEAD".into())), None, cx)
    }

    fn file_text_now(&self, path: &Path, cx: &Context<Self>) -> String {
        match self.tabs.iter().find(|t| t.read(cx).path().map(|p| p.as_path()) == Some(path)) {
            Some(t) => t.read(cx).editor.buffer.text(),
            None => std::fs::read_to_string(path).unwrap_or_default(),
        }
    }

    /// Loads both sides in the background, then shows them; `keep`: the block to stay on
    /// (after staging / reverting one).
    pub(crate) fn load_diff(&mut self, path: PathBuf, mode: DiffMode, keep: Option<usize>, cx: &mut Context<Self>) {
        let Some(root) = self.git.root.clone() else { return };
        let current = self.file_text_now(&path, cx);
        let (p, m) = (path.clone(), mode.clone());
        cx.spawn(async move |this, cx| {
            let sides = cx
                .background_executor()
                .spawn(async move {
                    match &m {
                        DiffMode::WorkTree => {
                            let old = vcs::index_text(&root, &p).or_else(|| vcs::head_text(&root, &p)).unwrap_or_default();
                            ("Đã stage (index)".to_string(), "Bản hiện tại".to_string(), old, current)
                        }
                        DiffMode::Staged => ("HEAD".into(), "Đã stage (index)".into(), vcs::head_text(&root, &p).unwrap_or_default(), vcs::index_text(&root, &p).unwrap_or_default()),
                        DiffMode::Rev(r) => (r.clone(), "Bản hiện tại".into(), vcs::file_at(&root, r, &p).unwrap_or_default(), current),
                        DiffMode::Commit => (String::new(), String::new(), String::new(), String::new()),
                    }
                })
                .await;
            let _ = this.update(cx, |ws, cx| {
                let (lt, rt, old, new) = sides;
                ws.show_diff(path, mode, lt, rt, old, new, keep, cx);
            });
        })
        .detach();
    }

    /// Shows a diff of two texts (also a file of a commit in the log).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn show_diff(&mut self, path: PathBuf, mode: DiffMode, left_title: String, right_title: String, old: String, new: String, keep: Option<usize>, cx: &mut Context<Self>) {
        let (rows, blocks) = vcs::side_by_side_hunks(&old, &new);
        if blocks.is_empty() {
            let name = crate::tree::file_name(&path);
            self.message = Some(if old != new {
                format!("{name}: chỉ khác kiểu xuống dòng (CRLF/LF) hoặc dòng trống cuối file so với {left_title}")
            } else if keep.is_some() {
                format!("{name}: hết khác biệt")
            } else {
                format!("{name} không khác {left_title}")
            });
            if keep.is_some() {
                self.diff_view = None;
            }
            return cx.notify();
        }
        let current = keep.unwrap_or(0).min(blocks.len() - 1);
        let focus = self.diff_view.as_ref().map(|v| v.focus.clone()).unwrap_or_else(|| cx.focus_handle());
        let fold = self.diff_view.as_ref().is_none_or(|v| v.fold);
        let stats = rows.iter().fold((0, 0), |(a, d), r| match r.kind {
            RowKind::Added => (a + 1, d),
            RowKind::Removed => (a, d + 1),
            RowKind::Changed => (a + 1, d + 1),
            RowKind::Same => (a, d),
        });
        let mut view = DiffView {
            path,
            left_title,
            right_title,
            rows,
            blocks,
            current,
            scroll: UniformListScrollHandle::new(),
            mode,
            old_text: old,
            new_text: new,
            focus,
            needs_focus: true,
            fold,
            opened: BTreeSet::new(),
            display: Vec::new(),
            stats,
        };
        view.redisplay();
        view.scroll_to_current();
        self.diff_view = Some(view);
        cx.notify();
    }

    fn diff_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(v) = self.diff_view.as_mut() else { return };
        let n = v.blocks.len();
        v.current = if forward { (v.current + 1) % n } else { (v.current + n - 1) % n };
        v.scroll_to_current();
        cx.notify();
    }

    fn diff_toggle_fold(&mut self, cx: &mut Context<Self>) {
        let Some(v) = self.diff_view.as_mut() else { return };
        v.fold = !v.fold;
        v.opened.clear();
        v.redisplay();
        v.scroll_to_current();
        cx.notify();
    }

    /// `stage`: stage (index ↔ work tree) / unstage (HEAD ↔ index) the current block;
    /// otherwise revert it in the work tree (the editor buffer when the file is open).
    pub(crate) fn diff_block_action(&mut self, stage: bool, cx: &mut Context<Self>) {
        let Some(v) = self.diff_view.as_ref() else { return };
        let (path, mode, current) = (v.path.clone(), v.mode.clone(), v.current);
        let hunk = v.blocks[current].1.clone();
        match (&mode, stage) {
            (DiffMode::WorkTree, true) => {
                let new = v.new_text.clone();
                // stage what is on disk: save the edited tab first
                if let Some(t) = self.tabs.iter().find(|t| t.read(cx).path() == Some(&path)).cloned() {
                    if t.read(cx).is_dirty() {
                        t.update(cx, |v, _| v.save());
                    }
                }
                let (p, m) = (path.clone(), mode.clone());
                self.run_git("Stage khối", move |r| vcs::stage_hunk(r, &p, &new, &hunk), move |ws, r, cx| match r {
                    Ok(_) => ws.load_diff(path, m, Some(current), cx),
                    Err(e) => ws.message = Some(format!("Stage khối lỗi: {}", crate::git_support::first_line(&e))),
                }, cx);
            }
            (DiffMode::Staged, true) => {
                let (p, m) = (path.clone(), mode.clone());
                self.run_git("Unstage khối", move |r| vcs::unstage_hunk(r, &p, &hunk), move |ws, r, cx| match r {
                    Ok(_) => ws.load_diff(path, m, Some(current), cx),
                    Err(e) => ws.message = Some(format!("Unstage khối lỗi: {}", crate::git_support::first_line(&e))),
                }, cx);
            }
            (DiffMode::WorkTree | DiffMode::Rev(_), false) => {
                let text = vcs::apply_hunk(&v.new_text, &v.old_text, &vcs::reverse(&hunk));
                self.write_file_text(&path, &text, cx);
                self.refresh_git(cx);
                self.load_diff(path, mode, Some(current), cx);
            }
            _ => {}
        }
    }

    pub(crate) fn render_diff_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let v = self.diff_view.as_mut()?;
        if std::mem::take(&mut v.needs_focus) {
            crate::dialogs::focus_later(v.focus.clone(), window, cx);
        }
        let v = self.diff_view.as_ref()?;
        let rows = v.rows.clone();
        let display = v.display.clone();
        let starts: Vec<usize> = v.blocks.iter().map(|b| b.0).collect();
        let current = v.current;
        let this = cx.entity().downgrade();
        let list = uniform_list("diff-rows", display.len(), move |range, _, _| {
            range
                .map(|k| {
                    let this = this.clone();
                    match display[k] {
                        Disp::Fold(start, n) => div()
                            .id(("diff-fold", k))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .w_full()
                            .h(px(22.))
                            .px_3()
                            .bg(c(theme::CURRENT_LINE))
                            .text_size(px(11.5))
                            .text_color(c(theme::FG_DIM))
                            .cursor_pointer()
                            .hover(|s| s.text_color(c(theme::ACCENT)))
                            .child(icon(Ico::ChevronDown, 12., c(theme::FG_DIM)))
                            .child(SharedString::from(format!("{n} dòng không đổi")))
                            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                let _ = this.update(cx, |ws, cx| {
                                    if let Some(v) = ws.diff_view.as_mut() {
                                        v.opened.insert(start);
                                        v.redisplay();
                                        cx.notify()
                                    }
                                });
                            })
                            .into_any_element(),
                        Disp::Row(i) => {
                            let r = &rows[i];
                            let block = (r.kind != RowKind::Same).then(|| starts.iter().rposition(|&s| s <= i)).flatten();
                            div()
                                .id(("diff-row", i))
                                .flex()
                                .flex_row()
                                .w_full()
                                .child(div().w(px(3.)).flex_none().h_full().when(block == Some(current), |d| d.bg(c(theme::ACCENT))))
                                .children(diff_sides(r))
                                .when_some(block, |d, b| {
                                    d.cursor_pointer().on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                        let _ = this.update(cx, |ws, cx| {
                                            if let Some(v) = ws.diff_view.as_mut() {
                                                v.current = b;
                                                cx.notify()
                                            }
                                        });
                                    })
                                })
                                .into_any_element()
                        }
                    }
                })
                .collect()
        })
        .track_scroll(v.scroll.clone())
        .size_full();
        let path = v.path.clone();
        let mode = v.mode.clone();
        let chip = |text: &str, color: u32| {
            div()
                .flex_none()
                .px(px(7.))
                .py(px(1.))
                .rounded_full()
                .bg(tint(color, 0.14))
                .text_size(px(11.5))
                .text_color(c(color))
                .whitespace_nowrap()
                .child(SharedString::from(text.to_string()))
        };
        let side_title = |text: &str, color: u32| {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .px_3()
                .py(px(5.))
                .child(div().flex_none().size(px(7.)).rounded_full().bg(c(color)))
                .child(div().min_w_0().truncate().text_size(px(11.5)).text_color(c(theme::FG_DIM)).child(SharedString::from(text.to_string())))
        };
        let pill = |id: &'static str, label: &'static str, ico: Ico| {
            div()
                .id(id)
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .px(px(10.))
                .h(px(26.))
                .rounded_full()
                .border_1()
                .border_color(c(theme::BORDER))
                .text_size(px(12.))
                .text_color(c(theme::FG))
                .whitespace_nowrap()
                .hover(|s| s.bg(c(theme::ROW_HOVER)))
                .child(icon(ico, 13., c(theme::FG_DIM)))
                .child(label)
        };
        let (added, removed) = v.stats;
        let mut bar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_3()
            .py(px(8.))
            .border_b_1()
            .border_color(c(theme::BORDER))
            .child(icon(Ico::Commit, 16., c(theme::ACCENT)))
            .child(div().flex_none().font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(theme::FG)).child(SharedString::from(crate::tree::file_name(&v.path))))
            .child(chip(&format!("+{added}"), ADD_COLOR))
            .child(chip(&format!("−{removed}"), DEL_COLOR))
            .child(div().flex_1())
            .child(
                div()
                    .id("diff-fold")
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .px(px(8.))
                    .h(px(26.))
                    .rounded_full()
                    .text_size(px(12.))
                    .text_color(c(if v.fold { theme::ACCENT } else { theme::FG_DIM }))
                    .when(v.fold, |d| d.bg(tint(theme::ACCENT, 0.14)))
                    .hover(|s| s.bg(c(theme::ROW_HOVER)))
                    .tooltip(tip("Ẩn / hiện các dòng không đổi"))
                    .child(icon(Ico::List, 13., c(if v.fold { theme::ACCENT } else { theme::FG_DIM })))
                    .child("Thu gọn")
                    .on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.diff_toggle_fold(cx))),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .rounded_full()
                    .border_1()
                    .border_color(c(theme::BORDER))
                    .child(icon_btn("diff-prev", Ico::ChevronUp, "Khối trước (Shift+F7)").on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.diff_step(false, cx))))
                    .child(div().px_1().text_size(px(12.)).text_color(c(theme::FG)).whitespace_nowrap().child(SharedString::from(format!("{} / {}", current + 1, v.blocks.len()))))
                    .child(icon_btn("diff-next", Ico::ChevronDown, "Khối sau (F7)").on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.diff_step(true, cx)))),
            );
        match mode {
            DiffMode::WorkTree => {
                bar = bar
                    .child(pill("diff-stage", "Stage", Ico::Plus).tooltip(tip("Stage khối này (Ctrl+Alt+A)")).on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.diff_block_action(true, cx))))
                    .child(pill("diff-revert", "Hoàn tác", Ico::Refresh).tooltip(tip("Hoàn tác khối này (Ctrl+Alt+Z)")).on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.diff_block_action(false, cx))));
            }
            DiffMode::Staged => {
                bar = bar.child(pill("diff-unstage", "Unstage", Ico::Minus).tooltip(tip("Unstage khối này (Ctrl+Alt+A)")).on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.diff_block_action(true, cx))));
            }
            DiffMode::Rev(_) => {
                bar = bar.child(pill("diff-revert", "Hoàn tác", Ico::Refresh).tooltip(tip("Hoàn tác khối này (Ctrl+Alt+Z)")).on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.diff_block_action(false, cx))));
            }
            DiffMode::Commit => {}
        }
        if path.exists() {
            bar = bar.child(icon_btn("diff-open", Ico::Folder, "Mở file trong editor").on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, window, cx| {
                ws.diff_view = None;
                ws.log_view = None;
                ws.open_file(path.clone(), window, cx)
            })));
        }
        bar = bar.child(icon_btn("diff-close", Ico::Close, "Đóng (Esc)").on_mouse_down(MouseButton::Left, cx.listener(|ws, _, window, cx| ws.close_diff(window, cx))));
        let titles = div()
            .flex()
            .flex_row()
            .border_b_1()
            .border_color(c(theme::BORDER))
            .child(side_title(&v.left_title, DEL_COLOR).border_r_1().border_color(c(theme::BORDER)))
            .child(side_title(&v.right_title, ADD_COLOR));
        let focus = v.focus.clone();
        Some(
            div()
                .occlude()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .p(px(24.))
                .bg(gpui::rgba(0x00000066))
                .child(
                    div()
                        .key_context("DiffView")
                        .track_focus(&focus)
                        .size_full()
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .bg(c(theme::POPUP_BG))
                        .border_1()
                        .border_color(c(theme::BORDER))
                        .rounded(px(14.))
                        .shadow_lg()
                        .on_action(cx.listener(|ws, _: &crate::actions::DiffNext, _, cx| ws.diff_step(true, cx)))
                        .on_action(cx.listener(|ws, _: &crate::actions::DiffPrev, _, cx| ws.diff_step(false, cx)))
                        .on_action(cx.listener(|ws, _: &crate::actions::DiffStageBlock, _, cx| ws.diff_block_action(true, cx)))
                        .on_action(cx.listener(|ws, _: &crate::actions::DiffRevertBlock, _, cx| ws.diff_block_action(false, cx)))
                        .on_action(cx.listener(|ws, _: &crate::actions::CloseDiff, window, cx| ws.close_diff(window, cx)))
                        .child(bar)
                        .child(titles)
                        .child(div().flex_1().min_h_0().bg(c(theme::BG)).font_family(SharedString::from(self.settings.font_family.clone())).child(list)),
                )
                .into_any_element(),
        )
    }

    pub(crate) fn close_diff(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.diff_view = None;
        // back to whatever is under it: the log, the conflict resolver, the editor
        if let Some(l) = self.log_view.as_mut() {
            l.needs_focus = true;
        } else if let Some(cv) = self.conflict_view.as_mut() {
            cv.needs_focus = true;
        } else if let Some(i) = self.active {
            self.activate(i, window, cx);
        }
        cx.notify();
    }

    // ---- conflicts -------------------------------------------------------------------------

    pub(crate) fn open_conflicts(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let text = self.file_text_now(&path, cx);
        let change = self.git.tree.changes.iter().find(|c| c.path == path).cloned();
        let state = self.git.tree.state.unwrap_or(RepoState::Clean);
        self.conflict_view = Some(ConflictView {
            path: path.clone(),
            change,
            conflicts: vcs::parse_conflicts(&text),
            d3: Vec::new(),
            current: 0,
            history: Vec::new(),
            lines: text.lines().map(str::to_string).collect(),
            state,
            error: None,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            needs_focus: true,
            total: 0,
        });
        if let Some(v) = self.conflict_view.as_mut() {
            v.total = v.conflicts.len();
        }
        self.conflict_scroll();
        // the base of every conflict, from the index stages (markers often lack it)
        if let Some(root) = self.git.root.clone() {
            cx.spawn(async move |this, cx| {
                let p = path.clone();
                let d3 = cx.background_executor().spawn(async move { vcs::diff3_text(&root, &p).map(|t| vcs::parse_conflicts(&t)).unwrap_or_default() }).await;
                let _ = this.update(cx, |ws, cx| {
                    if let Some(v) = ws.conflict_view.as_mut().filter(|v| v.path == path) {
                        v.d3 = d3;
                        cx.notify()
                    }
                });
            })
            .detach();
        }
        cx.notify();
    }

    /// Writes the new text (the open tab's buffer, undoable, and the file).
    fn write_file_text(&mut self, path: &Path, text: &str, cx: &mut Context<Self>) {
        if let Some(t) = self.tabs.iter().find(|t| t.read(cx).path().map(|p| p.as_path()) == Some(path)).cloned() {
            t.update(cx, |v, cx| {
                v.editor.buffer.break_group();
                v.editor.anchor = 0;
                v.editor.head = v.editor.buffer.len();
                v.editor.insert(text);
                v.editor.buffer.break_group();
                v.save();
                cx.notify()
            });
        } else {
            let _ = std::fs::write(path, text);
        }
    }

    fn set_conflict_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let Some(path) = self.conflict_view.as_ref().map(|v| v.path.clone()) else { return };
        self.write_file_text(&path, text, cx);
        if let Some(v) = self.conflict_view.as_mut() {
            v.conflicts = vcs::parse_conflicts(text);
            v.lines = text.lines().map(str::to_string).collect();
            v.current = v.current.min(v.conflicts.len().saturating_sub(1));
            v.error = None;
        }
        self.conflict_scroll();
        cx.notify();
    }

    fn conflict_scroll(&mut self) {
        if let Some(v) = self.conflict_view.as_ref() {
            if let Some(cf) = v.conflicts.get(v.current) {
                v.scroll.scroll_to_item(cf.start, ScrollStrategy::Center);
            }
        }
    }

    /// The base lines of conflict `i`: from diff3 markers, or from the re-merge with the index.
    fn conflict_base(v: &ConflictView, i: usize) -> Option<Vec<String>> {
        let cf = v.conflicts.get(i)?;
        if !cf.base.is_empty() {
            return Some(cf.base.clone());
        }
        v.d3.iter().find(|d| d.ours == cf.ours && d.theirs == cf.theirs).map(|d| d.base.clone())
    }

    pub(crate) fn pick_conflict(&mut self, index: usize, pick: Pick, cx: &mut Context<Self>) {
        let Some(v) = self.conflict_view.as_ref() else { return };
        let Some(cf) = v.conflicts.get(index) else { return };
        let lines = match pick {
            Pick::Ours => cf.ours.clone(),
            Pick::Theirs => cf.theirs.clone(),
            Pick::Both => cf.ours.iter().chain(&cf.theirs).cloned().collect(),
            Pick::Base => match Self::conflict_base(v, index) {
                Some(b) => b,
                None => return,
            },
        };
        self.replace_conflict_with(index, lines, cx);
    }

    /// Both sides, the right one first.
    fn pick_conflict_both_reversed(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(cf) = self.conflict_view.as_ref().and_then(|v| v.conflicts.get(index)) else { return };
        let lines = cf.theirs.iter().chain(&cf.ours).cloned().collect();
        self.replace_conflict_with(index, lines, cx);
    }

    fn replace_conflict_with(&mut self, index: usize, lines: Vec<String>, cx: &mut Context<Self>) {
        let Some(path) = self.conflict_view.as_ref().map(|v| v.path.clone()) else { return };
        let text = self.file_text_now(&path, cx);
        let new = vcs::replace_conflict(&text, index, &lines);
        if let Some(v) = self.conflict_view.as_mut() {
            v.history.push(text);
        }
        self.set_conflict_text(&new, cx);
    }

    fn undo_conflict(&mut self, cx: &mut Context<Self>) {
        let Some(prev) = self.conflict_view.as_mut().and_then(|v| v.history.pop()) else { return };
        self.set_conflict_text(&prev, cx);
    }

    fn conflict_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(v) = self.conflict_view.as_mut() else { return };
        let n = v.conflicts.len();
        if n == 0 {
            return;
        }
        v.current = if forward { (v.current + 1) % n } else { (v.current + n - 1) % n };
        self.conflict_scroll();
        cx.notify();
    }

    fn take_whole_side(&mut self, ours: bool, cx: &mut Context<Self>) {
        let Some(v) = self.conflict_view.as_ref() else { return };
        let path = v.path.clone();
        let ch = v.change.clone().unwrap_or(Change { path: path.clone(), orig: None, index: 'U', worktree: 'U' });
        let (on, tn) = side_names(v.state);
        let side = if ours { on } else { tn };
        self.run_git("Lấy cả file", move |r| vcs::take_side(r, &ch, ours), move |ws, r, cx| match r {
            Ok(_) => {
                ws.reload_clean_tabs(cx);
                ws.conflict_view = None;
                ws.message = Some(format!("{}: lấy toàn bộ {side} - đã đánh dấu giải quyết", crate::tree::file_name(&path)));
            }
            Err(e) => {
                if let Some(v) = ws.conflict_view.as_mut() {
                    v.error = Some(e);
                }
            }
        }, cx);
    }

    pub(crate) fn finish_conflicts(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.conflict_view.as_ref().map(|v| v.path.clone()) else { return };
        let left = vcs::parse_conflicts(&self.file_text_now(&path, cx)).len();
        if left > 0 {
            if let Some(v) = self.conflict_view.as_mut() {
                v.error = Some(format!("Còn {left} chỗ xung đột chưa chọn"));
            }
            return cx.notify();
        }
        self.save_all_for_git(cx);
        let p = path.clone();
        self.run_git("Đánh dấu đã giải quyết", move |r| vcs::mark_resolved(r, &p), move |ws, r, _| match r {
            Ok(_) => {
                ws.conflict_view = None;
                ws.message = Some(format!("{}: đã giải quyết xung đột", crate::tree::file_name(&path)));
            }
            Err(e) => {
                if let Some(v) = ws.conflict_view.as_mut() {
                    v.error = Some(e);
                }
            }
        }, cx);
    }

    pub(crate) fn render_conflicts(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let v = self.conflict_view.as_mut()?;
        if std::mem::take(&mut v.needs_focus) {
            crate::dialogs::focus_later(v.focus.clone(), window, cx);
        }
        let v = self.conflict_view.as_ref()?;
        let name = crate::tree::file_name(&v.path);
        let path = v.path.clone();
        let (ours_name, theirs_name) = side_names(v.state);
        let font = SharedString::from(self.settings.font_family.clone());
        const OURS: u32 = theme::ACCENT;
        const THEIRS: u32 = theme::WARNING;
        let i = v.current;
        let n = v.conflicts.len();
        let total = v.total.max(n);
        let action = |id: &'static str, label: &'static str, color: u32| {
            div()
                .id(id)
                .flex_none()
                .px(px(10.))
                .h(px(24.))
                .flex()
                .items_center()
                .rounded_full()
                .bg(tint(color, 0.16))
                .text_size(px(12.))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(c(color))
                .whitespace_nowrap()
                .hover(|s| s.bg(tint(color, 0.28)))
                .child(label)
        };
        // one side of the current conflict: title, its "take" button, its lines
        let pane = |id: &'static str, title: String, sub: String, lines: Option<Vec<String>>, color: u32, take: Option<AnyElement>| {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .rounded(px(10.))
                .border_1()
                .border_color(tint(color, 0.45))
                .overflow_hidden()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py(px(5.))
                        .bg(tint(color, 0.10))
                        .child(div().flex_none().size(px(8.)).rounded_full().bg(c(color)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(div().truncate().text_size(px(12.)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(theme::FG)).child(SharedString::from(title)))
                                .when(!sub.is_empty(), |d| d.child(div().truncate().text_size(px(10.5)).text_color(c(theme::FG_DIM)).child(SharedString::from(sub)))),
                        )
                        .children(take),
                )
                .child(
                    div()
                        .id(id)
                        .flex()
                        .flex_col()
                        .h(px(180.))
                        .py_1()
                        .overflow_y_scroll()
                        .bg(c(theme::BG))
                        .font_family(font.clone())
                        .text_size(px(12.5))
                        .children(lines.clone().unwrap_or_default().into_iter().enumerate().map(|(k, l)| {
                            div()
                                .flex()
                                .flex_row()
                                .child(div().w(px(32.)).flex_none().pr_2().flex().justify_end().text_color(c(theme::GUTTER_FG)).child(SharedString::from((k + 1).to_string())))
                                .child(div().whitespace_nowrap().text_color(c(theme::FG)).child(SharedString::from(if l.is_empty() { " ".to_string() } else { l.replace('\t', "    ") })))
                        }))
                        .when(lines.as_ref().is_some_and(|l| l.is_empty()), |d| d.child(div().px_3().italic().text_color(c(theme::FG_DIM)).child("(trống)")))
                        .when(lines.is_none(), |d| d.child(div().px_3().italic().text_color(c(theme::FG_DIM)).child("(không có bản gốc)"))),
                )
        };
        let compare: AnyElement = match v.conflicts.get(i) {
            Some(cf) => {
                let base = Self::conflict_base(v, i);
                let has_base = base.is_some();
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .child(pane(
                                "cf-pane-ours",
                                format!("Trái · {ours_name}"),
                                cf.ours_label.clone(),
                                Some(cf.ours.clone()),
                                OURS,
                                Some(action("cf-ours", "← Lấy", OURS).tooltip(tip("Lấy bên trái (Alt+←)")).on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, cx| ws.pick_conflict(i, Pick::Ours, cx))).into_any_element()),
                            ))
                            .child(pane(
                                "cf-pane-base",
                                "Bản gốc".into(),
                                "tổ tiên chung (base)".into(),
                                base,
                                theme::FG_DIM,
                                has_base.then(|| action("cf-base", "Lấy", theme::FG_DIM).on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, cx| ws.pick_conflict(i, Pick::Base, cx))).into_any_element()),
                            ))
                            .child(pane(
                                "cf-pane-theirs",
                                format!("Phải · {theirs_name}"),
                                cf.theirs_label.clone(),
                                Some(cf.theirs.clone()),
                                THEIRS,
                                Some(action("cf-theirs", "Lấy →", THEIRS).tooltip(tip("Lấy bên phải (Alt+→)")).on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, cx| ws.pick_conflict(i, Pick::Theirs, cx))).into_any_element()),
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_center()
                            .gap_2()
                            .child(div().text_size(px(11.5)).text_color(c(theme::FG_DIM)).child("Giữ cả hai:"))
                            .child(action("cf-both", "Trái rồi phải", theme::MODIFIED).on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, cx| ws.pick_conflict(i, Pick::Both, cx))))
                            .child(action("cf-both2", "Phải rồi trái", theme::MODIFIED).on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, cx| ws.pick_conflict_both_reversed(i, cx)))),
                    )
                    .into_any_element()
            }
            None => {
                let (explain, done) = match v.change.as_ref().map(|c| (c.index, c.worktree)) {
                    Some(('D', 'U')) => (format!("Bên trái ({ours_name}) đã xóa file này, bên phải sửa nó. Chọn \"Cả file: phải\" để giữ file, \"Cả file: trái\" để xóa."), false),
                    Some(('U', 'D')) => (format!("Bên phải ({theirs_name}) đã xóa file này, bên trái sửa nó. Chọn \"Cả file: trái\" để giữ file, \"Cả file: phải\" để xóa."), false),
                    Some(('D', 'D')) => ("Cả hai bên đều xóa file này: lấy bên nào cũng là xóa.".to_string(), false),
                    _ => ("Đã xử lý hết xung đột. Kiểm tra kết quả bên dưới rồi bấm Đánh dấu đã giải quyết.".to_string(), true),
                };
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .m_3()
                    .p_3()
                    .rounded(px(10.))
                    .bg(tint(if done { theme::GIT_ADDED } else { theme::WARNING }, 0.12))
                    .child(icon(if done { Ico::Check } else { Ico::Problems }, 18., c(if done { theme::GIT_ADDED } else { theme::WARNING })))
                    .child(div().flex_1().text_color(c(theme::FG)).child(SharedString::from(explain)))
                    .into_any_element()
            }
        };
        // the whole file as it will be saved, conflicts tinted, the current one stronger
        let lines = v.lines.clone();
        let mut region: Vec<Option<usize>> = vec![None; lines.len()];
        for (k, cf) in v.conflicts.iter().enumerate() {
            for r in region.iter_mut().take(cf.end + 1).skip(cf.start) {
                *r = Some(k);
            }
        }
        let this = cx.entity().downgrade();
        let result = uniform_list("cf-result", lines.len(), move |range, _, _| {
            range
                .map(|li| {
                    let l = &lines[li];
                    let reg = region[li];
                    let marker = ["<<<<<<<", "=======", ">>>>>>>", "|||||||"].iter().any(|m| l.starts_with(m));
                    let cur = reg == Some(i);
                    let this = this.clone();
                    div()
                        .id(("cf-line", li))
                        .flex()
                        .flex_row()
                        .w_full()
                        .when(reg.is_some(), |d| d.bg(tint(if marker { DEL_COLOR } else { theme::WARNING }, if cur { 0.16 } else { 0.07 })).cursor_pointer())
                        .child(div().w(px(3.)).flex_none().h_full().when(cur, |d| d.bg(c(theme::ACCENT))))
                        .child(div().w(px(44.)).flex_none().pr_2().flex().justify_end().text_color(c(theme::GUTTER_FG)).child(SharedString::from((li + 1).to_string())))
                        .child(
                            div()
                                .whitespace_nowrap()
                                .text_color(c(if marker { DEL_COLOR } else { theme::FG }))
                                .when(marker, |d| d.italic())
                                .child(SharedString::from(if l.is_empty() { " ".to_string() } else { l.replace('\t', "    ") })),
                        )
                        .when_some(reg, |d, k| {
                            d.on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                let _ = this.update(cx, |ws, cx| {
                                    if let Some(v) = ws.conflict_view.as_mut() {
                                        v.current = k;
                                        cx.notify()
                                    }
                                });
                            })
                        })
                })
                .collect()
        })
        .track_scroll(v.scroll.clone())
        .size_full();
        let status = match (&v.error, n) {
            (Some(e), _) => div().text_color(c(theme::ERROR)).child(SharedString::from(e.clone())),
            (None, 0) => div().text_color(c(theme::GIT_ADDED)).child("Sẵn sàng đánh dấu đã giải quyết"),
            (None, n) => div().text_color(c(theme::FG_DIM)).child(SharedString::from(format!("Còn {n} chỗ cần chọn"))),
        };
        let can_undo = !v.history.is_empty();
        let done = total - n;
        let progress = div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(
                div()
                    .w(px(90.))
                    .h(px(5.))
                    .rounded_full()
                    .bg(c(theme::ROW_HOVER))
                    .child(div().h_full().rounded_full().bg(c(theme::GIT_ADDED)).w(px(if total == 0 { 90. } else { 90. * done as f32 / total as f32 }))),
            )
            .child(div().text_size(px(11.5)).text_color(c(theme::FG_DIM)).whitespace_nowrap().child(SharedString::from(format!("{done}/{total} đã xử lý"))));
        let bar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_3()
            .py(px(8.))
            .border_b_1()
            .border_color(c(theme::BORDER))
            .child(icon(Ico::Branch, 16., c(theme::WARNING)))
            .child(div().flex_none().font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(theme::FG)).child(SharedString::from(name)))
            .when(!v.state.label().is_empty(), |d| {
                d.child(div().flex_none().px(px(7.)).rounded_full().bg(tint(theme::WARNING, 0.14)).text_size(px(10.5)).font_weight(gpui::FontWeight::BOLD).text_color(c(theme::WARNING)).child(v.state.label()))
            })
            .child(div().flex_1())
            .child(progress)
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .rounded_full()
                    .border_1()
                    .border_color(c(theme::BORDER))
                    .child(icon_btn("cf-prev", Ico::ChevronUp, "Xung đột trước (Shift+F7)").on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.conflict_step(false, cx))))
                    .child(div().px_1().text_size(px(12.)).text_color(c(theme::FG)).whitespace_nowrap().child(SharedString::from(if n == 0 { "0 / 0".to_string() } else { format!("{} / {n}", i + 1) })))
                    .child(icon_btn("cf-next", Ico::ChevronDown, "Xung đột sau (F7)").on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.conflict_step(true, cx)))),
            )
            .when(can_undo, |d| d.child(icon_btn("cf-undo", Ico::History, "Hoàn tác lần chọn trước (Ctrl+Z)").on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.undo_conflict(cx)))))
            .child(icon_btn("cf-close", Ico::Close, "Đóng (Esc)").on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| {
                ws.conflict_view = None;
                cx.notify()
            })));
        let footer = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_3()
            .py(px(8.))
            .border_t_1()
            .border_color(c(theme::BORDER))
            .child(div().flex_1().min_w_0().text_size(px(12.)).child(status))
            .child(action("cf-ours-all", "Cả file: trái", OURS).tooltip(tip("Bỏ mọi thay đổi bên phải, lấy toàn bộ file bên trái")).on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.take_whole_side(true, cx))))
            .child(action("cf-theirs-all", "Cả file: phải", THEIRS).tooltip(tip("Bỏ mọi thay đổi bên trái, lấy toàn bộ file bên phải")).on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.take_whole_side(false, cx))))
            .child(action("cf-edit", "Sửa tay", theme::FG_DIM).tooltip(tip("Mở file trong editor để sửa tay")).on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, window, cx| {
                ws.conflict_view = None;
                ws.open_file(path.clone(), window, cx)
            })))
            .child(crate::dialogs::button("cf-done", "Đánh dấu đã giải quyết", true, cx, |ws: &mut Workspace, _, cx| ws.finish_conflicts(cx)));
        let focus = v.focus.clone();
        Some(
            div()
                .occlude()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .p(px(24.))
                .bg(gpui::rgba(0x00000066))
                .child(
                    div()
                        .key_context("ConflictView")
                        .track_focus(&focus)
                        .size_full()
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .bg(c(theme::POPUP_BG))
                        .border_1()
                        .border_color(c(theme::BORDER))
                        .rounded(px(14.))
                        .shadow_lg()
                        .on_action(cx.listener(|ws, _: &crate::actions::DiffNext, _, cx| ws.conflict_step(true, cx)))
                        .on_action(cx.listener(|ws, _: &crate::actions::DiffPrev, _, cx| ws.conflict_step(false, cx)))
                        .on_action(cx.listener(|ws, _: &crate::actions::ConflictTakeOurs, _, cx| {
                            let i = ws.conflict_view.as_ref().map(|v| v.current).unwrap_or(0);
                            ws.pick_conflict(i, Pick::Ours, cx)
                        }))
                        .on_action(cx.listener(|ws, _: &crate::actions::ConflictTakeTheirs, _, cx| {
                            let i = ws.conflict_view.as_ref().map(|v| v.current).unwrap_or(0);
                            ws.pick_conflict(i, Pick::Theirs, cx)
                        }))
                        .on_action(cx.listener(|ws, _: &crate::actions::ConflictUndo, _, cx| ws.undo_conflict(cx)))
                        .on_action(cx.listener(|ws, _: &crate::actions::CloseDiff, _, cx| {
                            ws.conflict_view = None;
                            cx.notify()
                        }))
                        .child(bar)
                        .child(div().flex_none().border_b_1().border_color(c(theme::BORDER)).child(compare))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .px_3()
                                .py(px(5.))
                                .child(div().text_size(px(10.5)).font_weight(gpui::FontWeight::BOLD).text_color(c(theme::FG_DIM)).child("KẾT QUẢ"))
                                .child(div().text_size(px(11.)).text_color(c(theme::FG_DIM)).child("file sẽ được lưu · bấm vào vùng xung đột để chọn")),
                        )
                        .child(div().flex_1().min_h_0().bg(c(theme::BG)).font_family(font).child(result))
                        .child(footer),
                )
                .into_any_element(),
        )
    }
}

/// Who left ("ours") and right ("theirs") are: during a rebase they are swapped (ours = the
/// branch rebased onto, theirs = your commit being replayed).
pub fn side_names(state: RepoState) -> (&'static str, &'static str) {
    match state {
        RepoState::Rebasing => ("nhánh đích (upstream)", "commit của bạn đang rebase"),
        RepoState::CherryPicking => ("của bạn (HEAD)", "commit đang cherry-pick"),
        RepoState::Reverting => ("của bạn (HEAD)", "bản revert"),
        _ => ("của bạn (HEAD)", "nhánh đang merge vào"),
    }
}

/// Left and right halves of a diff row; a changed pair highlights the characters that differ.
fn diff_sides(r: &DiffRow) -> [gpui::Div; 2] {
    let clean = |l: &Option<(usize, String)>| l.as_ref().map(|(_, t)| t.trim_end_matches('\r').replace('\t', "    ")).unwrap_or_default();
    let (lt, rt) = (clean(&r.left), clean(&r.right));
    let (ml, mr) = match (r.kind == RowKind::Changed).then(|| vcs::inline_diff(&lt, &rt)) {
        Some((a, b)) => (Some(a), Some(b)),
        None => (None, None),
    };
    let side = |line: &Option<(usize, String)>, text: String, left: bool, mid: Option<std::ops::Range<usize>>| {
        // removed on the left, added on the right; a changed pair is both
        let color = match (r.kind, left) {
            (RowKind::Same, _) => None,
            (RowKind::Changed, true) | (RowKind::Removed, true) => Some(DEL_COLOR),
            (RowKind::Changed, false) | (RowKind::Added, false) => Some(ADD_COLOR),
            _ => None,
        };
        // the other side of an added / removed line: an empty hatch-like filler
        let filler = line.is_none() && r.kind != RowKind::Same;
        let content: AnyElement = match mid.filter(|m| !m.is_empty() && m.len() < text.len()).zip(color) {
            Some((m, col)) => div()
                .flex()
                .flex_row()
                .whitespace_nowrap()
                .child(SharedString::from(text[..m.start].to_string()))
                .child(div().rounded(px(3.)).bg(tint(col, 0.38)).child(SharedString::from(text[m.clone()].to_string())))
                .child(SharedString::from(text[m.end..].to_string()))
                .into_any_element(),
            None => div().whitespace_nowrap().child(SharedString::from(text)).into_any_element(),
        };
        let sign = match color {
            Some(_) if line.is_some() => if left { "−" } else { "+" },
            _ => "",
        };
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_row()
            .overflow_hidden()
            .when_some(color.filter(|_| !filler), |d, col| d.bg(tint(col, 0.12)))
            .when(filler, |d| d.bg(c(theme::CURRENT_LINE)))
            .child(
                div()
                    .w(px(44.))
                    .flex_none()
                    .pr_2()
                    .flex()
                    .justify_end()
                    .when_some(color.filter(|_| !filler), |d, col| d.bg(tint(col, 0.10)))
                    .text_color(c(theme::GUTTER_FG))
                    .child(SharedString::from(line.as_ref().map(|(n, _)| (n + 1).to_string()).unwrap_or_default())),
            )
            .child(div().w(px(16.)).flex_none().flex().justify_center().text_color(color.map(c).unwrap_or(c(theme::GUTTER_FG))).child(sign))
            .child(div().text_color(c(theme::FG)).child(content))
    };
    [side(&r.left, lt, true, ml).border_r_1().border_color(c(theme::BORDER)), side(&r.right, rt, false, mr)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_long_unchanged_runs() {
        let old: String = (0..30).map(|i| format!("l{i}\n")).collect();
        let new = old.replace("l15\n", "X\n");
        let (rows, _) = vcs::side_by_side_hunks(&old, &new);
        let d = diff_display(&rows, true, &BTreeSet::new());
        // folds are keyed by the first row of their unchanged run
        assert_eq!(d[0], Disp::Fold(0, 12));
        assert_eq!(d[4], Disp::Row(15));
        assert_eq!(*d.last().unwrap(), Disp::Fold(16, 11));
        assert_eq!(diff_display(&rows, false, &BTreeSet::new()).len(), rows.len());
        assert!(!diff_display(&rows, true, &BTreeSet::from([0])).contains(&Disp::Fold(0, 12)));
    }
}
