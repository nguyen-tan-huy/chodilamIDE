//! Run/Debug/Maven sessions of the workspace. Run and Debug both go through
//! chodilamide-debugger (DAP over JDI; `noDebug` for Run) so there is one code path and
//! no dependency on jdtls. Maven runs as a plain process whose output fills a console.
use crate::workspace::Workspace;
use dap_client::{DapClient, DapEvent};
use futures::StreamExt;
use gpui::{Context, Window};
use runner::config::RunConfig;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Run,
    Debug,
    Maven,
    Test,
}

#[derive(Clone, PartialEq)]
pub enum State {
    Building,
    Running,
    Stopped { thread: i64, reason: String },
    Exited(Option<i64>),
}

#[derive(Clone)]
pub struct Frame {
    pub id: i64,
    pub name: String,
    pub path: Option<PathBuf>,
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Var {
    pub name: String,
    pub value: String,
    pub reference: i64,
}

pub struct Session {
    pub id: usize,
    pub name: String,
    pub kind: Kind,
    pub state: State,
    pub console: Vec<(String, bool)>, // (line, is_stderr)
    dap: Option<Arc<DapClient>>,
    process: Option<Arc<Mutex<Child>>>,
    pub frames: Vec<Frame>,
    pub selected_frame: usize,
    /// every thread of the debuggee while stopped (id, name)
    pub threads: Vec<(i64, String)>,
    /// variablesReference → children (fetched lazily)
    pub vars: HashMap<i64, Vec<Var>>,
    pub expanded: HashSet<i64>,
    pub locals_ref: Option<i64>,
    pub config: Option<RunConfig>,
    /// a debugger is attached/launched (Debug, or a test run with Debug)
    pub debugging: bool,
    /// test runs: module, what was run, results once Maven finished
    pub test_module: Option<String>,
    pub test_targets: Vec<runner::test::TestTarget>,
    pub test_results: Vec<runner::test::TestResult>,
    pub test_debug: bool,
    /// Run with Coverage: the JaCoCo exec file of this run
    pub coverage_exec: Option<PathBuf>,
}

impl Session {
    pub(crate) fn dap_client(&self) -> Option<Arc<DapClient>> {
        self.dap.clone()
    }

    pub fn is_live(&self) -> bool {
        !matches!(self.state, State::Exited(_))
    }

    /// Output arrives line by line (debugger and Maven both send whole lines).
    fn push_output(&mut self, text: &str, err: bool) {
        let text = text.strip_suffix('\n').unwrap_or(text);
        for part in text.split('\n') {
            self.console.push((part.trim_end_matches('\r').to_string(), err));
        }
        // cap memory: keep the last 20k lines
        if self.console.len() > 20_000 {
            self.console.drain(0..self.console.len() - 20_000);
        }
    }
}

/// Strips ANSI color codes (Spring Boot's banner/logs use them).
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                while let Some(&n) = chars.peek() {
                    chars.next();
                    if n.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// "at a.b.C.m(C.java:12)" → ("a/b/C.java", 12)
pub fn stack_link(line: &str) -> Option<(String, usize)> {
    let at = line.trim_start().strip_prefix("at ")?;
    let open = at.find('(')?;
    let close = at[open..].find(')')? + open;
    let (file, num) = at[open + 1..close].split_once(':')?;
    let num: usize = num.parse().ok()?;
    let qualified = &at[..open];
    let class_path = qualified.rsplit_once('.').map(|(c, _)| c)?; // drop method
    let pkg = class_path.rsplit_once('.').map(|(p, _)| p).unwrap_or("");
    let rel = if pkg.is_empty() { file.to_string() } else { format!("{}/{}", pkg.replace('.', "/"), file) };
    Some((rel, num))
}

impl Workspace {
    fn session_mut(&mut self, id: usize) -> Option<&mut Session> {
        self.sessions.iter_mut().find(|s| s.id == id)
    }

    pub(crate) fn active_session(&self) -> Option<&Session> {
        self.active_session.and_then(|id| self.sessions.iter().find(|s| s.id == id))
    }

    /// A Run tab for output produced by the IDE itself (HTTP client).
    pub(crate) fn new_http_session(&mut self, name: String, cx: &mut Context<Self>) -> usize {
        let id = self.new_session(name, Kind::Run, None);
        if let Some(s) = self.session_mut(id) {
            s.state = State::Running;
        }
        cx.notify();
        id
    }

    pub(crate) fn session_output(&mut self, id: usize, text: &str, err: bool, cx: &mut Context<Self>) {
        if let Some(s) = self.session_mut(id) {
            s.push_output(text, err);
        }
        cx.notify();
    }

    pub(crate) fn session_finished(&mut self, id: usize, code: Option<i64>, cx: &mut Context<Self>) {
        if let Some(s) = self.session_mut(id) {
            s.state = State::Exited(code);
        }
        cx.notify();
    }

    fn new_session(&mut self, name: String, kind: Kind, config: Option<RunConfig>) -> usize {
        self.next_session += 1;
        let id = self.next_session;
        // drop finished sessions of the same config, like IntelliJ reusing its tab
        self.sessions.retain(|s| s.is_live() || s.name != name);
        self.sessions.push(Session {
            id,
            name,
            kind,
            state: State::Building,
            console: Vec::new(),
            dap: None,
            process: None,
            frames: Vec::new(),
            selected_frame: 0,
            threads: Vec::new(),
            vars: HashMap::new(),
            expanded: HashSet::new(),
            locals_ref: None,
            config,
            debugging: kind == Kind::Debug,
            test_module: None,
            test_targets: Vec::new(),
            test_results: Vec::new(),
            test_debug: false,
            coverage_exec: None,
        });
        self.active_session = Some(id);
        self.panel_open = true;
        self.console_follow = true;
        id
    }

    // ---- configs ---------------------------------------------------------------

    pub(crate) fn root_str(&self) -> String {
        self.root.display().to_string()
    }

    /// Builds, then makes sure the classes about to run really exist in target/: jdtls
    /// may not compile test sources (its .classpath can lack src/test/java - seen on
    /// computing-core), so fall back to `mvn [test-]compile` (offline, then online).
    pub(crate) fn ensure_built(&mut self, module: &project_model::Module, profiles: &[String], tests: bool, classes: Vec<String>, jdk: Option<String>, cx: &mut Context<Self>) -> gpui::Task<anyhow::Result<()>> {
        // like IntelliJ: everything unsaved is saved before a run, so the build sees it
        for t in self.tabs.clone() {
            t.update(cx, |v, _| {
                if v.is_dirty() && !v.read_only {
                    v.save();
                    v.lsp_did_save();
                }
            });
        }
        let build = self.build_task(module, profiles, tests, cx);
        let dir = PathBuf::from(&module.path);
        // for the targeted javac: full (test) classpath and every source root it may need
        let (classpath, sourcepath) = match self.project.as_ref() {
            Some(p) => (runner::classpath::classpath(p, module, true), runner::classpath::sourcepaths(p, module, true)),
            None => (vec![], vec![]),
        };
        let jdk = jdk.or_else(|| runner::config::project_jdk(&self.root_str()));
        let goal = if tests { "test-compile" } else { "compile" };
        let mut goals = vec![goal, "-q"];
        if tests {
            // poms with <maven.test.skip>true</maven.test.skip> never compile tests otherwise
            goals.push("-Dmaven.test.skip=false");
        }
        let cmd = runner::maven::command(&self.root, module, &goals, profiles, true, jdk.clone());
        let argfile = std::env::temp_dir().join(format!("chodilamide-javac-{}-{}.txt", std::process::id(), classes.len()));
        cx.background_executor().spawn(async move {
            let first = build.await;
            // compiled AND newer than the source (a stale .class runs old code)
            let present = |classes: &[String]| classes.iter().all(|c| runner::test_profiles::class_file_fresh(&dir, c, tests, &sourcepath));
            if present(&classes) {
                return Ok(());
            }
            let mut last_err = first.err().map(|e| e.to_string()).unwrap_or_default();
            // 1) javac on exactly the files to run (IntelliJ-like, ignores unrelated broken files)
            let files: Vec<PathBuf> = classes.iter().filter_map(|c| runner::compile::source_file(&sourcepath, c)).collect();
            if !files.is_empty() {
                let out = dir.join(if tests { "target/test-classes" } else { "target/classes" });
                if let Ok(mut c) = runner::compile::javac(jdk.as_deref(), &classpath, &sourcepath, &out, &files, &argfile) {
                    match c.output() {
                        Ok(o) if o.status.success() && present(&classes) => return Ok(()),
                        Ok(o) => last_err = String::from_utf8_lossy(&o.stderr).lines().take(8).collect::<Vec<_>>().join("\n"),
                        Err(e) => last_err = e.to_string(),
                    }
                }
            }
            // 2) Maven, offline then online
            for offline in [true, false] {
                let mut c = Command::new(&cmd.program);
                c.args(&cmd.args).current_dir(&cmd.cwd);
                if offline {
                    c.arg("-o");
                }
                apply_java_home(&mut c, cmd.java_home.as_deref());
                match c.output() {
                    Ok(o) if o.status.success() && present(&classes) => return Ok(()),
                    Ok(o) => {
                        last_err = String::from_utf8_lossy(&o.stdout).lines().filter(|l| l.contains("ERROR")).take(8).collect::<Vec<_>>().join("\n");
                    }
                    Err(e) => last_err = e.to_string(),
                }
            }
            anyhow::bail!("chưa biên dịch được {}:\n{last_err}", classes.join(", "))
        })
    }

    /// What the toolbar's Run/Debug starts: a run configuration or a unit-test profile.
    pub(crate) fn active_target(&self) -> Option<RunTarget> {
        let root = self.root_str();
        let name = runner::config::active(&root);
        let cfgs = runner::config::load(&root);
        if let Some(c) = cfgs.iter().find(|c| Some(&c.name) == name.as_ref()) {
            return Some(RunTarget::Config(c.clone()));
        }
        if let Some(p) = runner::test_profiles::load(&root).into_iter().find(|p| Some(&p.name) == name.as_ref()) {
            return Some(RunTarget::Test(p));
        }
        cfgs.first().cloned().map(RunTarget::Config)
    }

    /// Runs / debugs a unit-test profile (from the toolbar).
    pub(crate) fn run_test_profile(&mut self, p: runner::test_profiles::TestProfile, debug: bool, cx: &mut Context<Self>) {
        let file = PathBuf::from(&p.file);
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        let class = p.class.clone().unwrap_or_else(|| {
            let stem = file.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            match text.lines().find_map(|l| l.trim().strip_prefix("package ").map(|r| r.trim_end_matches(';').trim().to_string())) {
                Some(pk) => format!("{pk}.{stem}"),
                None => stem,
            }
        });
        let method = if p.scope == "class" { None } else { p.method.clone().or_else(|| runner::test_profiles::method_near(&text, p.lnum)) };
        let Some(module) = self.project.as_ref().and_then(|pr| pr.find_module_for_file(&p.file)).map(|m| m.path.clone()) else {
            self.message = Some(format!("{} không thuộc module Maven nào", p.file));
            return;
        };
        self.run_tests(&module, vec![runner::test::TestTarget { class, methods: method.into_iter().collect() }], debug, cx);
    }

    /// Remembers a test run as a profile (shared test-profiles.json) and makes it the
    /// toolbar's active entry, like IntelliJ's temporary test configurations.
    pub(crate) fn remember_test_profile(&mut self, file: &Path, class: &str, method: Option<&str>, debug: bool) {
        let text = std::fs::read_to_string(file).unwrap_or_default();
        let simple = class.rsplit('.').next().unwrap_or(class);
        let lnum = method
            .and_then(|m| text.lines().position(|l| l.contains(&format!("{m}(")) && !l.trim_start().starts_with("//")))
            .map(|i| i + 1)
            .unwrap_or(1);
        let name = match method {
            Some(m) => format!("{simple}.{m}"),
            None => simple.to_string(),
        };
        let profile = runner::test_profiles::TestProfile {
            name: name.clone(),
            file: file.display().to_string(),
            lnum,
            scope: if method.is_some() { "nearest_method".into() } else { "class".into() },
            variant: if debug { "debug".into() } else { "run".into() },
            class: Some(class.to_string()),
            method: method.map(String::from),
            extra: Default::default(),
        };
        let root = self.root_str();
        if runner::test_profiles::upsert(&root, profile).is_ok() {
            let _ = runner::config::set_active(&root, &name);
        }
    }

    pub(crate) fn active_config(&self) -> Option<RunConfig> {
        let root = self.root_str();
        let cfgs = runner::config::load(&root);
        let name = runner::config::active(&root);
        cfgs.iter().find(|c| Some(&c.name) == name.as_ref()).or(cfgs.first()).cloned()
    }

    /// ▶ next to main: reuse the config for that class or create one (saved to the
    /// shared debug-configs.json so nvim sees it too).
    pub(crate) fn run_main(&mut self, path: &Path, main_class: &str, debug: bool, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root_str();
        let mut cfgs = runner::config::load(&root);
        let cfg = match cfgs.iter().find(|c| c.main_class == main_class) {
            Some(c) => c.clone(),
            None => {
                let Some(module) = self.project.as_ref().and_then(|p| p.find_module_for_file(&path.display().to_string())).cloned() else {
                    self.message = Some("File không thuộc module Maven nào".into());
                    return;
                };
                let c = runner::config::for_main_class(&module.path, main_class);
                cfgs.push(c.clone());
                if let Err(e) = runner::config::save(&root, &cfgs) {
                    self.message = Some(format!("Không lưu được config: {e}"));
                }
                c
            }
        };
        let _ = runner::config::set_active(&root, &cfg.name);
        self.launch(cfg, debug, window, cx);
    }

    // ---- launch ----------------------------------------------------------------

    pub(crate) fn launch(&mut self, cfg: RunConfig, debug: bool, _window: &mut Window, cx: &mut Context<Self>) {
        // jdtls still importing/building: its classes would be missing or stale
        if self.defer_run(crate::lsp_workspace::PendingRun::Launch { cfg: cfg.clone(), debug }, cx) {
            return;
        }
        let Some(project) = self.project.clone() else {
            self.message = Some("Chưa có mô hình Maven".into());
            return;
        };
        let Some(module) = project.modules.iter().find(|m| m.path == cfg.module_path).cloned() else {
            self.message = Some(format!("Không thấy module {} trong model", cfg.module_path));
            return;
        };
        let kind = if debug { Kind::Debug } else { Kind::Run };
        let id = self.new_session(cfg.name.clone(), kind, Some(cfg.clone()));
        // a main() inside src/test/java needs the test classpath (IntelliJ does the same)
        let test_scope = runner::classpath::main_in_test_sources(&module, &cfg.main_class);
        let classpath = runner::classpath::classpath(&project, &module, test_scope);
        let sourcepaths = runner::classpath::sourcepaths(&project, &module, test_scope);
        let jdk = cfg.jdk_path.clone().or_else(|| runner::config::project_jdk(&self.root_str()));
        let launch_args = json!({
            "mainClass": cfg.main_class,
            "classPaths": classpath,
            "sourcePaths": sourcepaths,
            "vmArgs": cfg.vm_args,
            "args": cfg.program_args,
            "env": cfg.env_vars,
            "cwd": if cfg.working_directory.is_empty() { cfg.module_path.clone() } else { cfg.working_directory.clone() },
            "javaExec": jdk.as_ref().map(|j| toolchain::java_bin(j, "java").display().to_string()),
            "noDebug": !debug,
        });

        // compile first: jdtls' incremental build when it is up, otherwise `mvn compile`
        let build = self.ensure_built(&module, &cfg.maven_profiles, test_scope, vec![cfg.main_class.clone()], jdk, cx);
        self.start_dap(id, launch_args, build, cx);
    }

    /// Debugger jar + a JDK 17+ to run it.
    fn debugger_cmd(&mut self) -> Option<(PathBuf, PathBuf)> {
        let Some(jar) = toolchain::find_debugger() else {
            self.message = Some("Không thấy chodilamide-debugger.jar (chạy debugger/build.sh)".into());
            return None;
        };
        let jdks = toolchain::find_jdks();
        let Some(java) = jdks.iter().filter(|j| j.major >= 17).min_by_key(|j| j.major).map(|j| toolchain::java_bin(&j.path.display().to_string(), "java")) else {
            self.message = Some("Debugger cần JDK 17+ (Cài đặt → Java & Maven → Tải JDK)".into());
            return None;
        };
        Some((java, jar))
    }

    /// Build, then run `launch_args` through chodilamide-debugger in session `id`
    /// (Run = noDebug, Debug, and tests all share this path).
    fn start_dap(&mut self, id: usize, launch_args: Value, build: gpui::Task<anyhow::Result<()>>, cx: &mut Context<Self>) {
        self.start_dap_request(id, "launch", launch_args, build, cx)
    }

    /// Run → Attach to Process (IntelliJ "Remote JVM Debug"): a JVM started with
    /// -agentlib:jdwp=transport=dt_socket,server=y,address=5005 somewhere.
    pub(crate) fn attach_jvm(&mut self, target: &str, cx: &mut Context<Self>) -> Result<(), String> {
        let (host, port) = match target.rsplit_once(':') {
            Some((h, p)) => (if h.is_empty() { "localhost" } else { h }, p),
            None => ("localhost", target),
        };
        let port: u16 = port.trim().parse().map_err(|_| format!("Cổng không hợp lệ: {port}"))?;
        // every source root of the project, so frames of any module open in the editor
        let sources: Vec<String> = self.project.as_ref().map(|p| p.modules.iter().flat_map(|m| m.source_roots.iter().map(|s| s.path.clone())).collect()).unwrap_or_default();
        let id = self.new_session(format!("Attach {host}:{port}"), Kind::Debug, None);
        let args = json!({ "hostName": host, "port": port, "sourcePaths": sources, "timeout": 10_000 });
        self.start_dap_request(id, "attach", args, gpui::Task::ready(Ok(())), cx);
        Ok(())
    }

    fn start_dap_request(&mut self, id: usize, request: &'static str, launch_args: Value, build: gpui::Task<anyhow::Result<()>>, cx: &mut Context<Self>) {
        let Some((java, jar)) = self.debugger_cmd() else {
            if let Some(s) = self.session_mut(id) {
                s.state = State::Exited(None);
            }
            return;
        };
        cx.spawn(async move |this, cx| {
            if let Err(e) = build.await {
                let _ = this.update(cx, |ws, cx| {
                    if let Some(s) = ws.session_mut(id) {
                        s.push_output(&format!("Build lỗi: {e}\n"), true);
                        s.state = State::Exited(None);
                    }
                    cx.notify();
                });
                return;
            }
            let spawned = DapClient::spawn(&java, &["-jar".to_string(), jar.display().to_string()]);
            let (client, mut events) = match spawned {
                Ok(v) => v,
                Err(e) => {
                    let _ = this.update(cx, |ws, cx| {
                        if let Some(s) = ws.session_mut(id) {
                            s.push_output(&format!("{e:#}\n"), true);
                            s.state = State::Exited(None);
                        }
                        cx.notify();
                    });
                    return;
                }
            };
            let init = client.request("initialize", json!({ "adapterID": "java", "clientID": "chodilamide", "linesStartAt1": true, "columnsStartAt1": true, "pathFormat": "path" }));
            let launch = client.request(request, launch_args);
            let _ = this.update(cx, |ws, cx| {
                if let Some(s) = ws.session_mut(id) {
                    s.dap = Some(client.clone());
                    s.state = State::Running;
                }
                cx.notify();
            });
            let _ = init.response().await;
            // events that arrive meanwhile (`initialized`, output) wait in the channel
            if let Err(e) = launch.response().await {
                let _ = this.update(cx, |ws, cx| {
                    if let Some(s) = ws.session_mut(id) {
                        s.push_output(&format!("{} lỗi: {e}\n", if request == "attach" { "Attach" } else { "Launch" }), true);
                    }
                    cx.notify();
                });
            }
            while let Some(ev) = events.next().await {
                let closed = matches!(ev, DapEvent::Closed);
                if this.update(cx, |ws, cx| ws.on_dap_event(id, ev, cx)).is_err() || closed {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    /// jdtls `java/buildWorkspace` (fast, incremental) or `mvn -o compile` without jdtls.
    fn build_task(&mut self, module: &project_model::Module, profiles: &[String], tests: bool, cx: &mut Context<Self>) -> gpui::Task<anyhow::Result<()>> {
        if let (Some(client), crate::lsp_workspace::LspState::Ready) = (self.lsp.clone(), &self.lsp_state) {
            let req = client.request("java/buildWorkspace", json!(false));
            return cx.background_executor().spawn(async move {
                // 0 failed, 1 ok, 2 ok-with-errors (IntelliJ also launches then), 3 cancelled
                match req.response().await?.as_i64() {
                    Some(0) => anyhow::bail!("jdtls build thất bại"),
                    _ => Ok(()),
                }
            });
        }
        let goal = if tests { "test-compile" } else { "compile" };
        let cmd = runner::maven::command(&self.root, module, &[goal, "-o", "-q"], profiles, true, runner::config::project_jdk(&self.root_str()));
        cx.background_executor().spawn(async move {
            let mut c = Command::new(&cmd.program);
            c.args(&cmd.args).current_dir(&cmd.cwd);
            apply_java_home(&mut c, cmd.java_home.as_deref());
            let out = c.output()?;
            if !out.status.success() {
                anyhow::bail!("mvn compile lỗi:\n{}", String::from_utf8_lossy(&out.stdout).lines().filter(|l| l.contains("ERROR")).take(10).collect::<Vec<_>>().join("\n"));
            }
            Ok(())
        })
    }

    fn on_dap_event(&mut self, id: usize, ev: DapEvent, cx: &mut Context<Self>) {
        let Some(s) = self.session_mut(id) else { return };
        let client = s.dap.clone();
        match ev {
            DapEvent::Event { event, body } => match event.as_str() {
                "initialized" => {
                    if let Some(c) = client {
                        if s.debugging {
                            let paths: Vec<PathBuf> = self.breakpoints.keys().cloned().collect();
                            for path in paths {
                                c.request("setBreakpoints", self.bp_args(&path));
                            }
                            c.request("setExceptionBreakpoints", json!({ "filters": ["uncaught"] }));
                        }
                        c.request("configurationDone", json!({}));
                    }
                }
                "output" => {
                    let text = strip_ansi(body["output"].as_str().unwrap_or(""));
                    let err = body["category"] == "stderr";
                    if s.kind == Kind::Test && text.starts_with(runner::test::RESULT_PREFIX) {
                        // test-runner protocol line: goes to the tree, not the console
                        for line in text.lines() {
                            if let Some(r) = runner::test::parse_runner_line(line) {
                                s.test_results.push(r);
                            }
                        }
                    } else {
                        s.push_output(&text, err);
                    }
                }
                "stopped" => {
                    let thread = body["threadId"].as_i64().unwrap_or(0);
                    let reason = body["reason"].as_str().unwrap_or("").to_string();
                    let text = body["text"].as_str().map(String::from);
                    s.state = State::Stopped { thread, reason: text.map(|t| format!("{reason}: {t}")).unwrap_or(reason) };
                    s.vars.clear();
                    s.expanded.clear();
                    s.locals_ref = None;
                    self.active_session = Some(id);
                    self.panel_open = true;
                    self.fetch_stack(id, thread, cx);
                    self.fetch_threads(id, cx);
                }
                "continued" => {
                    s.state = State::Running;
                    self.clear_exec_lines(cx);
                }
                "exited" => {
                    let code = body["exitCode"].as_i64();
                    if s.kind == Kind::Test {
                        let failed = s.test_results.iter().filter(|r| matches!(r.status, runner::test::Status::Failed | runner::test::Status::Error)).count();
                        let n = s.test_results.len();
                        s.push_output(&format!("{n} test, {failed} lỗi"), failed > 0);
                    }
                    s.push_output(&format!("\nTiến trình kết thúc, mã thoát {}\n", code.map(|c| c.to_string()).unwrap_or("?".into())), false);
                    s.state = State::Exited(code);
                    if let (Some(exec), Some(module)) = (s.coverage_exec.take(), s.test_module.clone()) {
                        self.finish_coverage(id, exec, module, cx);
                    }
                }
                "terminated" => {
                    // a test run keeps going until Maven itself finishes
                    if s.is_live() && s.process.is_none() {
                        s.state = State::Exited(None);
                    } else if s.process.is_some() {
                        s.state = State::Running;
                    }
                    if let Some(c) = s.dap.take() {
                        c.kill();
                    }
                    self.clear_exec_lines(cx);
                }
                _ => {}
            },
            DapEvent::Closed => {
                if s.is_live() && s.process.is_none() {
                    s.state = State::Exited(None);
                }
                s.dap = None;
                self.clear_exec_lines(cx);
            }
        }
        cx.notify();
    }

    /// Threads panel: the other threads of the stopped VM (IntelliJ's thread combo).
    fn fetch_threads(&mut self, id: usize, cx: &mut Context<Self>) {
        let Some(client) = self.session_mut(id).and_then(|s| s.dap.clone()) else { return };
        let req = client.request("threads", json!({}));
        cx.spawn(async move |this, cx| {
            let res = req.response().await;
            let _ = this.update(cx, |ws, cx| {
                let mut threads: Vec<(i64, String)> = res
                    .ok()
                    .and_then(|b| b["threads"].as_array().cloned())
                    .unwrap_or_default()
                    .iter()
                    .map(|t| (t["id"].as_i64().unwrap_or(0), t["name"].as_str().unwrap_or("?").to_string()))
                    .collect();
                // "main" and application threads before JVM internals
                threads.sort_by_key(|(_, n)| (n != "main", n.starts_with("Reference") || n.starts_with("Finalizer") || n.starts_with("Signal") || n.starts_with("Common-Cleaner") || n.starts_with("Attach"), n.clone()));
                if let Some(s) = ws.session_mut(id) {
                    s.threads = threads;
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Clicking a thread in the Threads list: show its stack.
    pub(crate) fn select_thread(&mut self, id: usize, thread: i64, cx: &mut Context<Self>) {
        if let Some(s) = self.session_mut(id) {
            if let State::Stopped { thread: t, .. } = &mut s.state {
                *t = thread;
            }
        }
        self.fetch_stack(id, thread, cx);
    }

    /// ⏸: suspend the running VM (the adapter picks "main" when no thread is given).
    pub(crate) fn pause_session(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.active_session else { return };
        let Some(s) = self.session_mut(id) else { return };
        if !matches!(s.state, State::Running) {
            return;
        }
        if let Some(c) = s.dap.clone() {
            c.request("pause", json!({ "threadId": 0 }));
        }
        cx.notify();
    }

    fn fetch_stack(&mut self, id: usize, thread: i64, cx: &mut Context<Self>) {
        let Some(client) = self.session_mut(id).and_then(|s| s.dap.clone()) else { return };
        let req = client.request("stackTrace", json!({ "threadId": thread, "startFrame": 0, "levels": 100 }));
        cx.spawn(async move |this, cx| {
            let res = req.response().await;
            let _ = this.update(cx, |ws, cx| {
                let frames: Vec<Frame> = res
                    .ok()
                    .and_then(|b| b["stackFrames"].as_array().cloned())
                    .unwrap_or_default()
                    .iter()
                    .map(|f| Frame {
                        id: f["id"].as_i64().unwrap_or(0),
                        name: f["name"].as_str().unwrap_or("?").to_string(),
                        path: f["source"]["path"].as_str().map(PathBuf::from),
                        line: f["line"].as_u64().unwrap_or(0) as usize,
                    })
                    .collect();
                if let Some(s) = ws.session_mut(id) {
                    s.frames = frames;
                    // select the first frame with source (skip JDK internals)
                    s.selected_frame = s.frames.iter().position(|f| f.path.is_some()).unwrap_or(0);
                }
                ws.select_frame(id, None, cx);
            });
        })
        .detach();
    }

    /// Shows a frame: opens its file at the line and loads its locals.
    pub(crate) fn select_frame(&mut self, id: usize, index: Option<usize>, cx: &mut Context<Self>) {
        let Some(s) = self.session_mut(id) else { return };
        if let Some(i) = index {
            s.selected_frame = i;
        }
        let Some(frame) = s.frames.get(s.selected_frame).cloned() else { return };
        let client = s.dap.clone();
        s.vars.clear();
        s.expanded.clear();
        s.locals_ref = None;
        self.clear_exec_lines(cx);
        if let Some(path) = frame.path.clone() {
            self.pending_exec = Some((path, frame.line.saturating_sub(1)));
        }
        let Some(client) = client else { return };
        let req = client.request("scopes", json!({ "frameId": frame.id }));
        cx.spawn(async move |this, cx| {
            let scopes = req.response().await.ok();
            let r = scopes.as_ref().and_then(|b| b["scopes"][0]["variablesReference"].as_i64());
            let _ = this.update(cx, |ws, cx| {
                if let (Some(s), Some(r)) = (ws.session_mut(id), r) {
                    s.locals_ref = Some(r);
                    s.expanded.insert(r);
                }
                if let Some(r) = r {
                    ws.fetch_vars(id, r, cx);
                }
                ws.refresh_watches(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn fetch_vars(&mut self, id: usize, reference: i64, cx: &mut Context<Self>) {
        let Some(client) = self.session_mut(id).and_then(|s| s.dap.clone()) else { return };
        let req = client.request("variables", json!({ "variablesReference": reference }));
        cx.spawn(async move |this, cx| {
            let res = req.response().await;
            let _ = this.update(cx, |ws, cx| {
                let vars: Vec<Var> = res
                    .ok()
                    .and_then(|b| b["variables"].as_array().cloned())
                    .unwrap_or_default()
                    .iter()
                    .map(|v| Var {
                        name: v["name"].as_str().unwrap_or("").to_string(),
                        value: v["value"].as_str().unwrap_or("").to_string(),
                        reference: v["variablesReference"].as_i64().unwrap_or(0),
                    })
                    .collect();
                let is_locals = ws.session_mut(id).is_some_and(|s| {
                    s.vars.insert(reference, vars);
                    s.locals_ref == Some(reference)
                });
                if is_locals {
                    // the frame's locals arrived: grey "name: value" hints in the editor
                    ws.refresh_inline_values(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn toggle_var(&mut self, id: usize, reference: i64, cx: &mut Context<Self>) {
        let Some(s) = self.session_mut(id) else { return };
        if !s.expanded.remove(&reference) {
            s.expanded.insert(reference);
            if !s.vars.contains_key(&reference) {
                self.fetch_vars(id, reference, cx);
            }
        }
        cx.notify();
    }

    fn clear_exec_lines(&mut self, cx: &mut Context<Self>) {
        // running again: values shown in the editor are stale
        self.clear_inline_values(cx);
        for t in &self.tabs {
            if t.read(cx).exec_line.is_some() {
                t.update(cx, |v, cx| {
                    v.exec_line = None;
                    cx.notify()
                });
            }
        }
    }

    // ---- stepping ------------------------------------------------------------------

    /// continue / next / stepIn / stepOut on the stopped thread of the active session.
    pub(crate) fn debug_command(&mut self, command: &str, cx: &mut Context<Self>) {
        let Some(id) = self.active_session else { return };
        let Some(s) = self.session_mut(id) else { return };
        let State::Stopped { thread, .. } = s.state.clone() else { return };
        let Some(client) = s.dap.clone() else { return };
        s.state = State::Running;
        s.frames.clear();
        s.vars.clear();
        client.request(command, json!({ "threadId": thread }));
        self.clear_exec_lines(cx);
        cx.notify();
    }

    pub(crate) fn stop_session(&mut self, id: Option<usize>, cx: &mut Context<Self>) {
        // Stop also cancels a run still waiting for jdtls
        if id.is_none() {
            if let Some(r) = self.pending_run.take() {
                self.message = Some(format!("Đã hủy: {}", r.label()));
                cx.notify();
                return;
            }
        }
        let Some(id) = id.or(self.active_session) else { return };
        let Some(s) = self.session_mut(id) else { return };
        if let Some(c) = s.dap.clone() {
            c.request("disconnect", json!({ "terminateDebuggee": true }));
            // make sure nothing lingers if the adapter doesn't exit by itself
            let c2 = c.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(3));
                c2.kill();
            });
        }
        if let Some(p) = s.process.take() {
            let _ = p.lock().unwrap().kill();
        }
        if s.is_live() {
            s.push_output("\nĐã dừng.\n", false);
            s.state = State::Exited(None);
        }
        self.clear_exec_lines(cx);
        cx.notify();
    }

    /// Ctrl+F9: compile, then push changed classes into every debug session (hot swap).
    pub(crate) fn hot_swap(&mut self, cx: &mut Context<Self>) {
        // app debug sessions and unit-test debug sessions alike
        let targets: Vec<Arc<DapClient>> = self.sessions.iter().filter(|s| s.debugging && s.is_live()).filter_map(|s| s.dap.clone()).collect();
        let module = self
            .sessions
            .iter()
            .rev()
            .find(|s| s.debugging && s.is_live())
            .and_then(|s| s.config.as_ref().map(|c| c.module_path.clone()).or(s.test_module.clone()))
            .and_then(|m| self.project.as_ref().and_then(|p| p.modules.iter().find(|x| x.path == m).cloned()));
        let Some(module) = module else {
            self.message = Some("Không có phiên debug nào đang chạy".into());
            return;
        };
        let build = self.build_task(&module, &[], true, cx);
        self.message = Some("Đang build để hot swap…".into());
        cx.spawn(async move |this, cx| {
            let built = build.await;
            let mut changed = Vec::new();
            let mut err = built.err().map(|e| e.to_string());
            if err.is_none() {
                for c in targets {
                    match c.request("redefineClasses", json!({})).response().await {
                        Ok(b) => changed.extend(b["changedClasses"].as_array().cloned().unwrap_or_default()),
                        Err(e) => err = Some(e.to_string()),
                    }
                }
            }
            let _ = this.update(cx, |ws, cx| {
                ws.message = Some(match err {
                    Some(e) => format!("Hot swap lỗi: {e}"),
                    None if changed.is_empty() => "Hot swap: không có class nào thay đổi".into(),
                    None => format!("Hot swap: đã nạp lại {} class", changed.len()),
                });
                cx.notify();
            });
        })
        .detach();
    }

    // ---- breakpoints -----------------------------------------------------------------

    pub(crate) fn toggle_breakpoint(&mut self, path: PathBuf, line: usize, cx: &mut Context<Self>) {
        let lines = self.breakpoints.entry(path.clone()).or_default();
        if !lines.remove(&line) {
            lines.insert(line);
        } else if let Some(conds) = self.bp_conditions.get_mut(&path) {
            conds.remove(&line);
        }
        if self.breakpoints.get(&path).is_some_and(|l| l.is_empty()) {
            self.breakpoints.remove(&path);
        }
        self.breakpoints_changed(&path, cx);
    }

    /// Sets (or clears, when empty) the condition of the breakpoint on `line`, creating it if needed.
    pub(crate) fn set_breakpoint_condition(&mut self, path: PathBuf, line: usize, condition: &str, cx: &mut Context<Self>) {
        self.breakpoints.entry(path.clone()).or_default().insert(line);
        let conds = self.bp_conditions.entry(path.clone()).or_default();
        if condition.trim().is_empty() {
            conds.remove(&line);
        } else {
            conds.insert(line, condition.trim().to_string());
        }
        self.breakpoints_changed(&path, cx);
    }

    pub(crate) fn remove_breakpoint(&mut self, path: PathBuf, line: usize, cx: &mut Context<Self>) {
        if let Some(l) = self.breakpoints.get_mut(&path) {
            l.remove(&line);
        }
        if let Some(c) = self.bp_conditions.get_mut(&path) {
            c.remove(&line);
        }
        self.breakpoints_changed(&path, cx);
    }

    fn breakpoints_changed(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.sync_breakpoint_marks(cx);
        self.push_breakpoints(path);
        self.save_breakpoints();
        cx.notify();
    }

    /// Sends `path`'s breakpoints to every session with a debugger attached - app debug
    /// AND debug of unit tests (both are `debugging`).
    fn push_breakpoints(&self, path: &Path) {
        let args = self.bp_args(path);
        for s in &self.sessions {
            if let (true, Some(c)) = (s.debugging && s.is_live(), &s.dap) {
                c.request("setBreakpoints", args.clone());
            }
        }
    }

    fn bp_args(&self, path: &Path) -> Value {
        let lines = self.breakpoints.get(path).cloned().unwrap_or_default();
        let conds = self.bp_conditions.get(path);
        json!({
            "source": { "path": path.display().to_string() },
            "breakpoints": lines.iter().map(|l| match conds.and_then(|c| c.get(l)) {
                Some(cond) => json!({ "line": l + 1, "condition": cond }),
                None => json!({ "line": l + 1 }),
            }).collect::<Vec<_>>()
        })
    }

    pub(crate) fn sync_breakpoint_marks(&mut self, cx: &mut Context<Self>) {
        for t in &self.tabs {
            let Some(p) = t.read(cx).path().cloned() else { continue };
            let lines: Vec<usize> = self.breakpoints.get(&p).map(|s| s.iter().copied().collect()).unwrap_or_default();
            let conditional: Vec<usize> = self.bp_conditions.get(&p).map(|c| c.keys().copied().collect()).unwrap_or_default();
            if t.read(cx).breakpoints != lines || t.read(cx).conditional_breakpoints != conditional {
                t.update(cx, |v, cx| {
                    v.breakpoints = lines;
                    v.conditional_breakpoints = conditional;
                    cx.notify()
                });
            }
        }
    }

    /// Breakpoints persist per project (IDE-only file, not shared with nvim).
    fn breakpoints_file(&self) -> PathBuf {
        project_model::cache::state_dir(&self.root_str()).join("chodilamide-breakpoints.json")
    }

    fn save_breakpoints(&self) {
        let files: BTreeMap<String, Vec<Value>> = self
            .breakpoints
            .iter()
            .map(|(p, lines)| {
                let conds = self.bp_conditions.get(p);
                let list = lines
                    .iter()
                    .map(|l| match conds.and_then(|c| c.get(l)) {
                        Some(cond) => json!({ "line": l, "condition": cond }),
                        None => json!({ "line": l }),
                    })
                    .collect();
                (p.display().to_string(), list)
            })
            .collect();
        if let Ok(bytes) = serde_json::to_vec(&json!({ "version": 2, "files": files })) {
            let _ = project_model::cache::write_atomic(&self.breakpoints_file(), &bytes);
        }
    }

    pub(crate) fn load_breakpoints(&mut self) {
        let Ok(s) = std::fs::read_to_string(self.breakpoints_file()) else { return };
        let Ok(v) = serde_json::from_str::<Value>(&s) else { return };
        let (bps, conds) = parse_breakpoints(&v);
        self.breakpoints = bps;
        self.bp_conditions = conds;
    }

    // ---- Maven ---------------------------------------------------------------------

    pub(crate) fn run_maven(&mut self, module_path: &str, goals: Vec<String>, cx: &mut Context<Self>) {
        let Some(module) = self.project.as_ref().and_then(|p| p.modules.iter().find(|m| m.path == module_path)).cloned() else { return };
        let goal_refs: Vec<&str> = goals.iter().map(String::as_str).collect();
        // profiles from the Maven panel, else the module's run configuration
        let mut profiles = self.maven_profiles();
        if profiles.is_empty() {
            profiles = self.active_config().filter(|c| c.module_path == module_path).map(|c| c.maven_profiles).unwrap_or_default();
        }
        let cmd = runner::maven::command(&self.root, &module, &goal_refs, &profiles, self.skip_tests, runner::config::project_jdk(&self.root_str()));
        let title = format!("mvn {} ({})", goals.join(" "), module.artifact_id.clone().unwrap_or_default());
        let id = self.new_session(title, Kind::Maven, None);
        let tree_refresh = goals_touch_pom(&goals);
        self.spawn_process(id, cmd, cx, move |ws, _, _| {
            if tree_refresh {
                ws.tree.refresh();
            }
        });
    }

    /// A Maven command in its own console session (reactor phases, custom goals).
    pub(crate) fn run_command(&mut self, title: String, cmd: runner::maven::MavenCommand, cx: &mut Context<Self>) {
        let id = self.new_session(title, Kind::Maven, None);
        self.spawn_process(id, cmd, cx, |ws, _, _| ws.tree.refresh());
    }

    /// Runs a command in session `id`, streaming its output; `done` runs after it exits.
    fn spawn_process(&mut self, id: usize, cmd: runner::maven::MavenCommand, cx: &mut Context<Self>, done: impl FnOnce(&mut Self, usize, Option<i32>) + 'static) {
        let mut c = Command::new(&cmd.program);
        c.args(&cmd.args).current_dir(&cmd.cwd).stdout(Stdio::piped()).stderr(Stdio::piped());
        apply_java_home(&mut c, cmd.java_home.as_deref());
        let header = format!("$ {} {}   (trong {})\n", cmd.program, cmd.args.join(" "), cmd.cwd.display());
        if let Some(s) = self.session_mut(id) {
            s.push_output(&header, false);
            s.state = State::Running;
        }
        let mut child = match c.spawn() {
            Ok(ch) => ch,
            Err(e) => {
                if let Some(s) = self.session_mut(id) {
                    s.push_output(&format!("Không chạy được mvn: {e}\n"), true);
                    s.state = State::Exited(None);
                }
                return;
            }
        };
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<(String, bool)>();
        for (stream, err) in [(child.stdout.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>), false), (child.stderr.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>), true)] {
            let Some(stream) = stream else { continue };
            let tx = tx.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    let _ = tx.unbounded_send((line, err));
                }
            });
        }
        drop(tx);
        let child = Arc::new(Mutex::new(child));
        if let Some(s) = self.session_mut(id) {
            s.process = Some(child.clone());
        }
        cx.spawn(async move |this, cx| {
            while let Some((line, err)) = rx.next().await {
                if this.update(cx, |ws, cx| {
                    if let Some(s) = ws.session_mut(id) {
                        s.push_output(&format!("{}\n", strip_ansi(&line)), err);
                    }
                    cx.notify();
                }).is_err() {
                    return;
                }
            }
            let code = child.lock().unwrap().wait().ok().and_then(|s| s.code());
            let _ = this.update(cx, |ws, cx| {
                if let Some(s) = ws.session_mut(id) {
                    let ok = code == Some(0);
                    s.push_output(if ok { "\nBUILD OK\n" } else { "\nBUILD LỖI\n" }, !ok);
                    s.state = State::Exited(code.map(i64::from));
                    s.process = None;
                    if let Some(d) = s.dap.take() {
                        d.kill();
                    }
                }
                done(ws, id, code);
                // Maven may have changed jars (install), wiped target/ (clean) or regenerated
                // sources: have jdtls re-read every module and rebuild
                ws.notify_modules_changed(None);
                ws.request_jdtls_build();
                ws.clear_exec_lines(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    // ---- tests ----------------------------------------------------------------------

    /// ▶ on a test: JUnit runs inside a JVM launched by the debugger (no Maven), so Run and
    /// Debug of tests are as fast as running a main class.
    pub(crate) fn run_tests(&mut self, module_path: &str, targets: Vec<runner::test::TestTarget>, debug: bool, cx: &mut Context<Self>) {
        if self.defer_run(crate::lsp_workspace::PendingRun::Tests { module: module_path.to_string(), targets: targets.clone(), debug }, cx) {
            return;
        }
        let Some(project) = self.project.clone() else { return };
        let Some(module) = project.modules.iter().find(|m| m.path == module_path).cloned() else { return };
        let Some(runner_jar) = toolchain::find_testrunner() else {
            self.message = Some("Không thấy chodilamide-testrunner.jar (chạy debugger/build.sh)".into());
            return;
        };
        let label = targets
            .first()
            .map(|t| {
                let cls = t.class.rsplit('.').next().unwrap_or(&t.class).to_string();
                match t.methods.as_slice() {
                    [m] => format!("{cls}.{m}"),
                    [] => cls,
                    ms => format!("{cls} ({} test)", ms.len()),
                }
            })
            .unwrap_or_else(|| "test".into());
        let classpath = match runner::test::launch_classpath(&project, &module, &runner_jar) {
            Ok(cp) => cp,
            Err(e) => {
                self.message = Some(format!("{e}"));
                cx.notify();
                return;
            }
        };
        // same JDK / VM args / env as the module's run configuration, if it has one
        let root = self.root_str();
        let cfg = runner::config::load(&root).into_iter().find(|c| c.module_path == module_path);
        let id = self.new_session(label, Kind::Test, None);
        if let Some(s) = self.session_mut(id) {
            s.test_module = Some(module_path.to_string());
            s.test_targets = targets.clone();
            s.test_debug = debug;
            s.debugging = debug;
        }
        // the run configuration's JDK, else the project default (Settings → JDK mặc định)
        let cfg_jdk = cfg.as_ref().and_then(|c| c.jdk_path.clone()).or_else(|| runner::config::project_jdk(&self.root_str()));
        let mut vm_args = cfg.as_ref().map(|c| c.vm_args.clone()).unwrap_or_default();
        let coverage = std::mem::take(&mut self.coverage_next) && !debug;
        if coverage {
            let (exec, arg) = self.coverage_agent_arg(id);
            vm_args = format!("{vm_args} {arg}").trim().to_string();
            if let Some(s) = self.session_mut(id) {
                s.coverage_exec = Some(exec);
                s.name = format!("{} (coverage)", s.name);
            }
        }
        let launch_args = json!({
            "mainClass": "chodilamide.testrunner.Main",
            "classPaths": classpath,
            "sourcePaths": runner::classpath::sourcepaths(&project, &module, true),
            "vmArgs": vm_args,
            "args": runner::test::runner_args(&targets),
            "env": cfg.as_ref().map(|c| serde_json::to_value(&c.env_vars).unwrap_or(json!({}))).unwrap_or(json!({})),
            "cwd": module_path,
            "javaExec": cfg_jdk.as_ref().map(|j| toolchain::java_bin(j, "java").display().to_string()),
            "noDebug": !debug,
        });
        let profiles = cfg.map(|c| c.maven_profiles).unwrap_or_default();
        let classes: Vec<String> = targets.iter().map(|t| t.class.clone()).collect();
        let build = self.ensure_built(&module, &profiles, true, classes, cfg_jdk, cx);
        // coverage: the JaCoCo agent must be on disk before the test JVM starts
        let build = if coverage {
            cx.background_executor().spawn(async move {
                build.await?;
                toolchain::managed::ensure_jacoco()?;
                Ok(())
            })
        } else {
            build
        };
        self.start_dap(id, launch_args, build, cx);
        cx.notify();
    }

    pub(crate) fn rerun_failed(&mut self, id: usize, cx: &mut Context<Self>) {
        let Some(s) = self.sessions.iter().find(|s| s.id == id) else { return };
        let targets = runner::test::failed_targets(&s.test_results);
        let (Some(module), debug) = (s.test_module.clone(), s.test_debug) else { return };
        if targets.is_empty() {
            self.message = Some("Không có test lỗi".into());
            return;
        }
        self.run_tests(&module, targets, debug, cx);
    }

    pub(crate) fn shutdown_sessions(&mut self) {
        for s in &mut self.sessions {
            if let Some(c) = s.dap.take() {
                c.request("disconnect", json!({ "terminateDebuggee": true }));
                std::thread::sleep(std::time::Duration::from_millis(300));
                c.kill();
            }
            if let Some(p) = s.process.take() {
                let _ = p.lock().unwrap().kill();
            }
        }
    }
}

/// Jars the model lists that are not on disk (deleted from ~/.m2, or failed downloads).
pub fn missing_jars(p: &project_model::Project) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = p
        .modules
        .iter()
        .flat_map(|m| m.classpath_jars.clone().unwrap_or_default())
        .filter(|j| j.ends_with(".jar"))
        .map(PathBuf::from)
        .filter(|j| !j.exists())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// JAVA_HOME and PATH for the JDK Maven should use (like maven_jdk.lua's env_for).
pub fn apply_java_home(c: &mut Command, java_home: Option<&str>) {
    toolchain::managed::apply_java_env(c, java_home)
}

/// The toolbar's active entry.
pub enum RunTarget {
    Config(RunConfig),
    Test(runner::test_profiles::TestProfile),
}

fn goals_touch_pom(goals: &[String]) -> bool {
    goals.iter().any(|g| g == "clean")
}

type Breakpoints = (HashMap<PathBuf, BTreeSet<usize>>, HashMap<PathBuf, BTreeMap<usize, String>>);

/// Reads the breakpoint file: v2 `{"version":2,"files":{path:[{line,condition?}]}}`
/// or the first format `{path:[line,…]}`.
fn parse_breakpoints(v: &Value) -> Breakpoints {
    let mut bps: HashMap<PathBuf, BTreeSet<usize>> = HashMap::new();
    let mut conds: HashMap<PathBuf, BTreeMap<usize, String>> = HashMap::new();
    let files = if v["version"] == 2 { &v["files"] } else { v };
    for (path, list) in files.as_object().into_iter().flatten() {
        let p = PathBuf::from(path);
        for item in list.as_array().into_iter().flatten() {
            let line = item.as_u64().or_else(|| item["line"].as_u64());
            let Some(line) = line.map(|l| l as usize) else { continue };
            bps.entry(p.clone()).or_default().insert(line);
            if let Some(c) = item["condition"].as_str() {
                conds.entry(p.clone()).or_default().insert(line, c.to_string());
            }
        }
    }
    (bps, conds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_helpers() {
        assert_eq!(strip_ansi("\u{1b}[32mOK\u{1b}[0m done"), "OK done");
        assert_eq!(stack_link("\tat vn.demo.core.Greeter.greet(Greeter.java:13)"), Some(("vn/demo/core/Greeter.java".into(), 13)));
        assert_eq!(stack_link("at Main.main(Main.java:5)"), Some(("Main.java".into(), 5)));
        assert_eq!(stack_link("Caused by: x"), None);
    }

    #[test]
    fn missing_jars_are_listed_once() {
        let dir = std::env::temp_dir().join(format!("miss-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let here = dir.join("a-1.jar");
        std::fs::write(&here, "x").unwrap();
        let gone = dir.join("b-2.jar").display().to_string();
        let m = |jars: Vec<String>| project_model::Module {
            path: "/p".into(), group_id: None, artifact_id: None, version: None, packaging: "jar".into(), content_root: "/p".into(),
            source_roots: vec![], dependencies: vec![], profiles: vec![], in_reactor: false, classpath_jars: Some(jars),
        };
        let p = project_model::Project { root: "/p".into(), modules: vec![m(vec![here.display().to_string(), gone.clone()]), m(vec![gone.clone(), "/p/target/classes".into()])] };
        assert_eq!(missing_jars(&p), vec![PathBuf::from(&gone)]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn breakpoint_file_old_and_new_format() {
        let (b, c) = parse_breakpoints(&json!({ "/p/A.java": [3, 9] }));
        assert_eq!(b[&PathBuf::from("/p/A.java")].len(), 2);
        assert!(c.is_empty());
        let (b, c) = parse_breakpoints(&json!({ "version": 2, "files": { "/p/B.java": [{ "line": 4 }, { "line": 7, "condition": "name.equals(\"Lan\")" }] } }));
        assert_eq!(b[&PathBuf::from("/p/B.java")].iter().copied().collect::<Vec<_>>(), vec![4, 7]);
        assert_eq!(c[&PathBuf::from("/p/B.java")][&7], "name.equals(\"Lan\")");
    }
}
