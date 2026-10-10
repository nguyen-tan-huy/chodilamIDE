//! Maven command lines - port of maven_runner.lua: a reactor module runs from the root
//! with `-pl <module> -am`; an independent module (no aggregator) runs inside itself.
use project_model::Module;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct MavenCommand {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub java_home: Option<String>,
}

/// Maven used when there is no wrapper: Settings → Java & Maven, `mvn` on PATH, or the app's own.
fn mvn() -> String {
    toolchain::managed::mvn_program().map(|p| p.display().to_string()).unwrap_or_else(|| "mvn".into())
}

/// How to start Maven in `dir`: the project's wrapper when there is one (and wrappers are on
/// in Settings), the selected Maven otherwise.
/// A wrapper without the executable bit (common after a zip/Windows checkout) runs through
/// `sh mvnw` instead of failing with "Permission denied"; its mode is left untouched.
pub fn launcher(dir: &Path, args: Vec<String>) -> (String, Vec<String>) {
    let use_wrapper = toolchain::managed::selection().use_wrapper;
    if cfg!(windows) {
        let w = dir.join("mvnw.cmd");
        return if use_wrapper && w.is_file() { (w.display().to_string(), args) } else { (mvn(), args) };
    }
    let w = dir.join("mvnw");
    if !use_wrapper || !w.is_file() {
        return (mvn(), args);
    }
    if is_executable(&w) {
        return (w.display().to_string(), args);
    }
    let mut with_script = vec![w.display().to_string()];
    with_script.extend(args);
    ("sh".into(), with_script)
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).map(|m| m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(_: &Path) -> bool {
    true
}

fn build(dir: &Path, args: Vec<String>, java_home: Option<String>) -> MavenCommand {
    let (program, args) = launcher(dir, args);
    MavenCommand { program, args, cwd: dir.to_path_buf(), java_home }
}

pub fn command(root: &Path, module: &Module, goals: &[&str], profiles: &[String], skip_tests: bool, java_home: Option<String>) -> MavenCommand {
    let module_dir = PathBuf::from(&module.path);
    let (cwd, mut args) = if module.in_reactor && module_dir != root {
        let rel = module_dir.strip_prefix(root).unwrap_or(&module_dir).display().to_string();
        (root.to_path_buf(), vec!["-pl".to_string(), rel, "-am".into()])
    } else {
        (module_dir.clone(), Vec::new())
    };
    let mut all: Vec<String> = goals.iter().map(|g| g.to_string()).collect();
    all.append(&mut args);
    if !profiles.is_empty() {
        all.push(format!("-P{}", profiles.join(",")));
    }
    if skip_tests {
        all.push("-DskipTests".into());
    }
    all.push("-B".into());
    build(&cwd, all, java_home)
}

/// Phases shown in the Maven tool window (same list as java-debug-model's maven_panel).
pub const LIFECYCLE: &[&str] = &["clean", "validate", "compile", "test", "package", "verify", "install", "site", "deploy"];

/// A goal for the whole reactor (root pom, no -pl), like the panel's "Lifecycle (toàn bộ reactor)".
pub fn reactor_command(root: &Path, goals: &[&str], profiles: &[String], skip_tests: bool, java_home: Option<String>) -> MavenCommand {
    let mut args: Vec<String> = goals.iter().map(|g| g.to_string()).collect();
    if !profiles.is_empty() {
        args.push(format!("-P{}", profiles.join(",")));
    }
    if skip_tests {
        args.push("-DskipTests".into());
    }
    args.push("-B".into());
    build(root, args, java_home)
}

/// Splits a user-typed goal line ("dependency:analyze -DskipTests") into args.
pub fn split_goal_line(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote = None;
    for c in line.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => quote = Some(c),
            (None, c) if c.is_whitespace() => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            (None, c) => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// `mvn dependency:tree -Dverbose -DoutputFile=…` inside the module (verbose keeps the
/// "omitted for conflict" lines that explain which version won).
pub fn dependency_tree_command(module: &Module, profiles: &[String], java_home: Option<String>, outfile: &Path) -> MavenCommand {
    let dir = PathBuf::from(&module.path);
    let mut args = vec!["dependency:tree".to_string(), "-Dverbose".into(), format!("-DoutputFile={}", outfile.display())];
    if !profiles.is_empty() {
        args.push(format!("-P{}", profiles.join(",")));
    }
    args.push("-B".into());
    build(&dir, args, java_home)
}

/// Depth of a dependency:tree line ("|  +- g:a:jar:1:compile" → 2); the root is 0.
pub fn tree_depth(line: &str) -> usize {
    let mut depth = 0;
    let mut rest = line;
    while rest.starts_with("|  ") || rest.starts_with("   ") {
        rest = &rest[3..];
        depth += 1;
    }
    if rest.starts_with("+- ") || rest.starts_with("\\- ") {
        depth += 1;
    }
    depth
}

/// Lines matching `query` plus every ancestor, so a match is shown with the path that
/// pulled it in (port of dependency_tree.lua's filtered_lines). Returns (lines, match count).
pub fn filter_tree<'a>(lines: &'a [String], query: &str) -> (Vec<&'a String>, usize) {
    let q = query.to_lowercase();
    if q.is_empty() {
        return (lines.iter().collect(), 0);
    }
    let mut keep = vec![false; lines.len()];
    let mut last_at_depth: Vec<Option<usize>> = Vec::new();
    let mut matches = 0;
    for (i, line) in lines.iter().enumerate() {
        let d = tree_depth(line);
        if last_at_depth.len() <= d {
            last_at_depth.resize(d + 1, None);
        }
        last_at_depth[d] = Some(i);
        last_at_depth.truncate(d + 1);
        if line.to_lowercase().contains(&q) {
            matches += 1;
            keep[i] = true;
            for a in last_at_depth[..d].iter().flatten() {
                keep[*a] = true;
            }
        }
    }
    (lines.iter().zip(keep).filter(|(_, k)| *k).map(|(l, _)| l).collect(), matches)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TreeMark {
    Normal,
    /// "omitted for conflict with X" - a version lost mediation
    Conflict,
    /// "omitted for duplicate"
    Duplicate,
}

pub fn tree_mark(line: &str) -> TreeMark {
    if line.contains("omitted for conflict") {
        TreeMark::Conflict
    } else if line.contains("omitted for duplicate") {
        TreeMark::Duplicate
    } else {
        TreeMark::Normal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TREE: &str = "net.lvs:product-web:jar:1.0.0
+- org.springframework.boot:spring-boot-starter-web:jar:2.7.12:compile
|  +- org.springframework.boot:spring-boot-starter-json:jar:2.7.12:compile
|  |  \\- com.fasterxml.jackson.core:jackson-databind:jar:2.13.5:compile
|  \\- (commons-io:commons-io:jar:2.6:compile - omitted for conflict with 2.11.0)
+- commons-io:commons-io:jar:2.11.0:compile
\\- net.lvs:product-core:jar:4.1.3-prod:compile";

    #[test]
    fn dependency_tree_depth_filter_marks() {
        let lines: Vec<String> = TREE.lines().map(String::from).collect();
        assert_eq!(lines.iter().map(|l| tree_depth(l)).collect::<Vec<_>>(), vec![0, 1, 2, 3, 2, 1, 1]);
        let (kept, n) = filter_tree(&lines, "jackson");
        assert_eq!(n, 1);
        // the match plus its ancestors: root, starter-web, starter-json
        assert_eq!(kept.len(), 4);
        assert!(kept[1].contains("starter-web") && kept[2].contains("starter-json"));
        let (kept, n) = filter_tree(&lines, "COMMONS-IO");
        assert_eq!((n, kept.len()), (2, 4));
        assert_eq!(tree_mark(&lines[4]), TreeMark::Conflict);
        assert_eq!(tree_mark(&lines[5]), TreeMark::Normal);
    }

    /// storefront-service's mvnw had no executable bit → "Permission denied".
    #[cfg(unix)]
    #[test]
    fn wrapper_without_exec_bit_runs_through_sh() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("mvnw-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let args = vec!["clean".to_string()];
        assert_eq!(launcher(&dir, args.clone()), (mvn(), args.clone()));
        let w = dir.join("mvnw");
        std::fs::write(&w, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&w, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(launcher(&dir, args.clone()), ("sh".to_string(), vec![w.display().to_string(), "clean".into()]));
        std::fs::set_permissions(&w, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(launcher(&dir, args.clone()), (w.display().to_string(), args));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn goal_line_and_reactor() {
        assert_eq!(split_goal_line(r#"versions:display-dependency-updates -Dincludes="org.spring*" "#), vec!["versions:display-dependency-updates", "-Dincludes=org.spring*"]);
        let c = reactor_command(Path::new("/p"), &["clean", "install"], &["dev".into()], true, None);
        assert_eq!(c.args, vec!["clean", "install", "-Pdev", "-DskipTests", "-B"]);
    }
}
