//! "New Spring Boot project / module": one dialog for both places that create one - the project
//! chooser (a new project in a folder of your choice) and the Maven window (a new module of the
//! open project). It only collects the choices; the files come from the `scaffold` crate.
use crate::dialogs::{self, DialogCancel, DialogConfirm, Field};
use crate::theme::{self, c};
use gpui::{Context, EventEmitter, FocusHandle, Focusable, IntoElement, MouseButton, Render, SharedString, Window, div, prelude::*, px};
use scaffold::{Coords, Kind, Spec};
use std::path::{Path, PathBuf};

/// Where the dialog was opened from.
#[derive(Clone)]
pub enum NewMode {
    /// a project of its own, in a folder chosen in the dialog
    Project,
    /// a module inside the open project at `root`; `parent` is its pom when that is an aggregator
    Module { root: PathBuf, parent: Option<ParentPom> },
}

#[derive(Clone)]
pub struct ParentPom {
    pub coords: Coords,
    /// the parent pom already brings Spring Boot's dependency / plugin management
    pub boot: bool,
}

pub enum NewProjectEvent {
    /// `dir` is the project, or the module's folder. `attached`: the module was added to the
    /// parent pom (otherwise it is independent and the host registers it).
    Created { dir: PathBuf, attached: bool },
    Cancel,
}

pub struct NewProjectDialog {
    mode: NewMode,
    location: Field,
    group: Field,
    artifact: Field,
    package: Field,
    boot: Field,
    java: usize,
    war: usize,
    /// Project: single / multi. Module with a parent pom: child / independent
    kind: usize,
    deps: Vec<bool>,
    git_init: bool,
    auto_package: String,
    auto_boot: String,
    error: Option<String>,
    focus: FocusHandle,
}

const JAVA: [u32; 5] = [8, 11, 17, 21, 25];

fn default_boot(java: u32) -> &'static str {
    // Spring Boot 3 needs Java 17; 2.7 is the last line for older JDKs (the field stays editable)
    if java >= 17 { "3.5.0" } else { "2.7.18" }
}

/// The newest installed JDK among the offered versions (else 17).
fn default_java_index() -> usize {
    let installed: Vec<u32> = toolchain::find_jdks().iter().filter_map(|j| toolchain::jdk_major(&j.path)).collect();
    let pick = [21u32, 17, 11, 8].into_iter().find(|v| installed.contains(v)).unwrap_or(17);
    JAVA.iter().position(|v| *v == pick).unwrap_or(2)
}

impl EventEmitter<NewProjectEvent> for NewProjectDialog {}

impl Focusable for NewProjectDialog {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl NewProjectDialog {
    pub fn new(mode: NewMode, location_default: &str, cx: &mut Context<Self>) -> Self {
        let (group_default, project) = match &mode {
            NewMode::Project => (String::from("com.example"), true),
            NewMode::Module { parent, root } => {
                let from_root = std::fs::read_to_string(root.join("pom.xml")).ok().and_then(|t| scaffold::read_pom(&t)).map(|(c, _)| c.group);
                (parent.as_ref().map(|p| p.coords.group.clone()).or(from_root).unwrap_or_else(|| "com.example".into()), false)
            }
        };
        let java = default_java_index();
        let boot_default = default_boot(JAVA[java]);
        let location = Field::new("Vị trí", "thư mục chứa project", location_default, cx);
        let group = Field::new("Group", "vd: vn.longvan", &group_default, cx);
        let artifact = Field::new(if project { "Artifact" } else { "Tên module" }, "vd: demo-api", "", cx);
        let package = Field::new("Package", "tự lấy từ Group + Artifact", "", cx);
        let boot = Field::new("Spring Boot", "vd: 2.7.18 hoặc 3.5.0", boot_default, cx);
        // typing group / artifact keeps the package in step (until the package is edited by hand)
        for f in [&group, &artifact] {
            cx.subscribe(&f.input, |this, _, _: &crate::input::InputEvent, cx| this.sync_package(cx)).detach();
        }
        for f in [&location, &package, &boot] {
            cx.subscribe(&f.input, |this, _, _: &crate::input::InputEvent, cx| {
                this.error = None;
                cx.notify()
            })
            .detach();
        }
        let mut d = Self {
            mode,
            location,
            group,
            artifact,
            package,
            boot,
            java,
            war: 0,
            kind: 0,
            deps: scaffold::DEPS.iter().map(|d| d.id == "web").collect(),
            git_init: project,
            auto_package: String::new(),
            auto_boot: boot_default.to_string(),
            error: None,
            focus: cx.focus_handle(),
        };
        d.sync_package(cx);
        d
    }

    /// The field to focus when the dialog opens.
    pub fn first_focus(&self, cx: &gpui::App) -> FocusHandle {
        self.artifact.input.read(cx).focus_handle(cx)
    }

    fn sync_package(&mut self, cx: &mut Context<Self>) {
        let derived = scaffold::default_package(&self.group.text(cx), &self.artifact.text(cx));
        let current = self.package.text(cx);
        if current.is_empty() || current == self.auto_package {
            self.package.set(&derived, cx);
        }
        self.auto_package = derived;
        self.error = None;
        cx.notify();
    }

    /// A new Java version moves the Spring Boot field with it, unless the user typed their own.
    fn pick_java(&mut self, i: usize, cx: &mut Context<Self>) {
        self.java = i;
        let wanted = default_boot(JAVA[i]).to_string();
        if self.boot.text(cx) == self.auto_boot {
            self.boot.set(&wanted, cx);
        }
        self.auto_boot = wanted;
    }

    fn pick_location(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Chọn thư mục chứa project".into()) });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                if let Some(p) = paths.into_iter().next() {
                    let _ = this.update(cx, |d, cx| d.location.set(&p.display().to_string(), cx));
                }
            }
        })
        .detach();
    }

    /// Module mode with a parent pom offers "child of the parent" or "independent".
    fn child_choice(&self) -> bool {
        matches!(&self.mode, NewMode::Module { parent: Some(p), .. } if p.boot)
    }

    fn build_spec(&self, cx: &Context<Self>) -> Spec {
        let artifact = self.artifact.text(cx).trim().to_string();
        let group = self.group.text(cx).trim().to_string();
        let kind = match (&self.mode, self.kind) {
            (NewMode::Project, 1) => Kind::Multi { module: format!("{artifact}-app") },
            (NewMode::Module { parent: Some(p), .. }, 0) if p.boot => Kind::Child(p.coords.clone()),
            _ => Kind::Single,
        };
        Spec {
            group,
            artifact,
            description: String::new(),
            package: self.package.text(cx).trim().to_string(),
            java: JAVA[self.java],
            boot: self.boot.text(cx).trim().to_string(),
            war: self.war == 1,
            deps: scaffold::DEPS.iter().zip(&self.deps).filter(|(_, on)| **on).map(|(d, _)| d.id.to_string()).collect(),
            kind,
        }
    }

    fn create(&mut self, cx: &mut Context<Self>) {
        let spec = self.build_spec(cx);
        let result = (|| -> Result<NewProjectEvent, String> {
            scaffold::validate(&spec)?;
            match &self.mode {
                NewMode::Project => {
                    let location = expand(&self.location.text(cx));
                    if location.as_os_str().is_empty() {
                        return Err("Chọn thư mục chứa project".into());
                    }
                    let dir = location.join(&spec.artifact);
                    scaffold::write(&dir, &spec)?;
                    if self.git_init {
                        // not a commit: git may not know who the user is yet
                        let _ = std::process::Command::new("git").args(["init", "-q", "-b", "main"]).current_dir(&dir).status();
                    }
                    Ok(NewProjectEvent::Created { dir, attached: false })
                }
                NewMode::Module { root, parent } => {
                    let dir = root.join(&spec.artifact);
                    let attached = matches!(spec.kind, Kind::Child(_));
                    scaffold::write(&dir, &spec)?;
                    if attached {
                        // the new module joins the parent pom's <modules>
                        let pom = root.join("pom.xml");
                        let text = std::fs::read_to_string(&pom).map_err(|e| format!("{}: {e}", pom.display()))?;
                        let updated = scaffold::add_module_to_pom(&text, &spec.artifact)?;
                        std::fs::write(&pom, updated).map_err(|e| format!("Không ghi được {}: {e}", pom.display()))?;
                    }
                    let _ = parent;
                    Ok(NewProjectEvent::Created { dir, attached })
                }
            }
        })();
        match result {
            Ok(ev) => cx.emit(ev),
            Err(e) => {
                self.error = Some(e);
                cx.notify();
            }
        }
    }
}

/// "~/x" → $HOME/x.
fn expand(p: &str) -> PathBuf {
    let p = p.trim();
    let home = || PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    match p.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None if p == "~" => home(),
        None => PathBuf::from(p),
    }
}

/// The aggregator pom of a folder, when it has one: its coordinates and whether Spring Boot is set up.
pub fn read_parent(root: &Path) -> Option<ParentPom> {
    let text = std::fs::read_to_string(root.join("pom.xml")).ok()?;
    let (coords, packaging) = scaffold::read_pom(&text)?;
    (packaging == "pom").then(|| ParentPom { coords, boot: scaffold::is_spring_boot_pom(&text) })
}

impl Render for NewProjectDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let project = matches!(self.mode, NewMode::Project);
        let note = |t: String, color: u32| div().px_2().text_size(px(11.5)).text_color(c(color)).child(SharedString::from(t));
        let mut body = div().flex().flex_col().gap_2();
        if project {
            body = body.child(
                self.location
                    .row()
                    .child(div().id("np-browse").flex_none().px_3().py(px(2.)).rounded_full().border_1().border_color(c(theme::BORDER)).hover(|s| s.bg(c(theme::ROW_HOVER))).child("Chọn…").on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|d, _, window, cx| d.pick_location(window, cx)),
                    )),
            );
        } else if let NewMode::Module { root, .. } = &self.mode {
            body = body.child(note(format!("Module sẽ nằm trong {}", root.display()), theme::FG_DIM));
        }
        body = body.child(self.group.row()).child(self.artifact.row()).child(self.package.row());
        if project {
            let opts = vec!["Một module".to_string(), "Nhiều module (parent pom + module đầu tiên)".to_string()];
            body = body.child(dialogs::choice_row("Kiểu", &opts, self.kind, "np-kind", cx, |d: &mut Self, i, _| d.kind = i));
        } else if self.child_choice() {
            let opts = vec!["Module con của parent pom".to_string(), "Độc lập (pom riêng)".to_string()];
            body = body.child(dialogs::choice_row("Kiểu", &opts, self.kind, "np-kind", cx, |d: &mut Self, i, _| d.kind = i));
        } else if let NewMode::Module { parent, .. } = &self.mode {
            body = body.child(note(
                match parent {
                    Some(_) => "Parent pom của project không dùng Spring Boot: module này là project độc lập (có pom riêng, spring-boot-starter-parent)".to_string(),
                    None => "Project chưa có parent pom: module này là project độc lập, được thêm vào cây Maven của IDE".to_string(),
                },
                theme::FG_DIM,
            ));
        }
        let inherits = !project && self.child_choice() && self.kind == 0;
        if !inherits {
            let java: Vec<String> = JAVA.iter().map(|v| format!("Java {v}")).collect();
            body = body.child(dialogs::choice_row("Java", &java, self.java, "np-java", cx, |d: &mut Self, i, cx| d.pick_java(i, cx))).child(self.boot.row());
        } else {
            body = body.child(note("Java và Spring Boot kế thừa từ parent pom".into(), theme::FG_DIM));
        }
        body = body.child(dialogs::choice_row("Đóng gói", &["Jar".to_string(), "War".to_string()], self.war, "np-war", cx, |d: &mut Self, i, _| d.war = i));
        body = body.child(dialogs::section("Dependencies"));
        let items = scaffold::DEPS.iter().enumerate().map(|(i, dep)| {
            let on = self.deps.get(i).copied().unwrap_or(false);
            div()
                .id(("np-dep", i))
                .w(px(220.))
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .px_2()
                .py(px(2.))
                .rounded_md()
                .hover(|s| s.bg(c(theme::ROW_HOVER)))
                .child(crate::git_style::check_box(on, false))
                .child(div().text_color(c(theme::FG)).child(dep.label))
                .on_mouse_down(MouseButton::Left, cx.listener(move |d, _, _, cx| {
                    if let Some(v) = d.deps.get_mut(i) {
                        *v = !*v;
                    }
                    cx.notify()
                }))
        });
        body = body.child(div().flex().flex_row().flex_wrap().children(items));
        if project {
            body = body.child(
                div()
                    .id("np-git")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py(px(2.))
                    .rounded_md()
                    .hover(|s| s.bg(c(theme::ROW_HOVER)))
                    .child(crate::git_style::check_box(self.git_init, false))
                    .child("Khởi tạo git (git init)")
                    .on_mouse_down(MouseButton::Left, cx.listener(|d, _, _, cx| {
                        d.git_init = !d.git_init;
                        cx.notify()
                    })),
            );
        }
        if let Some(e) = self.error.clone() {
            body = body.child(div().px_2().text_color(c(theme::ERROR)).child(SharedString::from(e)));
        }
        let title = if project { "Dự án Spring Boot mới" } else { "Module Spring Boot mới" };
        let footer = div()
            .flex()
            .gap_2()
            .child(dialogs::button("np-cancel", "Hủy (Esc)", false, cx, |_: &mut Self, _, cx| cx.emit(NewProjectEvent::Cancel)))
            .child(dialogs::button("np-create", "Tạo (Enter)", true, cx, |d: &mut Self, _, cx| d.create(cx)));
        dialogs::modal("np-body", title, 760., body, footer)
            .on_action(cx.listener(|d, _: &DialogConfirm, _, cx| d.create(cx)))
            .on_action(cx.listener(|_, _: &DialogCancel, _, cx| cx.emit(NewProjectEvent::Cancel)))
    }
}

#[cfg(test)]
impl NewProjectDialog {
    fn fill(&mut self, artifact: &str, location: &str, cx: &mut Context<Self>) {
        self.artifact.set(artifact, cx);
        self.location.set(location, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("np-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A new multi-module project: parent pom + first module on disk, git initialised.
    #[gpui::test]
    fn creates_a_multi_module_project(cx: &mut TestAppContext) {
        let base = temp("project");
        let d = cx.new(|cx| NewProjectDialog::new(NewMode::Project, "", cx));
        let created = std::rc::Rc::new(std::cell::RefCell::new(None));
        let c2 = created.clone();
        cx.update(|cx| {
            cx.subscribe(&d, move |_, ev: &NewProjectEvent, _| {
                if let NewProjectEvent::Created { dir, .. } = ev {
                    *c2.borrow_mut() = Some(dir.clone());
                }
            })
            .detach();
        });
        d.update(cx, |d, cx| {
            d.fill("demo-api", base.to_str().unwrap(), cx);
            d.kind = 1; // multi-module
        });
        d.update(cx, |d, cx| d.create(cx));
        let dir = created.borrow().clone().expect("the project was created");
        assert_eq!(dir, base.join("demo-api"));
        assert!(dir.join("pom.xml").is_file() && dir.join("demo-api-app/pom.xml").is_file());
        assert!(std::fs::read_to_string(dir.join("pom.xml")).unwrap().contains("<module>demo-api-app</module>"));
        assert!(dir.join(".git").exists(), "git init ran");
        // the same name again is refused, nothing is overwritten
        d.update(cx, |d, cx| {
            d.create(cx);
            assert!(d.error.as_deref().is_some_and(|e| e.contains("không trống")), "{:?}", d.error);
        });
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// A module of an aggregator project joins its parent pom's <modules>.
    #[gpui::test]
    fn adds_a_module_to_the_parent_pom(cx: &mut TestAppContext) {
        let root = temp("module");
        std::fs::write(
            root.join("pom.xml"),
            "<project>\n\t<parent>\n\t\t<groupId>org.springframework.boot</groupId>\n\t\t<artifactId>spring-boot-starter-parent</artifactId>\n\t\t<version>2.7.12</version>\n\t</parent>\n\t<groupId>vn.x</groupId>\n\t<artifactId>root</artifactId>\n\t<version>1.0</version>\n\t<packaging>pom</packaging>\n\t<modules>\n\t\t<module>a</module>\n\t</modules>\n</project>\n",
        )
        .unwrap();
        let parent = read_parent(&root).expect("an aggregator pom");
        assert!(parent.boot && parent.coords.artifact == "root");
        let d = cx.new(|cx| NewProjectDialog::new(NewMode::Module { root: root.clone(), parent: Some(parent) }, "", cx));
        // typing is one step, pressing Create a later one: the package follows the artifact in between
        d.update(cx, |d, cx| d.fill("svc-b", "", cx));
        d.update(cx, |d, cx| {
            assert_eq!(d.package.text(cx), "vn.x.svcb");
            d.create(cx);
            assert!(d.error.is_none(), "{:?}", d.error);
        });
        let pom = std::fs::read_to_string(root.join("pom.xml")).unwrap();
        assert!(pom.contains("<module>a</module>") && pom.contains("<module>svc-b</module>"), "{pom}");
        let child = std::fs::read_to_string(root.join("svc-b/pom.xml")).unwrap();
        assert!(child.contains("<artifactId>root</artifactId>") && !child.contains("<java.version>"), "{child}");
        assert!(root.join("svc-b/src/main/java/vn/x/svcb/SvcBApplication.java").is_file());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
