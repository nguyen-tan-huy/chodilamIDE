//! Log window (SmartGit's Log / IntelliJ's Git Log): the commit graph of every branch (or the
//! current one, or one file's history) with branch / tag labels, a filter, the selected
//! commit's message and changed files (click = side-by-side diff), and what can be done on a
//! commit: checkout, new branch / tag, cherry-pick, revert, reset, merge, rebase.
use crate::context_menu::{ConfirmAction, MenuAction, PromptKind, menu_item, menu_sep};
use crate::dialogs::Field;
use crate::git_panel::DiffMode;
use crate::theme::{self, c};
use crate::workspace::Workspace;
use gpui::{
    AnyElement, Bounds, ClipboardItem, Context, FocusHandle, Hsla, MouseButton, MouseDownEvent, PathBuilder, ScrollStrategy, SharedString, UniformListScrollHandle, Window, canvas, div,
    fill, point, prelude::*, px, size, uniform_list,
};
use std::path::PathBuf;
use vcs::{CommitFile, GraphCommit, GraphRow};

const LANE_W: f32 = 12.;
const ROW_H: f32 = 28.;
const MAX_LANES: usize = 10;
/// Height of a row of the branch picker (uniform_list measures one row for all).
const PICK_ROW_H: f32 = 28.;

enum PickItem {
    All,
    Current,
    Heading(&'static str),
    /// name, is the current branch, is remote
    Branch(String, bool, bool),
}
const LANE_COLORS: [u32; 8] = [0x3574f0, 0x5fb865, 0xe0b050, 0xc77dbb, 0x4fb5c4, 0xe08a3c, 0xd1675a, 0x9e9e9e];

pub struct LogView {
    /// a file's history instead of the whole repository
    pub file: Option<PathBuf>,
    /// every branch (--all, the default); else `branches` (none = the current one)
    pub all: bool,
    pub branches: Vec<String>,
    /// the branch picker: every branch (name, is current), open or not
    pub branch_list: Vec<(String, bool)>,
    pub picker_open: bool,
    pub commits: Vec<GraphCommit>,
    pub rows: Vec<GraphRow>,
    pub filter: Field,
    /// index into `commits`
    pub selected: Option<usize>,
    pub details: String,
    pub files: Vec<CommitFile>,
    pub scroll: UniformListScrollHandle,
    /// the "Working Tree" row is selected (staging area on the right) instead of a commit
    pub working: bool,
    /// branches, remotes, tags for the left list
    pub refs: vcs::RefList,
    /// sections of the left list the user flipped from their default (local open, others closed)
    pub flipped: std::collections::HashSet<String>,
    pub focus: FocusHandle,
    pub needs_focus: bool,
    pub loading: bool,
}

/// What a context-menu entry of the log does.
#[derive(Clone, Debug)]
pub enum LogOp {
    CheckoutCommit(String),
    CheckoutBranch(String),
    CherryPick(String),
    Revert(String),
    /// "soft" / "mixed" ("hard" asks first)
    Reset(&'static str, String),
    Merge(String),
    Rebase(String),
    CopyHash(String),
    DeleteBranch(String),
    /// undo the last commit (HEAD): reset --soft HEAD~1
    UndoCommit,
}

#[derive(PartialEq)]
enum RefKind {
    Head,
    Local,
    Remote,
    Tag,
}

/// "HEAD -> main", "origin/main", "tag: v1", "HEAD" → name and kind.
fn ref_kind(r: &str, remotes: &[String]) -> (String, RefKind) {
    if let Some(b) = r.strip_prefix("HEAD -> ") {
        return (b.to_string(), RefKind::Head);
    }
    if r == "HEAD" {
        return ("HEAD".into(), RefKind::Head);
    }
    if let Some(t) = r.strip_prefix("tag: ") {
        return (t.to_string(), RefKind::Tag);
    }
    if remotes.iter().any(|rm| r.starts_with(&format!("{rm}/"))) {
        return (r.to_string(), RefKind::Remote);
    }
    (r.to_string(), RefKind::Local)
}

impl Workspace {
    pub(crate) fn open_log_view(&mut self, file: Option<PathBuf>, _window: &mut Window, cx: &mut Context<Self>) {
        let filter = Field::new("Lọc", "lọc theo message, tác giả, hash, nhánh…", "", cx);
        // a whole-repository window opens on the working tree when there is something to commit
        let (file_is_none, has_changes) = (file.is_none(), !self.git.tree.changes.is_empty());
        self.git_ui(cx);
        self.log_view = Some(LogView {
            all: file.is_none(),
            branches: Vec::new(),
            branch_list: Vec::new(),
            picker_open: false,
            file,
            commits: Vec::new(),
            rows: Vec::new(),
            filter,
            selected: None,
            details: String::new(),
            files: Vec::new(),
            scroll: UniformListScrollHandle::new(),
            working: file_is_none && has_changes,
            refs: vcs::RefList::default(),
            flipped: Default::default(),
            focus: cx.focus_handle(),
            needs_focus: true,
            loading: true,
        });
        self.reload_log(cx);
    }

    /// Re-reads the graph (after a git command changed it), keeping the selected commit.
    pub(crate) fn reload_log(&mut self, cx: &mut Context<Self>) {
        let (Some(root), Some(v)) = (self.git.root.clone(), self.log_view.as_mut()) else { return };
        v.loading = true;
        let (all, branches, file) = (v.all, v.branches.clone(), v.file.clone());
        let keep = v.selected.and_then(|i| v.commits.get(i)).map(|c| c.hash.clone());
        cx.spawn(async move |this, cx| {
            let (commits, rows, refs) = cx
                .background_executor()
                .spawn(async move {
                    let commits = vcs::graph_log(&root, 3000, all, &branches, file.as_deref());
                    let rows = vcs::layout_graph(&commits);
                    (commits, rows, vcs::ref_list(&root))
                })
                .await;
            let _ = this.update(cx, |ws, cx| {
                let Some(v) = ws.log_view.as_mut() else { return };
                let sel = keep.and_then(|h| commits.iter().position(|c| c.hash == h)).or((!commits.is_empty()).then_some(0));
                v.commits = commits;
                v.rows = rows;
                v.refs = refs;
                v.loading = false;
                v.selected = None;
                // the working tree stays selected across reloads (after a stage / commit)
                match sel.filter(|_| !v.working) {
                    Some(i) => ws.select_commit(i, cx),
                    None => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn reload_log_after_git(&mut self, cx: &mut Context<Self>) {
        if self.log_view.is_some() {
            self.reload_log(cx);
        }
    }

    fn select_commit(&mut self, i: usize, cx: &mut Context<Self>) {
        let (Some(root), Some(v)) = (self.git.root.clone(), self.log_view.as_mut()) else { return };
        let Some(c) = v.commits.get(i).cloned() else { return };
        if v.selected == Some(i) && !v.working {
            return;
        }
        v.working = false;
        v.selected = Some(i);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let h = c.hash.clone();
            let (details, files) = cx.background_executor().spawn(async move { (vcs::commit_details(&root, &h), vcs::commit_files(&root, &h, c.parents.first().map(String::as_str))) }).await;
            let _ = this.update(cx, |ws, cx| {
                if let Some(v) = ws.log_view.as_mut().filter(|v| v.selected == Some(i)) {
                    v.details = details;
                    v.files = files;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Selects commit `i` and scrolls to it (row 0 of the list is the working tree).
    pub(crate) fn log_goto(&mut self, i: usize, cx: &mut Context<Self>) {
        self.select_commit(i, cx);
        let vis = self.log_visible(cx);
        if let (Some(k), Some(v)) = (vis.iter().position(|&x| x == i), self.log_view.as_ref()) {
            v.scroll.scroll_to_item(k + 1, ScrollStrategy::Center);
        }
    }

    pub(crate) fn log_select_working(&mut self, cx: &mut Context<Self>) {
        self.git_ui(cx);
        self.refresh_git(cx);
        if let Some(v) = self.log_view.as_mut() {
            v.working = true;
            v.selected = None;
        }
        cx.notify();
    }

    /// Indices of the commits matching the filter.
    fn log_visible(&self, cx: &Context<Self>) -> Vec<usize> {
        let Some(v) = self.log_view.as_ref() else { return Vec::new() };
        let f = v.filter.text(cx).to_lowercase();
        if f.is_empty() {
            return (0..v.commits.len()).collect();
        }
        v.commits
            .iter()
            .enumerate()
            .filter(|(_, c)| c.subject.to_lowercase().contains(&f) || c.author.to_lowercase().contains(&f) || c.hash.starts_with(&f) || c.refs.iter().any(|r| r.to_lowercase().contains(&f)))
            .map(|(i, _)| i)
            .collect()
    }

    fn log_move(&mut self, d: isize, cx: &mut Context<Self>) {
        let vis = self.log_visible(cx);
        if vis.is_empty() {
            return;
        }
        // position 0 is the working tree, commits start at 1
        let working = self.log_view.as_ref().is_some_and(|v| v.working);
        let cur = if working { Some(0) } else { self.log_view.as_ref().and_then(|v| v.selected).and_then(|s| vis.iter().position(|&i| i == s)).map(|p| p + 1) };
        let k = (cur.unwrap_or(0) as isize + d).clamp(0, vis.len() as isize) as usize;
        if k == 0 {
            self.log_select_working(cx);
        } else {
            self.select_commit(vis[k - 1], cx);
        }
        if let Some(v) = self.log_view.as_ref() {
            v.scroll.scroll_to_item(k, ScrollStrategy::Center);
        }
    }

    fn open_commit_file(&mut self, f: CommitFile, cx: &mut Context<Self>) {
        let (Some(root), Some(c)) = (self.git.root.clone(), self.log_view.as_ref().and_then(|v| v.selected.and_then(|i| v.commits.get(i)).cloned())) else { return };
        cx.spawn(async move |this, cx| {
            let (fc, cc) = (f.clone(), c.clone());
            let (old, new) = cx
                .background_executor()
                .spawn(async move {
                    let old = match cc.parents.first() {
                        Some(p) => vcs::file_at(&root, p, fc.orig.as_ref().unwrap_or(&fc.path)).unwrap_or_default(),
                        None => String::new(),
                    };
                    (old, vcs::file_at(&root, &cc.hash, &fc.path).unwrap_or_default())
                })
                .await;
            let _ = this.update(cx, |ws, cx| {
                let left = c.parents.first().map(|p| p.chars().take(8).collect()).unwrap_or_else(|| "(trống)".into());
                ws.show_diff(f.path, DiffMode::Commit, left, c.short.clone(), old, new, None, cx);
            });
        })
        .detach();
    }

    fn log_menu(&mut self, i: usize, ev: &MouseDownEvent, cx: &mut Context<Self>) {
        self.select_commit(i, cx);
        let Some(c) = self.log_view.as_ref().and_then(|v| v.commits.get(i)).cloned() else { return };
        let remotes = self.git.tree.remotes.clone();
        let current = self.git.branch.clone().unwrap_or_default();
        let h = c.hash.clone();
        let refs: Vec<(String, RefKind)> = c.refs.iter().map(|r| ref_kind(r, &remotes)).collect();
        // merge / rebase onto a branch name when the commit has one (nicer merge message)
        let target = refs.iter().find(|(n, k)| matches!(k, RefKind::Local | RefKind::Remote) && *n != current).map(|(n, _)| n.clone()).unwrap_or_else(|| h.clone());
        let op = |o: LogOp| MenuAction::GitLogOp(o);
        let mut items = Vec::new();
        for (name, k) in &refs {
            match k {
                RefKind::Local | RefKind::Remote if *name != current => items.push(menu_item(format!("Checkout {name}"), "", op(LogOp::CheckoutBranch(name.clone())))),
                _ => {}
            }
        }
        if c.refs.iter().any(|r| r.starts_with("HEAD ->")) {
            items.push(menu_item("Undo commit này (giữ thay đổi, đã stage)", "", op(LogOp::UndoCommit)));
            items.push(menu_sep());
        }
        items.extend([
            menu_item("Checkout commit này (detached)", "", op(LogOp::CheckoutCommit(h.clone()))),
            menu_item("Tạo nhánh từ đây…", "", MenuAction::GitPrompt(PromptKind::BranchAt(h.clone()))),
            menu_item("Tạo tag…", "", MenuAction::GitPrompt(PromptKind::TagAt(h.clone()))),
            menu_sep(),
            menu_item("Cherry-pick vào nhánh hiện tại", "", op(LogOp::CherryPick(h.clone()))),
            menu_item("Revert commit (tạo commit đảo ngược)", "", op(LogOp::Revert(h.clone()))),
            menu_item(format!("Merge {} vào {current}", short_name(&target)), "", op(LogOp::Merge(target.clone()))),
            menu_item(format!("Rebase {current} lên {}", short_name(&target)), "", op(LogOp::Rebase(target))),
            menu_sep(),
            menu_item(format!("Reset {current} về đây - Soft (giữ thay đổi, đã stage)"), "", op(LogOp::Reset("soft", h.clone()))),
            menu_item(format!("Reset {current} về đây - Mixed (giữ thay đổi)"), "", op(LogOp::Reset("mixed", h.clone()))),
            menu_item(format!("Reset {current} về đây - Hard (BỎ thay đổi)…"), "", op(LogOp::Reset("hard", h.clone()))),
            menu_sep(),
        ]);
        for (name, k) in &refs {
            if *k == RefKind::Local {
                if *name != current {
                    items.push(menu_item(format!("Xóa nhánh {name}…"), "", op(LogOp::DeleteBranch(name.clone()))));
                }
                items.push(menu_item(format!("Đổi tên nhánh {name}…"), "", MenuAction::GitPrompt(PromptKind::RenameBranch(name.clone()))));
            }
        }
        items.push(menu_item("Copy hash", "", op(LogOp::CopyHash(h))));
        self.show_menu(ev.position, items, cx);
    }

    pub(crate) fn run_log_op(&mut self, op: LogOp, window: &mut Window, cx: &mut Context<Self>) {
        let conflicts = |what: &'static str| {
            move |ws: &mut Workspace, r: Result<String, String>, cx: &mut Context<Workspace>| match r {
                Ok(_) => ws.message = Some(format!("{what} xong")),
                Err(e) => {
                    ws.message = Some(format!("{what} dừng: {}", crate::git_support::first_line(&e)));
                    ws.open_git_if_conflicts(cx);
                }
            }
        };
        match op {
            LogOp::CopyHash(h) => cx.write_to_clipboard(ClipboardItem::new_string(h)),
            LogOp::UndoCommit => self.git_undo_last_commit(window, cx),
            LogOp::CheckoutBranch(b) => {
                if let Err(e) = self.git_checkout(&b, false, cx) {
                    self.message = Some(format!("Checkout lỗi: {e}"));
                }
            }
            LogOp::CheckoutCommit(h) => {
                if self.tabs.iter().any(|t| t.read(cx).is_dirty()) {
                    self.message = Some("Còn file chưa lưu - lưu (Ctrl+S) trước khi checkout".into());
                } else {
                    self.run_git("Checkout", move |r| vcs::checkout_detached(r, &h), |ws, r, cx| {
                        ws.message = Some(match r {
                            Ok(_) => "Đang ở commit (detached HEAD) - tạo nhánh nếu muốn commit tiếp".into(),
                            Err(e) => format!("Checkout lỗi: {}", crate::git_support::first_line(&e)),
                        });
                        ws.reload_clean_tabs(cx);
                    }, cx);
                }
            }
            LogOp::CherryPick(h) => {
                self.save_all_for_git(cx);
                self.run_git("Cherry-pick", move |r| vcs::cherry_pick(r, &h), conflicts("Cherry-pick"), cx);
            }
            LogOp::Revert(h) => {
                self.save_all_for_git(cx);
                self.run_git("Revert", move |r| vcs::revert_commit(r, &h), conflicts("Revert"), cx);
            }
            LogOp::Merge(t) => {
                self.save_all_for_git(cx);
                self.run_git("Merge", move |r| vcs::merge(r, &t), conflicts("Merge"), cx);
            }
            LogOp::Rebase(t) => {
                self.save_all_for_git(cx);
                self.run_git("Rebase", move |r| vcs::rebase(r, &t), conflicts("Rebase"), cx);
            }
            LogOp::Reset("hard", h) => {
                let short: String = h.chars().take(8).collect();
                self.open_confirm_pub(
                    format!("Reset --hard về {short}?"),
                    "Nhánh hiện tại trỏ về commit này và MỌI thay đổi chưa commit bị bỏ.\nCommit phía sau vẫn còn trong reflog (git reflog) nếu cần lấy lại.".into(),
                    ConfirmAction::GitResetHard(h),
                    window,
                    cx,
                );
            }
            LogOp::Reset(mode, h) => {
                self.save_all_for_git(cx);
                self.run_git("Reset", move |r| vcs::reset(r, mode, &h), |ws, r, cx| {
                    if let Err(e) = r {
                        ws.message = Some(format!("Reset lỗi: {}", crate::git_support::first_line(&e)));
                    }
                    ws.reload_clean_tabs(cx);
                }, cx);
            }
            LogOp::DeleteBranch(b) => {
                self.open_confirm_pub(format!("Xóa nhánh {b}?"), format!("Xóa nhánh local {b} (git branch -d).\nNhánh chưa merge sẽ hỏi lại trước khi xóa hẳn."), ConfirmAction::GitDeleteBranch(b, false), window, cx);
            }
        }
    }

    pub(crate) fn git_reset_hard(&mut self, h: String, cx: &mut Context<Self>) {
        self.run_git("Reset --hard", move |r| vcs::reset(r, "hard", &h), |ws, r, cx| {
            if let Err(e) = r {
                ws.message = Some(format!("Reset lỗi: {}", crate::git_support::first_line(&e)));
            }
            ws.reload_clean_tabs(cx);
        }, cx);
    }

    pub(crate) fn git_delete_branch(&mut self, b: String, force: bool, window: &mut Window, cx: &mut Context<Self>) {
        let _ = window;
        let name = b.clone();
        self.run_git("Xóa nhánh", move |r| vcs::delete_branch(r, &b, force), move |ws, r, _| match r {
            Ok(_) => ws.message = Some(format!("Đã xóa nhánh {name}")),
            Err(e) if e.contains("not fully merged") => {
                ws.message = Some(format!("Nhánh {name} chưa được merge - dùng lại menu và xác nhận xóa hẳn"));
                ws.pending_force_delete = Some(name);
            }
            Err(e) => ws.message = Some(format!("Xóa nhánh lỗi: {}", crate::git_support::first_line(&e))),
        }, cx);
    }

    pub(crate) fn render_log_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        // a branch that is not merged: ask again before -D
        if let Some(b) = self.pending_force_delete.take() {
            self.open_confirm_pub(format!("Nhánh {b} chưa merge - vẫn xóa?"), format!("Commit chỉ có trên {b} sẽ không còn nhánh nào giữ (vẫn lấy lại được qua git reflog)."), ConfirmAction::GitDeleteBranch(b, true), window, cx);
        }
        let v = self.log_view.as_mut()?;
        if std::mem::take(&mut v.needs_focus) {
            crate::dialogs::focus_later(v.focus.clone(), window, cx);
        }
        let vis = self.log_visible(cx);
        let v = self.log_view.as_ref()?;
        let filtered = vis.len() != v.commits.len();
        let n_vis = vis.len();
        let commits: Vec<GraphCommit> = vis.iter().map(|&i| v.commits[i].clone()).collect();
        // the graph only makes sense unfiltered
        let rows: Vec<Option<GraphRow>> = vis.iter().map(|&i| (!filtered).then(|| v.rows[i].clone())).collect();
        // width from the lanes the first screens use: one wide branchy stretch deep in history must not push everything right
        let lanes = if filtered { 0 } else { v.rows.iter().take(200).map(|r| r.lanes).max().unwrap_or(1).clamp(1, MAX_LANES) };
        let graph_w = lanes as f32 * LANE_W + 10.;
        let selected = v.selected;
        let remotes = self.git.tree.remotes.clone();
        let this = cx.entity().downgrade();
        // same-year dates drop the year: the newest commit's year stands in for "now"
        let this_year = commits.first().and_then(|c| c.date.get(0..4)).unwrap_or("").to_string();
        // the whole-repository window starts with a "Working Tree" row (staging area)
        let with_wt = v.file.is_none();
        let wt_selected = v.working;
        let n_changes = self.git.tree.changes.len();
        let wt_this = cx.entity().downgrade();
        let list = uniform_list("log-rows", commits.len() + with_wt as usize, move |range, _, _| {
            range
                .map(|k| {
                    if with_wt && k == 0 {
                        let t = wt_this.clone();
                        return div()
                            .id("log-working")
                            .w_full()
                            .h(px(ROW_H))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_3()
                            .px_3()
                            .when(wt_selected, |d| d.bg(c(theme::SELECTION)))
                            .hover(|s| s.bg(c(theme::ROW_HOVER)))
                            .child(div().w(px(graph_w.max(22.))).flex_none().flex().justify_center().child(div().size(px(9.)).rounded_full().bg(c(theme::WARNING))))
                            .child(
                                div()
                                    .flex_1()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(c(theme::FG))
                                    .child(SharedString::from(if n_changes == 0 { "Working Tree (sạch)".to_string() } else { format!("Working Tree/Index ({n_changes} thay đổi)") })),
                            )
                            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                let _ = t.update(cx, |ws, cx| ws.log_select_working(cx));
                            });
                    }
                    let k = k - with_wt as usize;
                    let cm = &commits[k];
                    let i = vis[k];
                    let row = rows[k].clone();
                    let (t1, t2) = (this.clone(), this.clone());
                    let chips: Vec<AnyElement> = cm
                        .refs
                        .iter()
                        .map(|r| {
                            let (name, kind) = ref_kind(r, &remotes);
                            let color = match kind {
                                RefKind::Head => theme::ACCENT,
                                RefKind::Local => theme::GIT_ADDED,
                                RefKind::Remote => theme::GIT_MODIFIED,
                                RefKind::Tag => theme::WARNING,
                            };
                            let mut bg = c(color);
                            bg.a = if kind == RefKind::Head { 1. } else { 0.16 };
                            div()
                                .flex_none()
                                .max_w(px(180.))
                                .truncate()
                                .px(px(7.))
                                .rounded_full()
                                .bg(bg)
                                .text_color(if kind == RefKind::Head { gpui::white() } else { c(color) })
                                .text_size(px(10.5))
                                .child(SharedString::from(name))
                                .into_any_element()
                        })
                        .collect();
                    div()
                        .id(("log-row", i))
                        .w_full()
                        .h(px(ROW_H))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .pr_3()
                        .overflow_hidden()
                        .when(selected == Some(i), |d| d.bg(c(theme::SELECTION)))
                        .hover(|s| s.bg(c(theme::ROW_HOVER)))
                        .when_some(row, |d, row| d.child(graph_cell(row, graph_w)))
                        .when(filtered, |d| d.pl_3())
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_1()
                                .overflow_hidden()
                                .child(div().flex_shrink().min_w_0().truncate().text_color(c(theme::FG)).child(SharedString::from(cm.subject.clone())))
                                .children(chips),
                        )
                        .child(
                            div()
                                .w(px(124.))
                                .flex_none()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .child(crate::git_style::avatar(&cm.author, 16.))
                                .child(div().min_w_0().truncate().text_color(c(theme::FG_DIM)).child(SharedString::from(cm.author.clone()))),
                        )
                        .child(div().min_w(px(98.)).flex_none().whitespace_nowrap().text_color(c(theme::FG_DIM)).child(SharedString::from(crate::git_style::short_date(&cm.date, &this_year))))
                        .child(div().min_w(px(66.)).flex_none().whitespace_nowrap().font_family("JetBrains Mono").text_size(px(11.)).text_color(c(theme::FG_DIM)).child(SharedString::from(cm.short.clone())))
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            let _ = t1.update(cx, |ws, cx| ws.select_commit(i, cx));
                        })
                        .on_mouse_down(MouseButton::Right, move |ev: &MouseDownEvent, _, cx| {
                            let ev = ev.clone();
                            let _ = t2.update(cx, |ws, cx| ws.log_menu(i, &ev, cx));
                        })
                })
                .collect()
        })
        .track_scroll(v.scroll.clone())
        .size_full();

        let root = self.git.root.clone().unwrap_or_default();
        // the commit's files: virtualized, a commit can touch thousands of them
        let file_items = std::sync::Arc::new(v.files.clone());
        let files = uniform_list("log-files-list", file_items.len(), cx.processor(move |_, range: std::ops::Range<usize>, _, cx| {
            range
                .map(|k| {
                    let f = &file_items[k];
                    let rel = f.path.strip_prefix(&root).unwrap_or(&f.path);
                    let name = rel.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    let dir = match &f.orig {
                        Some(o) => format!("← {}", o.strip_prefix(&root).unwrap_or(o).display()),
                        None => rel.parent().map(|d| d.display().to_string()).unwrap_or_default(),
                    };
                    let color = match f.status {
                        'A' => theme::GIT_ADDED,
                        'D' => theme::ERROR,
                        _ => theme::GIT_MODIFIED,
                    };
                    let f2 = f.clone();
                    div()
                        .id(("log-file", k))
                        .h(px(26.))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .rounded_md()
                        .hover(|s| s.bg(c(theme::ROW_HOVER)))
                        .child(crate::git_style::status_badge(f.status, color))
                        .child(div().flex_none().max_w(px(220.)).truncate().text_color(c(theme::FG)).child(SharedString::from(name)))
                        .child(div().flex_1().min_w_0().truncate().text_size(px(11.)).text_color(c(theme::FG_DIM)).child(SharedString::from(dir)))
                        .on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, cx| ws.open_commit_file(f2.clone(), cx)))
                })
                .collect::<Vec<_>>()
        }));
        // details: "<hash>\nTác giả: ...\nCommitter: ...\n\n<message>": the header comes from the commit itself
        let body: String = v.details.split_once("\n\n").map(|(_, b)| b.trim().to_string()).unwrap_or_default();
        let mut body_lines = body.lines();
        let _ = body_lines.next(); // the subject, shown as the title
        let body_rest: Vec<AnyElement> = body_lines.map(|l| div().min_h(px(16.)).text_color(c(theme::FG_DIM)).child(SharedString::from(l.to_string())).into_any_element()).collect();
        let sel_commit = selected.and_then(|i| v.commits.get(i)).cloned();
        let n_files = v.files.len();
        let header_card = match sel_commit {
            Some(cm) => {
                let hash = cm.hash.clone();
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .child(div().text_size(px(15.)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(theme::FG)).child(SharedString::from(cm.subject.clone())))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(crate::git_style::avatar(&cm.author, 22.))
                            .child(div().flex_1().min_w_0().truncate().text_color(c(theme::FG)).child(SharedString::from(cm.author.clone())))
                            .child(div().flex_none().text_color(c(theme::FG_DIM)).child(SharedString::from(cm.date.clone()))),
                    )
                    .child(div().flex().flex_row().child(
                        div()
                            .id("log-hash")
                            .flex_none()
                            .px_2()
                            .rounded_full()
                            .bg(c(theme::ROW_HOVER))
                            .font_family("JetBrains Mono")
                            .text_size(px(11.))
                            .text_color(c(theme::FG_DIM))
                            .hover(|s| s.text_color(c(theme::FG)))
                            .tooltip(crate::ui_icons::tip("Copy hash"))
                            .child(SharedString::from(cm.short.clone()))
                            .on_mouse_down(MouseButton::Left, move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(hash.clone()))),
                    ))
                    .when(!body_rest.is_empty(), |d| d.child(div().id("log-body").max_h(px(160.)).overflow_y_scroll().children(body_rest)))
                    .into_any_element()
            }
            None => div().p_3().text_color(c(theme::FG_DIM)).child("Chọn một commit").into_any_element(),
        };
        let side = div()
            .w(px(420.))
            .flex_none()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(c(theme::BORDER))
            .child(header_card)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .pt_2()
                    .pb_1()
                    .border_t_1()
                    .border_color(c(theme::BORDER))
                    .child(div().text_size(px(11.)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(theme::FG_DIM)).child("FILE THAY ĐỔI"))
                    .child(crate::git_style::count_pill(n_files)),
            )
            .child(div().id("log-files").flex_1().min_h_0().px_1().child(files.size_full()));

        let title = match &v.file {
            Some(f) => format!("History · {}", crate::tree::file_name(f)),
            None => "Git Log".to_string(),
        };
        let count = if v.loading { "đang tải…".to_string() } else if filtered { format!("{n_vis}/{}", v.commits.len()) } else { v.commits.len().to_string() };
        let all = v.all;
        let picker = v.picker_open.then(|| self.render_branch_picker(cx));
        let v = self.log_view.as_ref()?;
        let has_file = v.file.is_some();
        let bar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .px_3()
            .h(px(46.))
            .border_b_1()
            .border_color(c(theme::BORDER))
            .child(crate::ui_icons::icon(crate::ui_icons::Ico::History, 16., c(theme::FG_DIM)))
            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(theme::FG)).child(SharedString::from(title)))
            .child(div().flex_none().px(px(8.)).rounded_full().bg(c(theme::ROW_HOVER)).text_size(px(11.)).text_color(c(theme::FG_DIM)).child(SharedString::from(count)))
            .child(
                div()
                    .w(px(340.))
                    .h(px(28.))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .rounded_full()
                    .border_1()
                    .border_color(c(theme::BORDER))
                    .child(crate::ui_icons::icon(crate::ui_icons::Ico::Search, 13., c(theme::FG_DIM)))
                    .child(div().flex_1().min_w_0().child(v.filter.input.clone())),
            )
            .child(div().flex_1())
            .when(!has_file, |d| {
                let label = if all {
                    "Tất cả nhánh".to_string()
                } else {
                    match v.branches.as_slice() {
                        [] => "Nhánh hiện tại".to_string(),
                        [one] => one.clone(),
                        many => format!("{} nhánh", many.len()),
                    }
                };
                d.child(
                    div()
                        .id("log-branches")
                        .h(px(28.))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .rounded_full()
                        .border_1()
                        .border_color(c(if v.picker_open { theme::ACCENT } else { theme::BORDER }))
                        .hover(|s| s.bg(c(theme::ROW_HOVER)))
                        .child(crate::ui_icons::icon(crate::ui_icons::Ico::Branch, 13., c(theme::FG_DIM)))
                        .child(div().max_w(px(200.)).truncate().text_size(px(12.)).text_color(c(theme::FG)).child(SharedString::from(label)))
                        .child(crate::ui_icons::icon(crate::ui_icons::Ico::ChevronDown, 11., c(theme::FG_DIM)))
                        .on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.open_branch_picker(cx))),
                )
            })
            .when(has_file, |d| {
                d.child(crate::git_style::icon_btn("log-refresh", crate::ui_icons::Ico::Refresh, "Tải lại").on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.reload_log(cx))))
                    .child(crate::git_style::icon_btn("log-close", crate::ui_icons::Ico::Close, "Đóng (Esc)").on_mouse_down(MouseButton::Left, cx.listener(|ws, _, window, cx| ws.close_log(window, cx))))
            });
        let focus = v.focus.clone();
        let full = !has_file;
        let working = v.working;
        let toolbar = full.then(|| self.render_git_toolbar(cx));
        let banner = if full { self.render_git_state_banner(cx) } else { None };
        let sidebar = full.then(|| self.render_git_sidebar(cx));
        let wt_pane = (full && working).then(|| self.render_working_pane(cx));
        Some(
            div()
                .occlude()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .p(px(16.))
                .bg(gpui::rgba(0x00000066))
                .child(
                    div()
                        .key_context("GitLog")
                        .track_focus(&focus)
                        .size_full()
                        .flex()
                        .flex_col()
                        // a solid sheet: the transparent BG would show the editor through
                        .bg(c(theme::POPUP_BG))
                        .border_1()
                        .border_color(c(theme::BORDER))
                        .rounded(px(14.))
                        .overflow_hidden()
                        .shadow_lg()
                        .on_action(cx.listener(|ws, _: &crate::actions::GitUp, _, cx| ws.log_move(-1, cx)))
                        .on_action(cx.listener(|ws, _: &crate::actions::GitDown, _, cx| ws.log_move(1, cx)))
                        .on_action(cx.listener(|ws, _: &crate::actions::CloseDiff, window, cx| ws.close_log(window, cx)))
                        .relative()
                        .children(toolbar)
                        .children(banner)
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .flex_1()
                                .min_h_0()
                                .children(sidebar)
                                .child(div().flex_1().min_w_0().flex().flex_col().child(bar).child(div().flex_1().min_h_0().child(list)))
                                .child(match wt_pane {
                                    Some(p) => p,
                                    None => side.into_any_element(),
                                }),
                        )
                        .children(picker),
                )
                .into_any_element(),
        )
    }

    /// Opens the branch picker (a click anywhere else closes it).
    fn open_branch_picker(&mut self, cx: &mut Context<Self>) {
        let root = self.git.root.clone();
        let Some(v) = self.log_view.as_mut() else { return };
        v.picker_open = true;
        v.branch_list = root.as_deref().map(vcs::branches).unwrap_or_default();
        cx.notify()
    }

    /// `None` = every branch, `Some([])` = the current one, else that branch toggled.
    pub(crate) fn pick_branches(&mut self, pick: Option<Option<String>>, cx: &mut Context<Self>) {
        let Some(v) = self.log_view.as_mut() else { return };
        match pick {
            None => {
                v.all = true;
                v.branches.clear();
            }
            Some(None) => {
                v.all = false;
                v.branches.clear();
            }
            Some(Some(b)) => {
                if v.all {
                    v.all = false;
                    v.branches.clear();
                }
                if let Some(i) = v.branches.iter().position(|x| *x == b) {
                    v.branches.remove(i);
                } else {
                    v.branches.push(b);
                }
                // nothing left ticked: back to every branch
                if v.branches.is_empty() {
                    v.all = true;
                }
            }
        }
        self.reload_log(cx);
    }

    /// The dropdown under the branch button: every branch with a checkbox, local first.
    fn render_branch_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        let v = self.log_view.as_ref().unwrap();
        let remotes = &self.git.tree.remotes;
        let is_remote = |b: &str| remotes.iter().any(|r| b.starts_with(&format!("{r}/")));
        let mut items: Vec<PickItem> = vec![PickItem::All, PickItem::Current];
        for (title, remote) in [("LOCAL", false), ("REMOTE", true)] {
            let list: Vec<&(String, bool)> = v.branch_list.iter().filter(|(b, _)| is_remote(b) == remote).collect();
            if list.is_empty() {
                continue;
            }
            items.push(PickItem::Heading(title));
            items.extend(list.into_iter().map(|(b, cur)| PickItem::Branch(b.clone(), *cur, remote)));
        }
        let n = items.len();
        let items = std::sync::Arc::new(items);
        // virtualized: hundreds of remote branches, only the rows in view are built
        let list = uniform_list("lb-list-uni", n, cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
            let (all, picked, only_current) = this.log_view.as_ref().map(|v| (v.all, v.branches.clone(), !v.all && v.branches.is_empty())).unwrap_or_default();
            let row = |id: SharedString, checked: bool, label: String, dim: bool| {
                div()
                    .id(id)
                    .h(px(PICK_ROW_H))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .rounded_md()
                    .hover(|s| s.bg(c(theme::ROW_HOVER)))
                    .child(crate::git_style::check_box(checked, false))
                    .child(div().flex_1().min_w_0().truncate().text_color(c(if dim { theme::FG_DIM } else { theme::FG })).child(SharedString::from(label)))
            };
            range
                .map(|i| match &items[i] {
                    PickItem::All => row("lb-all".into(), all, "Tất cả nhánh".into(), false).on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.pick_branches(None, cx))).into_any_element(),
                    PickItem::Current => row("lb-cur".into(), only_current, "Nhánh hiện tại (HEAD)".into(), false)
                        .on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.pick_branches(Some(None), cx)))
                        .into_any_element(),
                    PickItem::Heading(t) => div().h(px(PICK_ROW_H)).px_2().pt_2().text_size(px(10.5)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(theme::FG_DIM)).child(*t).into_any_element(),
                    PickItem::Branch(b, current, remote) => {
                        let name = b.clone();
                        let label = if *current { format!("{b}  ·  hiện tại") } else { b.clone() };
                        row(SharedString::from(format!("lb-b-{i}")), !all && picked.contains(b), label, *remote)
                            .on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, cx| ws.pick_branches(Some(Some(name.clone())), cx)))
                            .into_any_element()
                    }
                })
                .collect::<Vec<_>>()
        }));
        div()
            .absolute()
            .top(px(44.))
            .right(px(80.))
            .w(px(320.))
            .occlude()
            .bg(c(theme::POPUP_BG))
            .border_1()
            .border_color(c(theme::BORDER))
            .rounded(px(12.))
            .shadow_lg()
            .p_1()
            .on_mouse_down_out(cx.listener(|ws, _, _, cx| {
                if let Some(v) = ws.log_view.as_mut() {
                    v.picker_open = false;
                }
                cx.notify()
            }))
            .child(div().id("lb-list").h(px((n as f32 * PICK_ROW_H).min(420.))).child(list.size_full()))
            .into_any_element()
    }

    pub(crate) fn close_log(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.log_view = None;
        if let Some(i) = self.active {
            self.activate(i, window, cx);
        }
        cx.notify();
    }
}

fn short_name(t: &str) -> String {
    if t.len() == 40 && t.chars().all(|c| c.is_ascii_hexdigit()) { t[..8].to_string() } else { t.to_string() }
}

fn lane_color(i: usize) -> Hsla {
    c(LANE_COLORS[i % LANE_COLORS.len()])
}

/// The graph column of one row: lane segments and the commit's dot.
fn graph_cell(row: GraphRow, width: f32) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<gpui::Pixels>, _, window, _| {
            let x = |l: usize| bounds.origin.x + px(6. + LANE_W * l.min(MAX_LANES - 1) as f32);
            let y = |k: u8| bounds.origin.y + bounds.size.height * (k as f32 / 2.0);
            for e in &row.edges {
                let mut pb = PathBuilder::stroke(px(1.5));
                pb.move_to(point(x(e.from.0), y(e.from.1)));
                pb.line_to(point(x(e.to.0), y(e.to.1)));
                if let Ok(path) = pb.build() {
                    window.paint_path(path, lane_color(e.lane));
                }
            }
            let color = row.edges.iter().find(|e| e.from == (row.col, 1) || e.to == (row.col, 1)).map(|e| lane_color(e.lane)).unwrap_or(lane_color(0));
            let r = px(4.);
            window.paint_quad(fill(Bounds::new(point(x(row.col) - r, y(1) - r), size(r * 2., r * 2.)), color).corner_radii(r));
        },
    )
    .w(px(width))
    .h_full()
    .flex_none()
}
