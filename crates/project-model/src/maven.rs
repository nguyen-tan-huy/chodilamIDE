//! Builds a Project by shelling out to Maven - port of `resolver/maven.lua`.
//! Never hand-parse pom.xml for dependency/classpath info; ask `mvn` instead.
use crate::cache;
use crate::model::{Dependency, Library, Module, Project, SourceRoot};
use crate::pom::{self, ParsedPom};
use anyhow::{bail, Result};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

/// Each mvn is a full JVM; cap how many run at once (same as the Lua).
pub const MAX_CONCURRENT_MVN: usize = 4;

const SCAN_EXCLUDE_DIRS: &[&str] = &[
    "target", ".git", "node_modules", ".idea", ".mvn", "build", "dist", ".settings", "bin", "out", ".vscode",
];

#[derive(Debug, Clone, Default)]
pub struct BuildOpts {
    pub profiles: Vec<String>,
    pub offline: bool,
    pub force: bool,
    /// Modules added via :JavaModelAddModule (manifest `added`).
    pub manually_added: Vec<String>,
    pub quiet: bool,
}

pub fn normalize_root(root: &Path) -> String {
    let abs = std::path::absolute(root).unwrap_or_else(|_| root.to_path_buf());
    let s = abs.to_string_lossy().to_string();
    let trimmed = s.trim_end_matches('/');
    if trimmed.is_empty() { "/".into() } else { trimmed.into() }
}

/// Every directory under `dir` holding a pom.xml, skipping build/VCS/IDE dirs.
/// Symlinked directories are not followed (libuv's fs_scandir reports them as links).
pub fn scan_for_poms(dir: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![PathBuf::from(dir)];
    while let Some(d) = stack.pop() {
        if d.join("pom.xml").is_file() {
            found.push(d.to_string_lossy().trim_end_matches('/').to_string());
        }
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for entry in rd.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if ft.is_dir() && !SCAN_EXCLUDE_DIRS.contains(&name.as_ref()) {
                stack.push(entry.path());
            }
        }
    }
    found.sort();
    found
}

/// ./mvnw (when wrappers are on), else the app-selected Maven - downloaded when the machine
/// has none (Settings → Java & Maven → tự tải). Blocking: runs on the model-load thread.
pub fn mvn_cmd(dir: &str) -> Result<String, String> {
    let wrapper = Path::new(dir).join(if cfg!(windows) { "mvnw.cmd" } else { "mvnw" });
    if toolchain::managed::selection().use_wrapper && is_executable(&wrapper) {
        return Ok(wrapper.to_string_lossy().into());
    }
    toolchain::managed::ensure_maven().map(|p| p.display().to_string()).map_err(|e| format!("Không có Maven: {e:#}"))
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    p.is_file()
}

pub struct MvnOutput {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

pub fn run_maven(dir: &str, args: &[String], opts: &BuildOpts) -> MvnOutput {
    let fail = |stderr: String| MvnOutput { ok: false, stdout: String::new(), stderr };
    let cmd = match mvn_cmd(dir) {
        Ok(c) => c,
        Err(e) => return fail(e),
    };
    // no java on the machine at all → download one first
    let java = match toolchain::managed::ensure_any_java() {
        Ok(j) => j,
        Err(e) => return fail(format!("Không có JDK để chạy Maven: {e:#}")),
    };
    let mut c = Command::new(cmd);
    c.args(args).current_dir(dir);
    toolchain::managed::apply_java_env(&mut c, java.as_deref().and_then(|p| p.to_str()));
    if !opts.profiles.is_empty() {
        c.arg(format!("-P{}", opts.profiles.join(",")));
    }
    if opts.offline {
        c.arg("-o");
    }
    c.arg("-B");
    if !opts.quiet {
        eprintln!("mvn {} ({dir})", args.first().map(String::as_str).unwrap_or(""));
    }
    match c.output() {
        Ok(o) => MvnOutput {
            ok: o.status.success(),
            stdout: String::from_utf8_lossy(&o.stdout).into(),
            stderr: String::from_utf8_lossy(&o.stderr).into(),
        },
        Err(e) => MvnOutput { ok: false, stdout: String::new(), stderr: e.to_string() },
    }
}

/// Runs `f` over `items` with at most `limit` in flight; results keep input order.
fn run_limited<T: Sync, R: Send>(items: &[T], limit: usize, f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let next = Arc::new(Mutex::new(0usize));
    let results: Mutex<Vec<Option<R>>> = Mutex::new((0..items.len()).map(|_| None).collect());
    std::thread::scope(|s| {
        for _ in 0..limit.min(items.len()) {
            s.spawn(|| loop {
                let i = {
                    let mut n = next.lock().unwrap();
                    let i = *n;
                    *n += 1;
                    i
                };
                if i >= items.len() {
                    break;
                }
                let r = f(&items[i]);
                results.lock().unwrap()[i] = Some(r);
            });
        }
    });
    results.into_inner().unwrap().into_iter().map(|r| r.unwrap()).collect()
}

fn basename(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

pub fn build(root: &Path, opts: &BuildOpts) -> Result<Project> {
    let root = normalize_root(root);
    let key = cache::cache_key(&root, &opts.profiles);

    let mut all_pom_dirs = scan_for_poms(&root);
    for added in &opts.manually_added {
        let added = normalize_root(Path::new(added));
        if Path::new(&added).join("pom.xml").is_file() && !all_pom_dirs.contains(&added) {
            all_pom_dirs.push(added);
        }
    }
    let mtimes = cache::pom_mtimes(&all_pom_dirs);

    if !opts.force {
        if let Some(p) = cache::load_entry(&root, &key, &mtimes) {
            return Ok(p);
        }
    }
    if !opts.quiet {
        eprintln!("chodilamide: resolving Maven project (mvn)...");
    }

    let mut reactor_by_path: BTreeMap<String, ParsedPom> = BTreeMap::new();
    let mut in_reactor: HashMap<String, bool> = HashMap::new();

    // Root without pom.xml (axigen-service): every scanned pom is independent.
    if Path::new(&root).join("pom.xml").is_file() {
        let out = run_maven(&root, &["help:effective-pom".into()], opts);
        if !out.ok {
            bail!("mvn help:effective-pom failed: {}", failure_detail(&out.stderr, &out.stdout));
        }
        for parsed in pom::parse_effective_pom(&out.stdout) {
            let (Some(_), Some(aid)) = (&parsed.group_id, &parsed.artifact_id) else { continue };
            if parsed.packaging == "pom" {
                continue;
            }
            // effective-pom doesn't print each module's directory: match by artifactId.
            let dir = all_pom_dirs.iter().find(|d| basename(d) == aid).cloned().unwrap_or_else(|| root.clone());
            in_reactor.insert(dir.clone(), true);
            reactor_by_path.insert(dir, parsed);
        }
    }

    let independent: Vec<String> = all_pom_dirs
        .iter()
        .filter(|d| **d != root && !reactor_by_path.contains_key(*d))
        .cloned()
        .collect();
    let resolved = run_limited(&independent, MAX_CONCURRENT_MVN, |dir| {
        let out = run_maven(dir, &["help:effective-pom".into()], opts);
        if !out.ok {
            if !opts.quiet {
                eprintln!("chodilamide: skipped {dir}: effective-pom failed");
            }
            return None;
        }
        let blocks = pom::parse_effective_pom(&out.stdout);
        let own = basename(dir);
        let pick = blocks
            .iter()
            .find(|p| p.packaging != "pom" && p.artifact_id.as_deref() == Some(own))
            .cloned()
            .or_else(|| (blocks.len() == 1 && blocks[0].packaging != "pom").then(|| blocks[0].clone()));
        pick.map(|p| (dir.clone(), p))
    });
    for (dir, parsed) in resolved.into_iter().flatten() {
        reactor_by_path.insert(dir, parsed);
    }

    let mut project = Project { root: root.clone(), modules: Vec::new() };
    for (dir, parsed) in &reactor_by_path {
        project.modules.push(build_module(dir, parsed, &opts.profiles, in_reactor.get(dir).copied().unwrap_or(false)));
    }

    resolve_classpaths(&mut project, opts);
    mark_sibling_dependencies(&mut project);
    cache::save_entry(&root, &key, &mtimes, &project)?;
    Ok(project)
}

/// Maven prints errors to stdout, so stderr is usually empty: lead with the first
/// [ERROR] line so a one-line toast still shows the real cause.
fn failure_detail(stderr: &str, stdout: &str) -> String {
    let errs = tail(stdout);
    let errs = errs.trim_start();
    match (stderr.trim(), errs) {
        // no [ERROR] lines (e.g. mvn.cmd failing before Maven starts on Windows): show the tail
        ("", "") => {
            let last: Vec<&str> = stdout.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
            if last.is_empty() {
                "(mvn exited with no output)".into()
            } else {
                last[last.len().saturating_sub(5)..].join(" | ")
            }
        }
        ("", e) => e.into(),
        (s, "") => s.into(),
        (s, e) => format!("{e}\n{s}"),
    }
}

fn tail(s: &str) -> String {
    let lines: Vec<&str> = s.lines().filter(|l| l.contains("ERROR")).take(5).collect();
    if lines.is_empty() { String::new() } else { format!("\n{}", lines.join("\n")) }
}

pub fn build_module(dir: &str, parsed: &ParsedPom, profiles: &[String], in_reactor: bool) -> Module {
    let main_dir = parsed.source_dir.clone().unwrap_or_else(|| format!("{dir}/src/main/java"));
    let test_dir = parsed.test_source_dir.clone().unwrap_or_else(|| format!("{dir}/src/test/java"));
    let mut source_roots = Vec::new();
    for (path, kind) in [(main_dir, "main"), (test_dir, "test")] {
        if Path::new(&path).is_dir() {
            source_roots.push(SourceRoot { path, kind: kind.into(), lang: "java".into() });
        }
    }
    let dependencies = parsed
        .dependencies
        .iter()
        .map(|d| Dependency {
            group_id: d.group_id.clone(),
            artifact_id: d.artifact_id.clone(),
            version: d.version.clone(),
            scope: d.scope.clone(),
            is_sibling: false,
            sibling_module_path: None,
            library: None,
        })
        .collect();
    Module {
        path: dir.into(),
        group_id: parsed.group_id.clone(),
        artifact_id: parsed.artifact_id.clone(),
        version: parsed.version.clone(),
        packaging: parsed.packaging.clone(),
        content_root: dir.into(),
        source_roots,
        dependencies,
        profiles: profiles.to_vec(),
        in_reactor,
        classpath_jars: None,
    }
}

/// `mvn dependency:build-classpath` per module. Not scope-filtered on purpose
/// (provided deps must be on the launch classpath) - see the Lua comment.
pub fn resolve_classpaths(project: &mut Project, opts: &BuildOpts) {
    let paths: Vec<String> = project.modules.iter().map(|m| m.path.clone()).collect();
    let results = run_limited(&paths, MAX_CONCURRENT_MVN, |dir| module_classpath(dir, opts));
    for (m, jars) in project.modules.iter_mut().zip(results) {
        let Some(jars) = jars else { continue };
        for jar in &jars {
            let Some((artifact, version)) = pom::jar_artifact_version(jar) else { continue };
            for dep in m.dependencies.iter_mut() {
                if dep.library.is_none()
                    && !dep.is_sibling
                    && dep.artifact_id.as_deref() == Some(artifact)
                    && dep.version.as_deref() == Some(version)
                {
                    dep.library = Some(library_from_jar(jar, artifact, version));
                }
            }
        }
        m.classpath_jars = Some(jars);
    }
}

pub fn module_classpath(dir: &str, opts: &BuildOpts) -> Option<Vec<String>> {
    let outfile = std::env::temp_dir().join(format!("chodilamide-cp-{}-{}.txt", std::process::id(), hash(dir)));
    let args = vec!["dependency:build-classpath".to_string(), format!("-Dmdep.outputFile={}", outfile.display())];
    let out = run_maven(dir, &args, opts);
    let content = std::fs::read_to_string(&outfile).ok();
    let _ = std::fs::remove_file(&outfile);
    if !out.ok {
        return None;
    }
    content.map(|c| pom::parse_classpath(&c))
}

fn hash(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

fn library_from_jar(jar: &str, artifact: &str, version: &str) -> Library {
    let sources = format!("{}-sources.jar", jar.strip_suffix(".jar").unwrap_or(jar));
    Library {
        group_id: String::new(),
        artifact_id: artifact.into(),
        version: version.into(),
        jar_path: Some(jar.into()),
        sources_jar_path: Path::new(&sources).is_file().then_some(sources),
    }
}

/// Dependencies whose groupId:artifactId is another module become sibling references.
pub fn mark_sibling_dependencies(project: &mut Project) {
    let by_ga: HashMap<String, String> = project
        .modules
        .iter()
        .rev() // first module wins, like find_module_by_ga
        .map(|m| (m.ga(), m.path.clone()))
        .collect();
    for m in project.modules.iter_mut() {
        for dep in m.dependencies.iter_mut() {
            let (Some(g), Some(a)) = (&dep.group_id, &dep.artifact_id) else { continue };
            if let Some(path) = by_ga.get(&format!("{g}:{a}")) {
                if *path != m.path {
                    dep.is_sibling = true;
                    dep.sibling_module_path = Some(path.clone());
                    dep.library = None;
                }
            }
        }
    }
}
