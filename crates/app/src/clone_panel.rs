//! "Clone từ Git" of the project chooser: paste a repository URL, pick where it goes and open it.
//! A new machine needs nothing set up beforehand: the dialog asks for what is missing - an access
//! token for an https URL (stored like the Git window's "Tài khoản", in ~/.git-credentials), an SSH
//! key for an ssh URL (it can create one and shows the public half to paste into GitLab / GitHub)
//! and the commit identity (user.name / user.email) when git has none.
use crate::dialogs::{self, DialogCancel, DialogConfirm, Field};
use crate::shell::Shell;
use crate::theme::{self, c};
use gpui::{AnyElement, ClipboardItem, Context, Focusable, MouseButton, SharedString, Window, div, prelude::*, px};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub struct CloneForm {
    url: Field,
    parent: Field,
    folder: Field,
    user: Field,
    token: Field,
    name: Field,
    email: Field,
    /// the folder name derived from the URL: replaced while the user has not typed their own
    auto_folder: String,
    running: bool,
    progress: String,
    error: Option<String>,
    /// the public key just created (shown so it can be copied)
    new_key: Option<String>,
}

/// "~/x" → $HOME/x.
fn expand(p: &str) -> PathBuf {
    let p = p.trim();
    match p.strip_prefix("~/") {
        Some(rest) => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(rest),
        None if p == "~" => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()),
        None => PathBuf::from(p),
    }
}

/// Where a new project / clone goes by default: next to the project opened last (that is where
/// the user keeps them), else the first usual project folder, else ~/Git-projects (created on demand).
pub(crate) fn default_parent() -> String {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let beside_recent = crate::projects::read_recent().into_iter().find_map(|p| p.parent().map(PathBuf::from).filter(|d| d.is_dir() && *d != home));
    let dir = beside_recent.or_else(|| crate::projects::project_roots().into_iter().find(|d| d.is_dir())).unwrap_or(home.join("Git-projects"));
    match dir.strip_prefix(&home) {
        Ok(rest) if !rest.as_os_str().is_empty() => format!("~/{}", rest.display()),
        Ok(_) => "~".into(),
        Err(_) => dir.display().to_string(),
    }
}

impl CloneForm {
    /// Screenshot checks / tests: types the address.
    pub(crate) fn set_url(&self, url: &str, cx: &mut Context<Shell>) {
        self.url.set(url, cx);
    }

    #[cfg(test)]
    pub(crate) fn set_parent(&self, dir: &str, cx: &mut Context<Shell>) {
        self.parent.set(dir, cx);
    }

    #[cfg(test)]
    pub(crate) fn folder_text(&self, cx: &gpui::App) -> String {
        self.folder.text(cx)
    }
}

impl Shell {
    pub(crate) fn open_clone(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let url = Field::new("Địa chỉ repo", "https://gitlab.company.vn/team/repo.git  hoặc  git@host:team/repo.git", "", cx);
        let parent = Field::new("Thư mục chứa", "~/Git-projects", &default_parent(), cx);
        let folder = Field::new("Tên thư mục", "tự lấy từ địa chỉ repo", "", cx);
        let user = Field::new("Username", "tên đăng nhập trên máy chủ git", "", cx);
        let token = Field::new("Token", "personal access token (không phải mật khẩu)", "", cx);
        token.input.update(cx, |i, _| i.secret = true);
        let (gn, ge) = vcs::global_identity();
        let name = Field::new("Tên (user.name)", "vd: Nguyễn Tấn Huy", &gn, cx);
        let email = Field::new("Email (user.email)", "vd: huy@company.vn", &ge, cx);
        // typing the URL fills the folder name (until the user types their own)
        cx.subscribe(&url.input, |shell, _, _: &crate::input::InputEvent, cx| shell.clone_url_changed(cx)).detach();
        for f in [&parent, &folder, &user, &token, &name, &email] {
            cx.subscribe(&f.input, |_, _, _: &crate::input::InputEvent, cx| cx.notify()).detach();
        }
        dialogs::focus_later(url.input.read(cx).focus_handle(cx), window, cx);
        self.clone = Some(CloneForm { url, parent, folder, user, token, name, email, auto_folder: String::new(), running: false, progress: String::new(), error: None, new_key: None });
        cx.notify();
    }

    fn clone_url_changed(&mut self, cx: &mut Context<Self>) {
        let Some(f) = self.clone.as_mut() else { return };
        let name = vcs::parse_repo_url(&f.url.text(cx)).map(|u| u.name).unwrap_or_default();
        let current = f.folder.text(cx);
        if current.is_empty() || current == f.auto_folder {
            f.folder.set(&name, cx);
        }
        f.auto_folder = name;
        f.error = None;
        cx.notify();
    }

    pub(crate) fn close_clone(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        // a clone in progress keeps going; the dialog just stays until it ends
        if self.clone.as_ref().is_some_and(|f| f.running) {
            return;
        }
        self.clone = None;
        cx.notify();
    }

    fn pick_clone_parent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Chọn thư mục chứa project".into()) });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                if let Some(p) = paths.into_iter().next() {
                    let _ = this.update(cx, |shell, cx| {
                        if let Some(f) = shell.clone.as_ref() {
                            f.parent.set(&p.display().to_string(), cx);
                        }
                    });
                }
            }
        })
        .detach();
    }

    fn generate_clone_key(&mut self, cx: &mut Context<Self>) {
        let Some(f) = self.clone.as_mut() else { return };
        let email = f.email.text(cx);
        match vcs::generate_ssh_key(&email) {
            Ok(public) => {
                f.new_key = Some(public);
                f.error = None;
            }
            Err(e) => f.error = Some(format!("Không tạo được SSH key: {e}")),
        }
        cx.notify();
    }

    pub(crate) fn start_clone(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.clone.as_mut() else { return };
        if f.running {
            return;
        }
        let fail = |f: &mut CloneForm, msg: &str, cx: &mut Context<Shell>| {
            f.error = Some(msg.to_string());
            cx.notify();
        };
        let url = f.url.text(cx).trim().to_string();
        let Some(parsed) = vcs::parse_repo_url(&url) else {
            return fail(f, "Nhập địa chỉ repo (https://… hoặc git@host:…)", cx);
        };
        let folder = f.folder.text(cx).trim().to_string();
        if folder.is_empty() || folder.contains('/') || folder == ".." {
            return fail(f, "Tên thư mục không hợp lệ", cx);
        }
        let parent = expand(&f.parent.text(cx));
        let dest = parent.join(&folder);
        if dest.exists() && std::fs::read_dir(&dest).map(|mut d| d.next().is_some()).unwrap_or(true) {
            return fail(f, &format!("{} đã tồn tại và không trống", dest.display()), cx);
        }
        if let Err(e) = std::fs::create_dir_all(&parent) {
            return fail(f, &format!("Không tạo được {}: {e}", parent.display()), cx);
        }
        // access: https needs a token (saved for next time), ssh needs a key
        if parsed.https {
            let (user, token) = (f.user.text(cx).trim().to_string(), f.token.text(cx).trim().to_string());
            let has_account = vcs::accounts().iter().any(|a| a.host == parsed.host);
            if !user.is_empty() || !token.is_empty() || !has_account {
                if let Err(e) = vcs::save_account(&parsed.host, &user, &token) {
                    return fail(f, &format!("Tài khoản cho {}: {e} - nhập username và token", parsed.host), cx);
                }
            }
        } else if parsed.ssh && !vcs::has_ssh_key() {
            return fail(f, "Máy chưa có SSH key: bấm \"Tạo SSH key\", dán public key vào máy chủ git rồi Clone lại", cx);
        }
        // who commits: only asked for when git has no identity at all
        let (gn, ge) = vcs::global_identity();
        let (name, email) = (f.name.text(cx).trim().to_string(), f.email.text(cx).trim().to_string());
        if (gn.is_empty() || ge.is_empty()) && !name.is_empty() && !email.is_empty() {
            if let Err(e) = vcs::set_global_identity(&name, &email) {
                return fail(f, &format!("Không lưu được tên/email: {e}"), cx);
            }
        }
        f.running = true;
        f.error = None;
        f.progress = "Đang kết nối…".into();
        cx.notify();

        let progress = Arc::new(Mutex::new(String::new()));
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (p2, d2, url2, dest2) = (progress.clone(), done.clone(), url.clone(), dest.clone());
        let task = cx.background_executor().spawn(async move {
            let r = vcs::clone_repo(&url2, &dest2, |line| *p2.lock().unwrap() = line.to_string());
            d2.store(true, std::sync::atomic::Ordering::SeqCst);
            r
        });
        // git's progress lines reach the dialog while the clone runs
        cx.spawn(async move |this, cx| {
            while !done.load(std::sync::atomic::Ordering::SeqCst) {
                cx.background_executor().timer(std::time::Duration::from_millis(150)).await;
                let line = progress.lock().unwrap().clone();
                if this.update(cx, |shell, cx| {
                    if let Some(f) = shell.clone.as_mut().filter(|f| f.running && !line.is_empty()) {
                        f.progress = line;
                        cx.notify();
                    }
                }).is_err() {
                    return;
                }
            }
        })
        .detach();
        let host_label = parsed.host.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |shell, window, cx| match result {
                Ok(()) => {
                    shell.clone = None;
                    shell.chooser = None;
                    shell.add_project(dest, true, window, cx);
                }
                Err(e) => {
                    let msg = if vcs::is_auth_error(&e) || e.contains("could not read Username") {
                        format!("Đăng nhập {host_label} thất bại: kiểm tra username và token (token cần quyền đọc repository). Chi tiết: {}", e.lines().last().unwrap_or(""))
                    } else if e.contains("Permission denied (publickey)") {
                        format!("{host_label} từ chối SSH key: dán public key của máy này vào tài khoản trên máy chủ git, rồi Clone lại")
                    } else {
                        e
                    };
                    if let Some(f) = shell.clone.as_mut() {
                        f.running = false;
                        f.error = Some(msg);
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(crate) fn render_clone(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let f = self.clone.as_ref()?;
        let url = f.url.text(cx);
        let parsed = vcs::parse_repo_url(&url);
        let dest = expand(&f.parent.text(cx)).join(f.folder.text(cx).trim());
        let (gn, ge) = vcs::global_identity();
        let need_identity = gn.is_empty() || ge.is_empty();
        let running = f.running;

        let note = |t: String, color: u32| div().px_2().text_size(px(11.5)).text_color(c(color)).child(SharedString::from(t));
        let mut body = div().flex().flex_col().gap_2().child(f.url.row()).child(
            f.parent
                .row()
                .child(div().id("clone-browse").flex_none().px_3().py(px(2.)).rounded_full().border_1().border_color(c(theme::BORDER)).hover(|s| s.bg(c(theme::ROW_HOVER))).child("Chọn…").on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|shell, _, window, cx| shell.pick_clone_parent(window, cx)),
                )),
        ).child(f.folder.row());
        if parsed.is_some() {
            body = body.child(note(format!("Sẽ clone vào {}", dest.display()), theme::FG_DIM));
        }

        match &parsed {
            Some(u) if u.https => {
                let account = vcs::accounts().into_iter().find(|a| a.host == u.host);
                body = body.child(dialogs::section("Tài khoản"));
                body = match &account {
                    Some(a) => body.child(note(format!("✓ Đã có tài khoản {} cho {} - để trống nếu dùng lại", a.username, u.host), theme::GIT_ADDED)),
                    None => body.child(note(format!("Máy chưa có tài khoản cho {}. Tạo Personal Access Token trên {} (GitHub: quyền repo; GitLab: read_repository) rồi nhập vào đây - app lưu lại cho các lần sau.", u.host, u.host), theme::WARNING)),
                };
                body = body.child(f.user.row()).child(f.token.row());
            }
            Some(u) if u.ssh => {
                body = body.child(dialogs::section("SSH key"));
                if vcs::has_ssh_key() {
                    body = body.child(note("✓ Máy đã có SSH key (~/.ssh). Nếu máy chủ từ chối, dán public key bên dưới vào tài khoản của bạn trên máy chủ git.".into(), theme::GIT_ADDED));
                } else {
                    body = body.child(note(format!("Máy chưa có SSH key cho {}. Tạo key rồi dán public key vào Settings → SSH Keys của máy chủ git, sau đó Clone.", u.host), theme::WARNING));
                }
                let key = f.new_key.clone().or_else(vcs::ssh_public_key);
                if let Some(k) = key {
                    let k2 = k.clone();
                    body = body.child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(div().flex_1().min_w_0().px_2().py_1().rounded_md().border_1().border_color(c(theme::BORDER)).text_size(px(11.)).text_color(c(theme::FG_DIM)).child(SharedString::from(k)))
                            .child(dialogs::button("clone-copy-key", "Copy", false, cx, move |_: &mut Shell, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(k2.clone())))),
                    );
                } else {
                    body = body.child(div().flex().child(dialogs::button("clone-gen-key", "Tạo SSH key", true, cx, |shell: &mut Shell, _, cx| shell.generate_clone_key(cx))));
                }
            }
            _ => {}
        }

        if need_identity {
            body = body
                .child(dialogs::section("Người commit"))
                .child(note("Git trên máy này chưa biết bạn là ai - cần để commit.".into(), theme::WARNING))
                .child(f.name.row())
                .child(f.email.row());
        }
        if running {
            body = body.child(div().px_2().text_color(c(theme::ACCENT)).child(SharedString::from(format!("⏳ {}", f.progress))));
        }
        if let Some(e) = f.error.clone() {
            body = body.child(div().px_2().text_color(c(theme::ERROR)).child(SharedString::from(e)));
        }

        let footer = div()
            .flex()
            .gap_2()
            .child(dialogs::button("clone-cancel", if running { "Đang clone…" } else { "Hủy (Esc)" }, false, cx, |shell: &mut Shell, window, cx| shell.close_clone(window, cx)))
            .child(dialogs::button("clone-go", "Clone (Enter)", !running, cx, |shell: &mut Shell, window, cx| shell.start_clone(window, cx)));
        Some(
            dialogs::modal("clone-body", "Clone project từ Git", 760., body, footer)
                .on_action(cx.listener(|shell, _: &DialogConfirm, window, cx| shell.start_clone(window, cx)))
                .on_action(cx.listener(|shell, _: &DialogCancel, window, cx| shell.close_clone(window, cx)))
                .into_any_element(),
        )
    }
}
