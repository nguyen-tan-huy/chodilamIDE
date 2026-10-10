//! Git in the IDE (IntelliJ's VCS integration, through the `vcs` crate / git CLI): file
//! colors in the tree, change bars in the gutter with rollback, the branch in the status bar
//! and its popup, Commit (Ctrl+K) / Push (Ctrl+Shift+K) / Pull (Ctrl+T), the log (Alt+9),
//! file history, diff against HEAD and Annotate.
use crate::dialogs::{self, DialogCancel, DialogConfirm, Field};
use crate::editor_view::EditorView;
use crate::theme::{self, c};
use crate::workspace::Workspace;
use gpui::{AnyElement, Context, Entity, Focusable, MouseButton, SharedString, Window, div, prelude::*, px};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use vcs::FileStatus;

#[derive(Default)]
pub struct GitState {
    pub root: Option<PathBuf>,
    pub status: HashMap<PathBuf, FileStatus>,
    pub branch: Option<String>,
    /// looked for once (None = not a repository)
    pub probed: bool,
    /// changes by group, upstream, merge/rebase state, stashes (Git tool window)
    pub tree: crate::git_panel::GitTree,
    /// number of the latest refresh started: an older one finishing later is dropped
    pub refresh_seq: u64,
    /// the git command running in the background ("Commit", "Stage"…)
    pub busy: Option<String>,
}

/// Git → Tài khoản: commit identity + https logins (tokens) per host.
pub struct AccountsDialog {
    name: Field,
    email: Field,
    /// identity for every repository (git config --global) instead of this one
    global: bool,
    host: Field,
    user: Field,
    token: Field,
    accounts: Vec<vcs::Account>,
    remotes: Vec<vcs::Remote>,
    error: Option<String>,
}

/// Text color of a file name in the tree / commit list.
pub fn status_color(s: FileStatus) -> u32 {
    match s {
        FileStatus::Modified | FileStatus::Renamed => theme::GIT_MODIFIED,
        FileStatus::Added => theme::GIT_ADDED,
        FileStatus::Untracked => theme::GIT_UNTRACKED,
        FileStatus::Deleted => theme::FG_DIM,
        FileStatus::Conflict => theme::ERROR,
    }
}

/// Where read-only git texts (show, diff) are written to be opened as tabs.
fn scratch_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).unwrap_or_default()).join(".cache"));
    base.join("chodilamide/git")
}

impl Workspace {
    /// Re-reads status + branch in the background, then refreshes the gutter bases of open tabs.
    pub(crate) fn refresh_git(&mut self, cx: &mut Context<Self>) {
        self.git.refresh_seq += 1;
        let seq = self.git.refresh_seq;
        let dir = self.root.clone();
        let known = self.git.root.clone();
        let probed = self.git.probed;
        let open: Vec<(Entity<EditorView>, PathBuf)> = self.tabs.iter().filter_map(|t| t.read(cx).path().cloned().map(|p| (t.clone(), p))).collect();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    let root = if probed { known } else { vcs::repo_root(&dir) };
                    let Some(root) = root else { return (None, HashMap::new(), None, Vec::new(), Default::default()) };
                    // one `git status` for the tree colors and the Git window
                    let tree = crate::git_panel::load_tree(&root);
                    let status = vcs::status_map(&tree.changes);
                    let branch = vcs::branch(&root);
                    // HEAD text of every open file (untracked / new: none → no bars)
                    let bases: Vec<(PathBuf, Option<String>)> = open.iter().map(|(_, p)| (p.clone(), vcs::head_text(&root, p))).collect();
                    (Some(root), status, branch, bases, tree)
                })
                .await;
            let _ = this.update(cx, |ws, cx| {
                if ws.git.refresh_seq != seq {
                    return; // a newer refresh is on its way
                }
                let (root, status, branch, bases, tree) = res;
                let busy = ws.git.busy.take();
                ws.git = GitState { root, status, branch, probed: true, tree, refresh_seq: seq, busy };
                ws.git_ui_sync();
                for (t, path) in open_tabs(ws, cx) {
                    let Some(base) = bases.iter().find(|(p, _)| *p == path).map(|(_, b)| b.clone()) else { continue };
                    t.update(cx, |v, cx| {
                        if v.git_base != base {
                            v.git_base = base;
                            v.git_marks_version = usize::MAX;
                            cx.notify();
                        }
                    });
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// One tab just opened: its HEAD text for the gutter.
    pub(crate) fn git_attach(&mut self, t: &Entity<EditorView>, cx: &mut Context<Self>) {
        let (Some(root), Some(path)) = (self.git.root.clone(), t.read(cx).path().cloned()) else { return };
        let t = t.clone();
        cx.spawn(async move |_, cx| {
            let base = cx.background_executor().spawn(async move { vcs::head_text(&root, &path) }).await;
            let _ = t.update(cx, |v, cx| {
                v.git_base = base;
                v.git_marks_version = usize::MAX;
                cx.notify()
            });
        })
        .detach();
    }

    /// Runs a git command in the background (hooks / big repositories never freeze the UI),
    /// one at a time; `then` sees the result, then everything is refreshed. Errors without
    /// a `then` handler go to the status bar.
    pub(crate) fn run_git(
        &mut self,
        what: &str,
        op: impl FnOnce(&Path) -> Result<String, String> + Send + 'static,
        then: impl FnOnce(&mut Workspace, Result<String, String>, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.git_root_or_message(cx) else { return };
        if let Some(b) = &self.git.busy {
            self.message = Some(format!("Đang chạy {b}… chờ xong rồi thử lại"));
            return cx.notify();
        }
        self.git.busy = Some(what.to_string());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = cx.background_executor().spawn(async move { op(&root) }).await;
            let _ = this.update(cx, |ws, cx| {
                ws.git.busy = None;
                then(ws, res, cx);
                ws.refresh_git(cx);
                ws.reload_log_after_git(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// [`run_git`] that only reports a failure.
    pub(crate) fn run_git_simple(&mut self, what: &'static str, op: impl FnOnce(&Path) -> Result<String, String> + Send + 'static, cx: &mut Context<Self>) {
        self.run_git(what, op, move |ws, r, _| {
            if let Err(e) = r {
                ws.message = Some(format!("{what} lỗi: {}", first_line(&e)));
            }
        }, cx);
    }

    /// Saves every edited tab (git sees the files on disk), like IntelliJ before VCS actions.
    pub(crate) fn save_all_for_git(&mut self, cx: &mut Context<Self>) {
        for t in self.tabs.clone() {
            if t.read(cx).is_dirty() && !t.read(cx).read_only {
                t.update(cx, |v, _| v.save());
            }
        }
    }

    fn git_root_or_message(&mut self, cx: &mut Context<Self>) -> Option<PathBuf> {
        match self.git.root.clone() {
            Some(r) => Some(r),
            None => {
                self.message = Some("Project không nằm trong git repository".into());
                cx.notify();
                None
            }
        }
    }

    /// Opens `text` as a read-only tab named `name` (git show / diff output).
    fn open_text_tab(&mut self, name: &str, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let dir = scratch_dir();
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(name);
        if std::fs::write(&path, text).is_err() {
            self.message = Some(format!("Không ghi được {}", path.display()));
            return;
        }
        // a stale tab of the same name shows the old text: reload it
        if let Some(t) = self.tabs.iter().find(|t| t.read(cx).path() == Some(&path)).cloned() {
            t.update(cx, |v, _| v.editor.buffer.reload_text(text));
        }
        self.open_file(path, window, cx);
        if let Some(t) = self.active.and_then(|i| self.tabs.get(i)) {
            t.update(cx, |v, _| v.read_only = true);
        }
    }

    // ---- actions ---------------------------------------------------------------

    /// The log window (graph of every branch, commit details, changed files); `file`: its history.
    pub(crate) fn git_log(&mut self, file: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        if self.git_root_or_message(cx).is_none() {
            return;
        }
        self.open_log_view(file, window, cx);
    }

    /// The picker opened a commit of the log: write `git show` and open it read-only.
    pub(crate) fn git_open_commit(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !path.starts_with(scratch_dir()) || path.exists() {
            return false;
        }
        let Some(root) = self.git.root.clone() else { return false };
        let hash = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let text = vcs::show(&root, &hash);
        let name = format!("{hash}.diff");
        self.open_text_tab(&name, &text, window, cx);
        true
    }

    /// Diff of the current file (or everything) against HEAD, in a read-only tab.
    pub(crate) fn git_show_diff(&mut self, file: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.git_root_or_message(cx) else { return };
        let text = vcs::diff_text(&root, file.as_deref());
        if text.trim().is_empty() {
            self.message = Some("Không có thay đổi so với HEAD".into());
            return cx.notify();
        }
        let name = match &file {
            Some(f) => format!("{}.HEAD.diff", crate::tree::file_name(f)),
            None => "working-tree.diff".into(),
        };
        self.open_text_tab(&name, &text, window, cx);
    }

    /// Toggles "Annotate" (blame) in the gutter of the active tab.
    pub(crate) fn git_annotate(&mut self, cx: &mut Context<Self>) {
        let Some(t) = self.active.and_then(|i| self.tabs.get(i)).cloned() else { return };
        if !t.read(cx).annotations.is_empty() {
            return t.update(cx, |v, cx| {
                v.annotations.clear();
                cx.notify()
            });
        }
        let (Some(root), Some(path)) = (self.git_root_or_message(cx), t.read(cx).path().cloned()) else { return };
        cx.spawn(async move |this, cx| {
            let blame = cx.background_executor().spawn(async move { vcs::blame(&root, &path) }).await;
            let _ = this.update(cx, |ws, cx| {
                if blame.is_empty() {
                    ws.message = Some("Không annotate được (file chưa commit?)".into());
                    return cx.notify();
                }
                let lines: Vec<String> = blame.iter().map(|b| format!("{} {} {}", b.date, b.short, b.author.chars().take(12).collect::<String>())).collect();
                t.update(cx, |v, cx| {
                    v.annotations = lines;
                    cx.notify()
                });
            });
        })
        .detach();
    }

    /// Rolls back the change block under the caret to its HEAD text (one undo step).
    pub(crate) fn git_rollback_hunk(&mut self, cx: &mut Context<Self>) {
        let Some(t) = self.active.and_then(|i| self.tabs.get(i)).cloned() else { return };
        let done = t.update(cx, |v, cx| {
            let Some(base) = v.git_base.clone() else { return false };
            let text = v.editor.buffer.text();
            let hunks = vcs::diff_lines(&base, &text);
            let line = v.editor.buffer.line_of(v.editor.head);
            let Some((range, old)) = vcs::revert_hunk(&base, &hunks, line) else { return false };
            let b = &v.editor.buffer;
            let start = b.line_start(range.start.min(b.line_count()));
            let end = if range.end >= b.line_count() { b.len() } else { b.line_start(range.end) };
            // a last line without "\n" keeps having none
            let old = if end == b.len() && !text.ends_with('\n') { old.trim_end_matches('\n').to_string() } else { old };
            v.editor.buffer.break_group();
            v.editor.anchor = start;
            v.editor.head = end;
            v.editor.insert(&old);
            v.editor.buffer.break_group();
            cx.notify();
            true
        });
        if !done {
            self.message = Some("Dòng này không có thay đổi so với HEAD".into());
            cx.notify();
        }
    }

    pub(crate) fn git_rollback_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let name = crate::tree::file_name(&path);
        self.run_git("Rollback", move |root| vcs::rollback_file(root, &path), move |ws, r, _| {
            ws.message = Some(match r {
                Ok(_) => format!("Đã rollback {name} về HEAD"),
                Err(e) => format!("Rollback lỗi: {}", first_line(&e)),
            })
        }, cx);
    }

    pub(crate) fn git_push(&mut self, cx: &mut Context<Self>) {
        self.git_push_with(false, cx)
    }

    /// `force`: --force-with-lease (after amend / rebase of pushed commits; refuses when the
    /// remote got commits you have not fetched).
    pub(crate) fn git_push_with(&mut self, force: bool, cx: &mut Context<Self>) {
        self.message = Some("git push…".into());
        self.run_git("Push", move |root| vcs::push(root, force), |ws, res, cx| match res {
            Ok(out) => ws.message = Some(format!("Push xong{}", out.lines().rfind(|l| l.contains("->")).map(|l| format!(": {}", l.trim())).unwrap_or_default())),
            Err(e) => ws.git_error("Push", &e, cx),
        }, cx);
    }

    /// Ctrl+T: git pull (--autostash: uncommitted edits are kept).
    pub(crate) fn git_pull(&mut self, cx: &mut Context<Self>) {
        self.save_all_for_git(cx);
        self.message = Some("git pull…".into());
        self.run_git("Pull", |root| vcs::pull(root), |ws, res, cx| match res {
            Ok(out) => {
                ws.message = Some(format!("Pull xong: {}", out.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("").trim()));
                // exit code 0 even when the autostash could not be put back: look for conflicts
                ws.open_git_if_conflicts(cx);
            }
            Err(e) => {
                ws.git_error("Pull", &e, cx);
                ws.open_git_if_conflicts(cx);
            }
        }, cx);
    }

    /// After merge / rebase / pull / cherry-pick: a stop on conflicts opens the Git window.
    pub(crate) fn open_git_if_conflicts(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.git.root.clone() else { return };
        let changes = vcs::changes(&root);
        if vcs::effective_state(&root, &changes) != vcs::RepoState::Clean {
            self.message = Some("Có xung đột - giải quyết trong cửa sổ Git (Continue / Abort ở thanh vàng)".into());
            self.pending_git_window = true;
            cx.notify();
        }
    }

    /// Undo the last commit (changes stay staged, its message goes back into the commit box).
    /// A commit that is already on the remote asks first.
    pub(crate) fn git_undo_last_commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.git_root_or_message(cx) else { return };
        if vcs::head_pushed(&root) {
            self.open_confirm_pub(
                "Undo commit đã push?".into(),
                "Commit này đã có trên remote. Undo chỉ đổi ở máy bạn (thay đổi về lại mục Đã stage); muốn đẩy lại phải force push và sẽ ảnh hưởng người khác đã pull nó.".into(),
                crate::context_menu::ConfirmAction::GitUndoCommit,
                window,
                cx,
            );
        } else {
            self.git_undo_last_commit_now(cx);
        }
    }

    pub(crate) fn git_undo_last_commit_now(&mut self, cx: &mut Context<Self>) {
        self.save_all_for_git(cx);
        self.run_git("Undo commit", |root| {
            let message = vcs::last_message(root);
            vcs::undo_last_commit(root).map(|_| message)
        }, |ws, res, cx| match res {
            Ok(message) => {
                ws.message = Some("Đã undo commit: thay đổi nằm ở mục Đã stage, message đã trả về ô commit".into());
                let ui = ws.git_ui(cx);
                if ui.msg.text(cx).is_empty() {
                    ui.msg.set(&message, cx);
                }
                if ws.log_view.as_ref().is_some_and(|v| v.file.is_none()) {
                    ws.log_select_working(cx);
                }
                ws.reload_clean_tabs(cx);
            }
            Err(e) => ws.message = Some(format!("Undo commit lỗi: {}", first_line(&e))),
        }, cx);
    }

    /// A failed push/pull/fetch: a login problem opens the accounts dialog.
    fn git_error(&mut self, what: &str, e: &str, cx: &mut Context<Self>) {
        let first = e.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
        if vcs::is_auth_error(e) {
            self.message = Some(format!("{what} lỗi đăng nhập: {first} - nhập token trong Git → Tài khoản"));
            self.pending_accounts = true;
        } else {
            self.message = Some(format!("{what} lỗi: {first}"));
        }
        cx.notify();
    }

    pub(crate) fn git_fetch(&mut self, cx: &mut Context<Self>) {
        self.message = Some("git fetch…".into());
        self.run_git("Fetch", |root| vcs::fetch(root), |ws, res, cx| match res {
            Ok(_) => ws.message = Some("Fetch xong".into()),
            Err(e) => ws.git_error("Fetch", &e, cx),
        }, cx);
    }

    // ---- accounts dialog ------------------------------------------------------------

    pub(crate) fn open_git_accounts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.git_root_or_message(cx) else { return };
        let ((ln, le), (gn, ge)) = vcs::identity(&root);
        let local = !ln.is_empty() || !le.is_empty();
        let remotes = vcs::remotes(&root);
        let host = remotes.iter().find_map(|r| r.https_host.clone()).unwrap_or_else(|| "github.com".into());
        let d = AccountsDialog {
            name: Field::new("Tên (user.name)", "vd: Nguyễn Tấn Huy", if local { &ln } else { &gn }, cx),
            email: Field::new("Email (user.email)", "vd: huy@longvan.net", if local { &le } else { &ge }, cx),
            global: !local,
            host: Field::new("Host", "github.com / gitlab.company.vn", &host, cx),
            user: Field::new("Username", "tên đăng nhập", "", cx),
            token: Field::new("Token", "personal access token (không phải mật khẩu)", "", cx),
            accounts: vcs::accounts(),
            remotes,
            error: None,
        };
        dialogs::focus_later(d.user.input.read(cx).focus_handle(cx), window, cx);
        self.accounts_dialog = Some(d);
        cx.notify();
    }

    fn close_git_accounts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.accounts_dialog = None;
        if let Some(i) = self.active {
            self.activate(i, window, cx);
        }
        cx.notify();
    }

    /// Saves the identity, and the login when a token was typed.
    fn save_git_accounts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.git.root.clone() else { return };
        let Some(d) = self.accounts_dialog.as_mut() else { return };
        let (name, email) = (d.name.text(cx), d.email.text(cx));
        let res = vcs::set_identity(&root, &name, &email, d.global).and_then(|_| {
            let (host, user, token) = (d.host.text(cx), d.user.text(cx), d.token.text(cx));
            if token.is_empty() { Ok(false) } else { vcs::save_account(&host, &user, &token).map(|_| true) }
        });
        match res {
            Ok(saved) => {
                self.message = Some(if saved { "Đã lưu tài khoản Git (token ở ~/.git-credentials, chỉ bạn đọc được)".into() } else { "Đã lưu tên / email commit".into() });
                self.close_git_accounts(window, cx);
            }
            Err(e) => {
                d.error = Some(e);
                cx.notify();
            }
        }
    }

    pub(crate) fn render_git_accounts(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let d = self.accounts_dialog.as_ref()?;
        let accounts: Vec<AnyElement> = d
            .accounts
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let acc = a.clone();
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .px_2()
                    .child(div().flex_1().text_color(c(theme::FG)).child(SharedString::from(format!("{}  ·  {}", a.host, a.username))))
                    .child(
                        div()
                            .id(("acc-x", i))
                            .px_1()
                            .text_color(c(theme::FG_DIM))
                            .hover(|s| s.text_color(c(theme::ERROR)))
                            .child("Xóa")
                            .on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, cx| {
                                let r = vcs::remove_account(&acc);
                                if let Some(d) = ws.accounts_dialog.as_mut() {
                                    d.error = r.err();
                                    d.accounts = vcs::accounts();
                                }
                                cx.notify()
                            })),
                    )
                    .into_any_element()
            })
            .collect();
        let remotes: Vec<String> = d
            .remotes
            .iter()
            .map(|r| match &r.https_host {
                Some(h) => format!("{}: {} (https - cần tài khoản cho {h})", r.name, r.url),
                None => format!("{}: {} (ssh - dùng SSH key, không cần token)", r.name, r.url),
            })
            .collect();
        let body = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(dialogs::section("Người commit"))
            .child(d.name.row())
            .child(d.email.row())
            .child(dialogs::choice_row("Áp dụng cho", &["Project này".to_string(), "Mọi project (global)".to_string()], if d.global { 1 } else { 0 }, "acc-scope", cx, |ws: &mut Workspace, i, _| {
                if let Some(d) = ws.accounts_dialog.as_mut() {
                    d.global = i == 1;
                }
            }))
            .child(dialogs::section("Remote của project"))
            .children(remotes.into_iter().map(|r| div().px_2().text_color(c(theme::FG_DIM)).child(SharedString::from(r))))
            .child(dialogs::section("Tài khoản đã lưu (push / pull qua https)"))
            .children(accounts)
            .when(d.accounts.is_empty(), |el| el.child(div().px_2().text_color(c(theme::FG_DIM)).child("Chưa có")))
            .child(dialogs::section("Thêm / thay tài khoản"))
            .child(d.host.row())
            .child(d.user.row())
            .child(d.token.row())
            .child(div().px_2().text_size(px(11.)).text_color(c(theme::FG_DIM)).child("GitHub: Settings → Developer settings → Personal access tokens (quyền repo). GitLab: Preferences → Access Tokens (read/write_repository)."))
            .when_some(d.error.clone(), |el, e| el.child(div().text_color(c(theme::ERROR)).child(SharedString::from(e))));
        let footer = div()
            .flex()
            .gap_2()
            .child(dialogs::button("acc-cancel", "Đóng (Esc)", false, cx, |ws: &mut Workspace, window, cx| ws.close_git_accounts(window, cx)))
            .child(dialogs::button("acc-ok", "Lưu (Enter)", true, cx, |ws: &mut Workspace, window, cx| ws.save_git_accounts(window, cx)));
        Some(
            dialogs::modal("acc-body", "Tài khoản Git", 720., body, footer)
                .on_action(cx.listener(|ws, _: &DialogConfirm, window, cx| ws.save_git_accounts(window, cx)))
                .on_action(cx.listener(|ws, _: &DialogCancel, window, cx| ws.close_git_accounts(window, cx)))
                .into_any_element(),
        )
    }

    /// The branch popup (the ⎇ branch button on the toolbar / Alt+`).
    pub(crate) fn git_branch_menu(&mut self, at: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        use crate::context_menu::{MenuAction as A, PromptKind as P, menu_item as item};
        if self.git_root_or_message(cx).is_none() {
            return;
        }
        let current = self.git.branch.clone().unwrap_or_default();
        let mut items = vec![
            item("Commit…", "Ctrl+K", A::GitCommit),
            item("Push", "Ctrl+Shift+K", A::GitPush),
            item("Push (force-with-lease)…", "", A::GitForcePush),
            item("Pull", "Ctrl+T", A::GitPull),
            item("Fetch", "", A::GitFetch),
            item("Log (graph mọi nhánh)", "", A::GitLog),
            item("Cửa sổ Git (thay đổi, stash, xung đột)", "Alt+9", A::GitPanel),
            item("Xem mọi thay đổi (diff)", "", A::GitDiffAll),
            crate::context_menu::menu_sep(),
            item("Nhánh mới…", "", A::GitNewBranch),
            item(format!("Đổi tên nhánh {current}…"), "", A::GitPrompt(P::RenameBranch(current.clone()))),
            item("Merge nhánh khác vào đây…", "", A::GitPrompt(P::MergeBranch)),
            item("Rebase nhánh này lên…", "", A::GitPrompt(P::RebaseBranch)),
            item("Tài khoản Git…", "", A::GitAccounts),
            crate::context_menu::menu_sep(),
        ];
        // from the last refresh: no git call while the menu opens
        for (b, cur) in self.git.tree.branches.iter().take(40) {
            let label = if *cur { format!("● {b}") } else { format!("   {b}") };
            items.push(item(label, "", A::GitCheckout(b.clone())));
        }
        self.show_menu(at, items, cx);
    }

    /// Switches branch in the background. A remote branch: its local branch, or a new one
    /// tracking it (an existing local branch is never reset).
    pub(crate) fn git_checkout(&mut self, branch: &str, create: bool, cx: &mut Context<Self>) -> Result<(), String> {
        if self.git.root.is_none() {
            return Err("Không có git repository".into());
        }
        if self.tabs.iter().any(|t| t.read(cx).is_dirty()) {
            return Err("Còn file chưa lưu - lưu (Ctrl+S) trước khi đổi nhánh".into());
        }
        let b = branch.to_string();
        self.run_git("Checkout", move |root| vcs::checkout(root, &b, create), |ws, r, _| {
            ws.message = Some(match r {
                Ok(_) => format!("Đã chuyển sang nhánh {}", vcs::branch(ws.git.root.as_deref().unwrap_or(Path::new("."))).unwrap_or_default()),
                Err(e) => format!("Checkout lỗi: {}", first_line(&e)),
            })
        }, cx);
        Ok(())
    }

    // ---- commit (in the Git window) ------------------------------------------------

    /// Ctrl+K: the Git window with the commit message focused (SmartGit / IntelliJ's
    /// non-modal commit: the files, what is staged and the message in one place).
    pub(crate) fn open_commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(_) = self.git_root_or_message(cx) else { return };
        // unsaved edits would not be committed: save them first like IntelliJ does
        self.save_all_for_git(cx);
        self.refresh_git(cx);
        self.git_log(None, window, cx);
        self.log_select_working(cx);
        let ui = self.git_ui(cx);
        let fh = ui.msg.input.read(cx).focus_handle(cx);
        dialogs::focus_later(fh, window, cx);
    }

    /// Commits what is staged. Nothing staged: the selected files (or, with none selected,
    /// every tracked change) are staged first, like IntelliJ's "commit all changes".
    pub(crate) fn do_commit(&mut self, and_push: bool, cx: &mut Context<Self>) {
        self.save_all_for_git(cx);
        let ui = self.git_ui(cx);
        let msg = ui.msg.text(cx);
        let amend = ui.amend;
        let sel: Vec<PathBuf> = ui.selected.iter().map(|(_, p)| p.clone()).collect();
        let changes = self.git.tree.changes.clone();
        if changes.iter().any(|c| c.conflicted()) {
            self.message = Some("Còn file xung đột - giải quyết trước khi commit".into());
            return cx.notify();
        }
        let any_staged = changes.iter().any(|c| c.staged());
        let merging = self.git.tree.state == Some(vcs::RepoState::Merging);
        let stage_first: Vec<PathBuf> = if any_staged || merging || amend {
            Vec::new()
        } else {
            changes.iter().filter(|c| !c.untracked() && (sel.is_empty() || sel.contains(&c.path))).flat_map(|c| c.paths()).collect()
        };
        if msg.is_empty() && !amend {
            self.message = Some("Chưa nhập commit message".into());
            return cx.notify();
        }
        self.run_git("Commit", move |root| vcs::commit(root, &stage_first, &msg, amend), move |ws, r, cx| match r {
            Ok(_) => {
                ws.message = Some(format!("Đã commit: {}", ws.git.root.as_deref().map(vcs::last_message).unwrap_or_default().lines().next().unwrap_or("")));
                let ui = ws.git_ui(cx);
                ui.msg.set("", cx);
                ui.amend = false;
                ui.selected.clear();
                if and_push {
                    ws.git_push(cx);
                }
            }
            Err(e) => ws.message = Some(format!("Commit lỗi: {}", first_line(&e))),
        }, cx);
    }
}

/// First non-empty line of a git error.
pub(crate) fn first_line(e: &str) -> String {
    e.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string()
}

#[cfg(test)]
impl Workspace {
    pub(crate) fn commit_for_test(&mut self, message: &str, cx: &mut Context<Self>) {
        self.git_ui(cx).msg.set(message, cx);
        self.do_commit(false, cx)
    }
}

fn open_tabs(ws: &Workspace, cx: &Context<Workspace>) -> Vec<(Entity<EditorView>, PathBuf)> {
    ws.tabs.iter().filter_map(|t| t.read(cx).path().cloned().map(|p| (t.clone(), p))).collect()
}
