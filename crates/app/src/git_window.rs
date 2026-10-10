//! The Git window (SmartGit layout): a toolbar of everyday commands, the branch list on the
//! left (local with ahead/behind, remotes, tags, stashes), the graph in the middle (its top
//! row is the working tree) and, on the right, either the selected commit or the staging
//! area with the commit box. Opened by the Git button / shortcut; everything is done here.
use crate::context_menu::{MenuAction, PromptKind, menu_item, menu_sep};
use crate::git_log::LogOp;
use crate::theme::{self, c};
use crate::ui_icons::{Ico, icon, tip};
use crate::workspace::Workspace;
use gpui::{AnyElement, Context, MouseButton, MouseDownEvent, SharedString, Stateful, div, prelude::*, px};

pub(crate) const SIDEBAR_W: f32 = 250.;
/// Height of every row of the branch list (uniform_list measures one row for all).
const SIDE_ROW_H: f32 = 26.;

/// A row of the branch list before it is built; the data stays in `LogView.refs`.
#[derive(Clone)]
enum SideItem {
    Head,
    /// key, title, count, open
    Heading(String, String, usize, bool),
    Local(usize),
    /// remote index, branch index
    Remote(usize, usize),
    Tag(usize),
    Stash(usize, String),
}

/// A toolbar button: icon over a small label (SmartGit).
fn tool(id: &'static str, ico: Ico, label: &'static str, hint: &'static str) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .flex_none()
        .min_w(px(54.))
        .px_2()
        .py_1()
        .flex()
        .flex_col()
        .items_center()
        .gap_0p5()
        .rounded_md()
        .hover(|s| s.bg(c(theme::ROW_HOVER)))
        .tooltip(tip(hint))
        .child(icon(ico, 18., c(theme::FG)))
        .child(div().text_size(px(11.)).whitespace_nowrap().text_color(c(theme::FG_DIM)).child(label))
}

fn tool_sep() -> gpui::Div {
    div().flex_none().w(px(1.)).h(px(32.)).mx_1().bg(c(theme::BORDER))
}

/// "+2" / "-5" pill next to a branch (ahead / behind its upstream).
fn count_chip(text: String, color: u32) -> gpui::Div {
    let mut bg = c(color);
    bg.a = 0.18;
    div().flex_none().px(px(6.)).rounded_full().bg(bg).text_size(px(10.5)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(color)).child(SharedString::from(text))
}

/// Does a commit's decoration ("HEAD -> main", "origin/main", "tag: v1") name `target`?
pub(crate) fn ref_names(r: &str, target: &str) -> bool {
    let r = r.strip_prefix("HEAD -> ").unwrap_or(r);
    let r = r.strip_prefix("tag: ").unwrap_or(r);
    r == target
}

impl Workspace {
    pub(crate) fn render_git_toolbar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.git.busy.clone();
        let (ahead, behind) = self.git.tree.ahead_behind.unwrap_or((0, 0));
        let has_stash = !self.git.tree.stashes.is_empty();
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_0p5()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(c(theme::BORDER))
            .child(tool("gw-pull", Ico::Pull, "Pull", "Pull (kéo và rebase/merge, giữ thay đổi chưa commit)").on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.git_pull(cx))))
            .child(tool("gw-fetch", Ico::Refresh, "Fetch", "Fetch mọi remote").on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.git_fetch(cx))))
            .child(
                tool("gw-push", Ico::Push, if ahead > 0 { "Push ↑" } else { "Push" }, "Push nhánh hiện tại")
                    .when(ahead > 0, |d| d.child(div().absolute().top(px(2.)).right(px(4.)).child(count_chip(ahead.to_string(), theme::GIT_ADDED))))
                    .relative()
                    .on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.git_push(cx))),
            )
            .when(behind > 0, |d| d.child(div().text_size(px(11.)).text_color(c(theme::GIT_MODIFIED)).child(SharedString::from(format!("↓{behind}")))))
            .child(tool_sep())
            .child(tool("gw-stage", Ico::Plus, "Stage", "Stage tất cả thay đổi").on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| {
                let paths: Vec<std::path::PathBuf> = ws.git.tree.changes.iter().filter(|c| c.unstaged() || c.untracked()).flat_map(|c| c.paths()).collect();
                if !paths.is_empty() {
                    ws.run_git_simple("Stage", move |r| vcs::stage(r, &paths), cx)
                }
            })))
            .child(tool("gw-unstage", Ico::Minus, "Unstage", "Unstage tất cả").on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| {
                let paths: Vec<std::path::PathBuf> = ws.git.tree.changes.iter().filter(|c| c.staged()).flat_map(|c| c.paths()).collect();
                if !paths.is_empty() {
                    ws.run_git_simple("Unstage", move |r| vcs::unstage(r, &paths), cx)
                }
            })))
            .child(tool("gw-discard", Ico::Close, "Discard", "Bỏ thay đổi của file đang chọn (hỏi trước)").on_mouse_down(MouseButton::Left, cx.listener(|ws, _, window, cx| ws.git_discard_sel(window, cx))))
            .child(tool_sep())
            .child(tool("gw-stash", Ico::Stash, "Stash", "Cất thay đổi vào stash").on_mouse_down(MouseButton::Left, cx.listener(|ws, ev: &MouseDownEvent, _, cx| {
                let items = vec![
                    menu_item("Stash file đã theo dõi…", "", MenuAction::GitPrompt(PromptKind::Stash(false))),
                    menu_item("Stash cả file chưa theo dõi…", "", MenuAction::GitPrompt(PromptKind::Stash(true))),
                ];
                ws.show_menu(ev.position, items, cx)
            })))
            .child(
                tool("gw-apply", Ico::Check, "Apply", "Pop stash mới nhất (apply rồi xóa)")
                    .when(!has_stash, |d| d.opacity(0.4))
                    .on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| {
                        if ws.git.tree.stashes.is_empty() {
                            ws.message = Some("Không có stash nào".into());
                            return cx.notify();
                        }
                        ws.run_git("Stash pop", |r| vcs::stash_op(r, "pop", 0), |ws, r, cx| {
                            if let Err(e) = r {
                                ws.message = Some(format!("Stash pop lỗi: {}", crate::git_support::first_line(&e)));
                                ws.open_git_if_conflicts(cx);
                            }
                            ws.reload_clean_tabs(cx);
                        }, cx)
                    })),
            )
            .child(tool("gw-undo", Ico::Undo, "Undo", "Undo commit cuối (giữ thay đổi, đã stage)").on_mouse_down(MouseButton::Left, cx.listener(|ws, _, window, cx| ws.git_undo_last_commit(window, cx))))
            .child(tool_sep())
            .child(tool("gw-branch", Ico::Branch, "Branch", "Tạo nhánh mới từ HEAD").on_mouse_down(MouseButton::Left, cx.listener(|ws, _, window, cx| ws.open_name_prompt(PromptKind::NewBranch, window, cx))))
            .child(tool("gw-merge", Ico::Tree, "Merge", "Merge một nhánh vào nhánh hiện tại").on_mouse_down(MouseButton::Left, cx.listener(|ws, _, window, cx| ws.open_name_prompt(PromptKind::MergeBranch, window, cx))))
            .child(tool("gw-rebase", Ico::Commit, "Rebase", "Rebase nhánh hiện tại lên nhánh khác").on_mouse_down(MouseButton::Left, cx.listener(|ws, _, window, cx| ws.open_name_prompt(PromptKind::RebaseBranch, window, cx))))
            .child(div().flex_1())
            .when_some(busy, |d, b| d.child(div().mr_2().text_color(c(theme::ACCENT)).child(SharedString::from(format!("⏳ {b}…")))))
            .child(tool("gw-accounts", Ico::Gear, "Tài khoản", "Tài khoản git (token https)").on_mouse_down(MouseButton::Left, cx.listener(|ws, _, window, cx| ws.open_git_accounts(window, cx))))
            .child(tool("gw-reload", Ico::History, "Reload", "Tải lại đồ thị và nhánh").on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| {
                ws.refresh_git(cx);
                ws.reload_log(cx)
            })))
            .child(tool("gw-close", Ico::Close, "Đóng", "Đóng cửa sổ Git (Esc)").on_mouse_down(MouseButton::Left, cx.listener(|ws, _, window, cx| ws.close_log(window, cx))))
            .into_any_element()
    }

    /// "ĐANG REBASE / MERGE…" with Continue / Skip / Abort, only while an operation is in progress.
    pub(crate) fn render_git_state_banner(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let state = self.git.tree.state.filter(|s| *s != vcs::RepoState::Clean)?;
        let btn = |id: &'static str, label: &'static str| div().id(id).px_3().h(px(24.)).flex().items_center().rounded_full().border_1().border_color(c(theme::BORDER)).text_size(px(12.)).text_color(c(theme::FG)).hover(|s| s.bg(c(theme::ROW_HOVER))).child(label);
        Some(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .px_3()
                .py_1p5()
                .bg(c(theme::WARNING).opacity(0.16))
                .border_b_1()
                .border_color(c(theme::BORDER))
                .child(div().flex().flex_row().items_center().gap_2().child(div().px_2().rounded_sm().bg(c(theme::WARNING)).text_color(gpui::black()).font_weight(gpui::FontWeight::SEMIBOLD).child(state.label()))
                .child(btn("gw-continue", if state == vcs::RepoState::Conflicted { "✓ Xong - bỏ stash tự động" } else { "▶ Continue" }).on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, cx| {
                    ws.run_git("Continue", move |r| vcs::state_action(r, state, "continue"), |ws, r, cx| {
                        if let Err(e) = r {
                            ws.message = Some(format!("Continue lỗi: {}", crate::git_support::first_line(&e)));
                            ws.open_git_if_conflicts(cx);
                        }
                    }, cx)
                })))
                .when(state == vcs::RepoState::Rebasing, |d| {
                    d.child(btn("gw-skip", "⤼ Skip").on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, cx| ws.run_git_simple("Skip", move |r| vcs::state_action(r, state, "skip"), cx))))
                })
                .child(btn("gw-abort", if state == vcs::RepoState::Conflicted { "✕ Abort: bỏ phần xung đột" } else { "✕ Abort" }).on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, cx| ws.run_git_simple("Abort", move |r| vcs::state_action(r, state, "abort"), cx))))
                                )
                .child(div().text_size(px(11.5)).text_color(c(theme::FG_DIM)).child(if state == vcs::RepoState::Conflicted { "Pull xong nhưng thay đổi chưa commit của bạn bị xung đột: giải quyết file bên phải rồi bấm Xong; Abort giữ chúng trong stash" } else { "Giải quyết xung đột ở danh sách file bên phải (Working Tree), rồi Continue" }))
                .into_any_element(),
        )
    }

    /// Branches / Remotes / Tags / Stashes. A click selects the tip commit in the graph,
    /// double-click checks a local branch out, right-click opens its menu. The ticks choose
    /// which branches the graph shows (none ticked = all). Virtualized: with hundreds of remote
    /// branches only the rows in view are built.
    pub(crate) fn render_git_sidebar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(v) = self.log_view.as_ref() else { return div().into_any_element() };
        let (refs, flipped, all) = (&v.refs, &v.flipped, v.all);
        let mut items: Vec<SideItem> = vec![SideItem::Head];
        let open = !flipped.contains("local");
        items.push(SideItem::Heading("local".into(), "Local Branches".into(), refs.locals.len(), open));
        if open {
            items.extend((0..refs.locals.len()).map(SideItem::Local));
        }
        for (ri, (remote, branches)) in refs.remotes.iter().enumerate() {
            // remotes, tags and stashes start collapsed: `flipped` holds the ones the user opened
            let key = format!("remote-{remote}");
            let open = flipped.contains(&key);
            items.push(SideItem::Heading(key, remote.clone(), branches.len(), open));
            if open {
                items.extend((0..branches.len()).map(|i| SideItem::Remote(ri, i)));
            }
        }
        let open = flipped.contains("tags");
        items.push(SideItem::Heading("tags".into(), "Tags".into(), refs.tags.len(), open));
        if open {
            items.extend((0..refs.tags.len()).map(SideItem::Tag));
        }
        let stashes = &self.git.tree.stashes;
        let open = flipped.contains("stashes");
        items.push(SideItem::Heading("stashes".into(), "Stashes".into(), stashes.len(), open));
        if open {
            items.extend(stashes.iter().map(|s| SideItem::Stash(s.index, s.message.clone())));
        }
        let items = std::sync::Arc::new(items);
        let n = items.len();
        let list = gpui::uniform_list("gw-sidebar", n, cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
            range.map(|i| this.render_side_item(&items[i], cx)).collect::<Vec<_>>()
        }))
        .size_full();

        div()
            .w(px(SIDEBAR_W))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(c(theme::BORDER))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .h(px(34.))
                    .px_3()
                    .border_b_1()
                    .border_color(c(theme::BORDER))
                    .child(icon(Ico::Branch, 14., c(theme::ACCENT)))
                    .child(div().font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(theme::FG)).child("Branches"))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(11.)).text_color(c(theme::FG_DIM)).child(SharedString::from(crate::tree::file_name(&self.root))))
                    .when(!all, |d| {
                        d.child(
                            div()
                                .id("gw-untick")
                                .px_2()
                                .rounded_full()
                                .text_size(px(11.))
                                .text_color(c(theme::ACCENT))
                                .hover(|s| s.bg(c(theme::ROW_HOVER)))
                                .tooltip(tip("Bỏ chọn: hiện lại mọi nhánh"))
                                .child("Tất cả")
                                .on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.pick_branches(None, cx))),
                        )
                    }),
            )
            .child(div().flex_1().min_h_0().py_1().child(list))
            .into_any_element()
    }

    /// One row of the branch list, built when it scrolls into view.
    fn render_side_item(&mut self, item: &SideItem, cx: &mut Context<Self>) -> AnyElement {
        let Some(v) = self.log_view.as_ref() else { return div().into_any_element() };
        let (all, picked) = (v.all, v.branches.clone());
        let current = self.git.branch.clone().unwrap_or_default();
        let leaf = |id: SharedString, indent: f32| div().id(id).flex().flex_row().items_center().gap_1p5().h(px(SIDE_ROW_H)).pl(px(indent)).pr_2().mx_1().rounded_md().whitespace_nowrap().hover(|s| s.bg(c(theme::ROW_HOVER)));
        // tick box: nothing ticked = every branch in the graph; ticked = the current branch + those
        let tick = |id: SharedString, name: String, cx: &mut Context<Self>| {
            let checked = !all && picked.contains(&name);
            div()
                .id(id)
                .flex_none()
                .p(px(2.))
                .rounded_sm()
                .hover(|s| s.bg(c(theme::ROW_ACTIVE)))
                .tooltip(tip("Chỉ hiện nhánh này (cùng nhánh hiện tại) trong đồ thị"))
                .child(crate::git_style::check_box(checked, false))
                .on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, cx| {
                    cx.stop_propagation();
                    ws.pick_branches(Some(Some(name.clone())), cx)
                }))
        };
        match item {
            SideItem::Head => {
                let head_short = vcs::head_hash(self.git.root.as_deref().unwrap_or(std::path::Path::new("."))).map(|h| h.chars().take(8).collect::<String>()).unwrap_or_default();
                leaf("gw-head".into(), 10.)
                    .child(icon(Ico::Commit, 12., c(theme::ACCENT)))
                    .child(div().text_color(c(theme::FG)).child("HEAD"))
                    .child(div().text_size(px(11.)).text_color(c(theme::FG_DIM)).child(SharedString::from(format!("({head_short})"))))
                    .on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.log_select_ref("HEAD".into(), cx)))
                    .into_any_element()
            }
            SideItem::Heading(key, title, n, open) => {
                let key = key.clone();
                leaf(SharedString::from(format!("gw-h-{key}")), 8.)
                    .child(div().w(px(12.)).flex_none().child(icon(if *open { Ico::ChevronDown } else { Ico::ChevronRight }, 11., c(theme::FG_DIM))))
                    .child(div().font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(theme::FG)).child(SharedString::from(title.clone())))
                    .child(div().text_size(px(11.)).text_color(c(theme::FG_DIM)).child(SharedString::from(format!("({n})"))))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, cx| {
                        if let Some(v) = ws.log_view.as_mut() {
                            if !v.flipped.remove(&key) {
                                v.flipped.insert(key.clone());
                            }
                        }
                        cx.notify()
                    }))
                    .into_any_element()
            }
            SideItem::Local(i) => {
                let Some(b) = v.refs.locals.get(*i).cloned() else { return div().into_any_element() };
                let (name, name2) = (b.name.clone(), b.name.clone());
                let is_cur = b.name == current;
                leaf(SharedString::from(format!("gw-local-{i}")), 22.)
                    .when(is_cur, |d| d.bg(c(theme::ROW_ACTIVE)))
                    .child(tick(SharedString::from(format!("gw-tick-l{i}")), b.name.clone(), cx))
                    .child(div().flex_1().min_w_0().truncate().when(is_cur, |d| d.font_weight(gpui::FontWeight::SEMIBOLD)).text_color(c(theme::FG)).child(SharedString::from(b.name.clone())))
                    .when(b.gone, |d| d.child(count_chip("gone".into(), theme::ERROR)))
                    .when(b.ahead > 0, |d| d.child(count_chip(format!("+{}", b.ahead), theme::GIT_ADDED)))
                    .when(b.behind > 0, |d| d.child(count_chip(format!("-{}", b.behind), theme::GIT_MODIFIED)))
                    .when(b.upstream.is_none() && !b.gone, |d| d.child(div().text_size(px(10.)).text_color(c(theme::FG_DIM)).child("local")))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |ws, ev: &MouseDownEvent, window, cx| {
                        if ev.click_count >= 2 {
                            ws.run_log_op(LogOp::CheckoutBranch(name.clone()), window, cx);
                        } else {
                            ws.log_select_ref(name.clone(), cx);
                        }
                    }))
                    .on_mouse_down(MouseButton::Right, cx.listener(move |ws, ev: &MouseDownEvent, _, cx| {
                        let cur = name2 == ws.git.branch.clone().unwrap_or_default();
                        ws.branch_menu(ev, name2.clone(), true, cur, cx)
                    }))
                    .into_any_element()
            }
            SideItem::Remote(ri, i) => {
                let Some((remote, branches)) = v.refs.remotes.get(*ri) else { return div().into_any_element() };
                let Some(b) = branches.get(*i) else { return div().into_any_element() };
                let full = format!("{remote}/{b}");
                let (f1, f2) = (full.clone(), full.clone());
                let sel = full.clone();
                leaf(SharedString::from(format!("gw-remote-{ri}-{i}")), 22.)
                    .debug_selector(move || format!("side-remote:{sel}"))
                    .child(tick(SharedString::from(format!("gw-tick-r{ri}-{i}")), full, cx))
                    .child(div().flex_1().min_w_0().truncate().text_color(c(theme::FG)).child(SharedString::from(b.clone())))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |ws, ev: &MouseDownEvent, window, cx| {
                        if ev.click_count >= 2 {
                            ws.run_log_op(LogOp::CheckoutBranch(f1.clone()), window, cx);
                        } else {
                            ws.log_select_ref(f1.clone(), cx);
                        }
                    }))
                    .on_mouse_down(MouseButton::Right, cx.listener(move |ws, ev: &MouseDownEvent, _, cx| ws.branch_menu(ev, f2.clone(), false, false, cx)))
                    .into_any_element()
            }
            SideItem::Tag(i) => {
                let Some(t) = v.refs.tags.get(*i).cloned() else { return div().into_any_element() };
                let t1 = t.clone();
                leaf(SharedString::from(format!("gw-tag-{i}")), 22.)
                    .child(icon(Ico::Locate, 11., c(theme::WARNING)))
                    .child(div().flex_1().min_w_0().truncate().text_color(c(theme::FG)).child(SharedString::from(t)))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, cx| ws.log_select_ref(t1.clone(), cx)))
                    .into_any_element()
            }
            SideItem::Stash(i, message) => {
                let i = *i;
                leaf(SharedString::from(format!("gw-stash-{i}")), 22.)
                    .child(icon(Ico::Stash, 11., c(theme::FG_DIM)))
                    .child(div().flex_1().min_w_0().truncate().text_color(c(theme::FG)).child(SharedString::from(message.clone())))
                    .on_mouse_down(MouseButton::Right, cx.listener(move |ws, ev: &MouseDownEvent, _, cx| {
                        let op = |o: &'static str, label: &'static str| menu_item(label, "", MenuAction::GitStash(o, i));
                        ws.show_menu(ev.position, vec![op("apply", "Apply (giữ stash)"), op("pop", "Pop (apply rồi xóa)"), menu_sep(), op("drop", "Drop (xóa stash)")], cx)
                    }))
                    .into_any_element()
            }
        }
    }

    fn branch_menu(&mut self, ev: &MouseDownEvent, name: String, local: bool, is_current: bool, cx: &mut Context<Self>) {
        let current = self.git.branch.clone().unwrap_or_default();
        let mut items = Vec::new();
        if !is_current {
            items.push(menu_item(format!("Checkout {name}"), "", MenuAction::GitLogOp(LogOp::CheckoutBranch(name.clone()))));
            items.push(menu_item(format!("Merge {name} vào {current}"), "", MenuAction::GitLogOp(LogOp::Merge(name.clone()))));
            items.push(menu_item(format!("Rebase {current} lên {name}"), "", MenuAction::GitLogOp(LogOp::Rebase(name.clone()))));
            items.push(menu_sep());
        }
        if local {
            items.push(menu_item(format!("Đổi tên {name}…"), "", MenuAction::GitPrompt(PromptKind::RenameBranch(name.clone()))));
            if !is_current {
                items.push(menu_item(format!("Xóa nhánh {name}…"), "", MenuAction::GitLogOp(LogOp::DeleteBranch(name.clone()))));
            }
        }
        items.push(menu_item("Copy tên nhánh", "", MenuAction::GitLogOp(LogOp::CopyHash(name))));
        self.show_menu(ev.position, items, cx);
    }

    /// Right pane while "Working Tree" is selected: the files, then the commit box.
    pub(crate) fn render_working_pane(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(root) = self.git.root.clone() else { return div().into_any_element() };
        self.git_ui(cx);
        let tree = self.git.tree.clone();
        let branch = self.git.branch.clone().unwrap_or_default();
        let rows = self.git_change_list(&root, "gw-files-list", false, cx);
        let n = tree.changes.len();
        let focus = self.git_ui.as_ref().map(|u| u.focus.clone());
        div()
            .w(px(420.))
            .flex_none()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(c(theme::BORDER))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .h(px(34.))
                    .px_3()
                    .border_b_1()
                    .border_color(c(theme::BORDER))
                    .child(icon(Ico::Folder, 14., c(theme::WARNING)))
                    .child(div().font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(theme::FG)).child("Working Tree"))
                    .child(crate::git_style::count_pill(n)),
            )
            .child({
                let list = div()
                    .id("gw-files")
                    .key_context("GitPanel")
                    .flex_1()
                    .min_h_0()
                    .pb_2()
                    .on_action(cx.listener(|ws, _: &crate::actions::GitToggleStage, _, cx| ws.git_toggle_stage_sel(cx)))
                    .on_action(cx.listener(|ws, _: &crate::actions::GitOpenSel, window, cx| ws.git_open_sel(window, cx)))
                    .on_action(cx.listener(|ws, _: &crate::actions::GitDiscardSel, window, cx| ws.git_discard_sel(window, cx)))
                    .child(rows);
                match focus {
                    Some(f) => list.track_focus(&f),
                    None => list,
                }
            })
            .child(self.render_commit_box(&branch, &tree, true, cx))
            .into_any_element()
    }

    /// Selects the tip of a branch / tag (or HEAD) in the graph and scrolls to it.
    pub(crate) fn log_select_ref(&mut self, target: String, cx: &mut Context<Self>) {
        let Some(v) = self.log_view.as_ref() else { return };
        let Some(i) = v.commits.iter().position(|c| c.refs.iter().any(|r| if target == "HEAD" { r.starts_with("HEAD") } else { ref_names(r, &target) })) else {
            self.message = Some(format!("{target}: không thấy trong đồ thị đang hiển thị (đổi bộ lọc nhánh?)"));
            return cx.notify();
        };
        self.log_goto(i, cx);
    }
}
