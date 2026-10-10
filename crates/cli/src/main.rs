mod jsf_check;
mod lsp_check;

use anyhow::{bail, Context, Result};
use project_model::{open_project, BuildOpts, Project};
use std::path::PathBuf;

const USAGE: &str = "usage:
  chodilamide model inspect <dir> [--profiles a,b] [--json] [--force] [--offline]
  chodilamide model classpath <dir> <module> [--profiles a,b] [--offline]
  chodilamide lsp check <dir> <file> [line col]   (1-based line, 0-based col)
  chodilamide run list <dir>
  chodilamide run show <dir> <config-name>        (DAP launch arguments, read-only)
  chodilamide test <dir> <module-dir> <Class[#method]> [--rerun-failed]   (qua Maven Surefire)
  chodilamide test-cp <dir> <module-dir>          (classpath chạy test nhanh, chỉ đọc)
  chodilamide spring <dir> [key]                  (endpoints, beans, profiles, metadata; key: nơi đặt + chỗ dùng)
  chodilamide doctor
  chodilamide tools install [--jdk]               (jdtls, lombok, JDK 21 vào ~/.local/share/chodilamide/tools)";

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

struct Args {
    positional: Vec<String>,
    opts: BuildOpts,
    json: bool,
    rerun: bool,
    jdk: bool,
}

fn parse_args() -> Result<Args> {
    let mut a = Args { positional: Vec::new(), opts: BuildOpts::default(), json: false, rerun: false, jdk: false };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--json" => a.json = true,
            "--rerun-failed" => a.rerun = true,
            "--jdk" => a.jdk = true,
            "--force" => a.opts.force = true,
            "--offline" => a.opts.offline = true,
            "--profiles" => {
                let v = it.next().context("--profiles needs a value")?;
                a.opts.profiles = v.split(',').filter(|s| !s.is_empty()).map(String::from).collect();
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            s if s.starts_with("--") => bail!("unknown flag {s}\n{USAGE}"),
            _ => a.positional.push(arg),
        }
    }
    Ok(a)
}

fn run() -> Result<()> {
    let mut args = parse_args()?;
    let pos: Vec<&str> = args.positional.iter().map(String::as_str).collect();
    match pos.as_slice() {
        ["spring", dir, rest @ ..] => {
            args.opts.quiet = true;
            let p = open_project(&PathBuf::from(dir), args.opts)?;
            let t = std::time::Instant::now();
            let eps = spring::endpoints::endpoints(&p);
            println!("Endpoints ({}, {:?}):", eps.len(), t.elapsed());
            for e in eps.iter().take(40) {
                println!("  {:<7}{:<50} {}.{}  {}:{}", e.method, e.path, e.class, e.handler, e.file.display(), e.line + 1);
            }
            let t = std::time::Instant::now();
            let beans = spring::beans::beans(&p);
            println!("Beans ({}, {:?}): {}", beans.len(), t.elapsed(), beans.iter().take(15).map(|b| format!("{}:{}", b.kind, b.name)).collect::<Vec<_>>().join(", "));
            println!("Profiles: {:?}", spring::config::profiles(&p));
            println!("Config files: {:?}", spring::config::config_files(&p));
            let t = std::time::Instant::now();
            let meta = spring::metadata::load(&p);
            println!("Metadata: {} keys ({:?}); server.port = {:?}", meta.len(), t.elapsed(), meta.get("server.port").map(|m| &m.description));
            if let [key] = rest {
                for l in spring::config::definitions(&p, key) {
                    println!("  đặt  {}:{}  {}", l.path.display(), l.line + 1, l.text);
                }
                for l in spring::config::java_usages(&p, key) {
                    println!("  dùng {}:{}  {}", l.path.display(), l.line + 1, l.text);
                }
            }
        }
        ["model", "inspect", dir] => {
            args.opts.quiet = args.json;
            let p = open_project(&PathBuf::from(dir), args.opts)?;
            if args.json {
                println!("{}", serde_json::to_string_pretty(&p)?);
            } else {
                print_inspect(&p);
            }
        }
        ["model", "classpath", dir, module] => {
            args.opts.quiet = true;
            let p = open_project(&PathBuf::from(dir), args.opts)?;
            let m = p
                .modules
                .iter()
                .find(|m| m.artifact_id.as_deref() == Some(*module) || m.path.ends_with(&format!("/{module}")))
                .with_context(|| format!("module {module} not found"))?;
            let jars = m.classpath_jars.as_deref().unwrap_or_default();
            println!("{}", jars.join(":"));
        }
        ["lsp", "complete", dir, file, line, col] => lsp_check::complete(&PathBuf::from(dir), &PathBuf::from(file), line.parse()?, col.parse()?)?,
        ["jsf", "check", dir, file] => jsf_check::check(&PathBuf::from(dir), &PathBuf::from(file))?,
        ["lsp", "refs", dir, file, line, col] => lsp_check::refs(&PathBuf::from(dir), &PathBuf::from(file), line.parse()?, col.parse()?)?,
        ["lsp", "progress", dir] => lsp_check::progress(&PathBuf::from(dir))?,
        ["lsp", "actions", dir, file, l1, c1, l2, c2, rest @ ..] => {
            lsp_check::actions(&PathBuf::from(dir), &PathBuf::from(file), [l1.parse()?, c1.parse()?, l2.parse()?, c2.parse()?], rest.first().copied())?
        }
        ["lsp", "watch-check", dir] => lsp_check::watch_check(&PathBuf::from(dir))?,
        ["lsp", "check", dir, file] => lsp_check::run(&PathBuf::from(dir), &PathBuf::from(file), None)?,
        ["lsp", "check", dir, file, line, col] => {
            let line: u32 = line.parse().context("line")?;
            lsp_check::run(&PathBuf::from(dir), &PathBuf::from(file), Some((line.saturating_sub(1), col.parse().context("col")?)))?
        }
        ["run", "list", dir] => {
            let root = project_model::maven::normalize_root(&PathBuf::from(dir));
            let active = runner::config::active(&root);
            for c in runner::config::load(&root) {
                let mark = if active.as_deref() == Some(c.name.as_str()) { "*" } else { " " };
                println!("{mark} {:28} {}  jdk={}  profiles={:?}", c.name, c.main_class, c.jdk_path.as_deref().unwrap_or("-"), c.maven_profiles);
            }
        }
        ["run", "show", dir, name] => {
            args.opts.quiet = true;
            let root = project_model::maven::normalize_root(&PathBuf::from(dir));
            let cfg = runner::config::load(&root).into_iter().find(|c| c.name == *name).with_context(|| format!("không có config {name}"))?;
            let p = open_project(&PathBuf::from(dir), args.opts)?;
            let m = p.modules.iter().find(|m| m.path == cfg.module_path).context("module của config không có trong model")?;
            let cp = runner::classpath::classpath(&p, m, false);
            let sp = runner::classpath::sourcepaths(&p, m, false);
            println!("mainClass: {}\njavaExec:  {}\nvmArgs:    {}\ncwd:       {}", cfg.main_class,
                cfg.jdk_path.as_deref().map(|j| toolchain::java_bin(j, "java").display().to_string()).unwrap_or("(JDK mặc định)".into()), cfg.vm_args, cfg.working_directory);
            println!("classPaths: {} mục; thư mục class: {:?}", cp.len(), cp.iter().filter(|c| !c.ends_with(".jar")).collect::<Vec<_>>());
            println!("sourcePaths: {:?}", sp);
        }
        ["compile", dir, module_dir, class] => {
            // the IDE's targeted compile (javac on just this class + what it references)
            args.opts.quiet = true;
            let p = open_project(&PathBuf::from(dir), args.opts)?;
            let md = project_model::maven::normalize_root(&PathBuf::from(module_dir));
            let m = p.modules.iter().find(|m| m.path == md).context("không thấy module")?;
            let test = runner::classpath::main_in_test_sources(m, class);
            let cp = runner::classpath::classpath(&p, m, true);
            let sp = runner::classpath::sourcepaths(&p, m, true);
            let file = runner::compile::source_file(&sp, class).context("không thấy file nguồn")?;
            let out = PathBuf::from(&m.path).join(if test { "target/test-classes" } else { "target/classes" });
            let jdk = runner::config::maven_jdk(&p.root);
            let t = std::time::Instant::now();
            let o = runner::compile::javac(jdk.as_deref(), &cp, &sp, &out, &[file.clone()], &std::env::temp_dir().join("chodilamide-javac-cli.txt"))?.output()?;
            println!("javac {} (JDK {}): exit {:?} trong {:.1}s", file.display(), jdk.as_deref().unwrap_or("PATH"), o.status.code(), t.elapsed().as_secs_f32());
            for l in String::from_utf8_lossy(&o.stderr).lines().take(6) { println!("  {l}"); }
            println!("class file có: {}", runner::test_profiles::class_file_exists(std::path::Path::new(&m.path), class, test));
        }
        ["doctor"] => {
            let checks = toolchain::install::doctor();
            for c in &checks {
                println!("{} {:30} {}", if c.ok { "✓" } else { "✗" }, c.name, c.detail);
                if !c.ok && !c.fix.is_empty() {
                    println!("  → {}", c.fix);
                }
            }
            let bad = checks.iter().filter(|c| !c.ok && c.name != "ghi chú").count();
            println!("{}", if bad == 0 { "Mọi thứ sẵn sàng.".to_string() } else { format!("{bad} mục cần xử lý.") });
        }
        ["tools", "install"] => {
            let dir = toolchain::tools_dir();
            println!("Cài vào {}", dir.display());
            for line in toolchain::install::install(&dir, args.jdk, &mut |m| println!("{m}"))? {
                println!("✓ {line}");
            }
        }
        ["test-cp", dir, module_dir] => {
            args.opts.quiet = true;
            let p = open_project(&PathBuf::from(dir), args.opts)?;
            let md = project_model::maven::normalize_root(&PathBuf::from(module_dir));
            let m = p.modules.iter().find(|m| m.path == md).context("không thấy module")?;
            let jar = toolchain::find_testrunner().context("không thấy chodilamide-testrunner.jar")?;
            let cp = runner::test::launch_classpath(&p, m, &jar)?;
            println!("{} mục", cp.len());
            for c in cp.iter().filter(|c| !c.ends_with(".jar") || c.contains("junit-platform") || c.contains("testrunner")) {
                println!("  {c}");
            }
        }
        ["test", dir, module_dir, target] => {
            args.opts.quiet = true;
            let root = PathBuf::from(project_model::maven::normalize_root(&PathBuf::from(dir)));
            let p = open_project(&root, args.opts)?;
            let md = project_model::maven::normalize_root(&PathBuf::from(module_dir));
            let m = p.modules.iter().find(|m| m.path == md).with_context(|| format!("không thấy module {md}"))?;
            let (class, method) = target.split_once('#').map(|(c, m)| (c, Some(m))).unwrap_or((target, None));
            let mut targets = vec![runner::test::TestTarget { class: class.into(), methods: method.map(|m| vec![m.to_string()]).unwrap_or_default() }];
            for round in 0..(if args.rerun { 2 } else { 1 }) {
                let cmd = runner::test::mvn_test(&root, m, &targets, &[], runner::config::project_jdk(&root.display().to_string()), None);
                println!("$ {} {}", cmd.program, cmd.args.join(" "));
                let since = std::time::SystemTime::now() - std::time::Duration::from_secs(1);
                let t = std::time::Instant::now();
                let mut c = std::process::Command::new(&cmd.program);
                // JAVA_HOME of the project / app JDK, like the IDE does (a machine without a
                // system JDK only has the one the app downloaded)
                toolchain::managed::apply_java_env(&mut c, cmd.java_home.as_deref());
                let st = c.args(&cmd.args).arg("-q").current_dir(&cmd.cwd).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status()?;
                let results = runner::test::collect_results(std::path::Path::new(&m.path), since);
                println!("vòng {}: mvn exit {:?}, {:.1}s, {} test", round + 1, st.code(), t.elapsed().as_secs_f32(), results.len());
                for r in &results {
                    println!("  {:?} {}.{} {}", r.status, r.class.rsplit('.').next().unwrap_or(""), r.name, r.message);
                }
                targets = runner::test::failed_targets(&results);
                if targets.is_empty() { break; }
                println!("  → chạy lại: -Dtest={}", runner::test::test_filter(&targets));
            }
        }
        _ => bail!("{USAGE}"),
    }
    Ok(())
}

/// Same layout as :JavaModelInspect.
fn print_inspect(p: &Project) {
    println!("Project: {}", p.root);
    for m in &p.modules {
        let tag = if m.in_reactor { "" } else { " [independent]" };
        println!("  {} ({}){tag}", m.coordinates(), m.packaging);
        println!("    path: {}", m.path);
        for sr in &m.source_roots {
            println!("    src[{}]: {}", sr.kind, sr.path);
        }
        let siblings: Vec<_> = m
            .dependencies
            .iter()
            .filter(|d| d.is_sibling)
            .map(|d| d.artifact_id.as_deref().unwrap_or("?"))
            .collect();
        if !siblings.is_empty() {
            println!("    sibling deps: {}", siblings.join(", "));
        }
        println!(
            "    deps: {}  classpath jars: {}",
            m.dependencies.len(),
            m.classpath_jars.as_ref().map_or(0, Vec::len)
        );
    }
}
