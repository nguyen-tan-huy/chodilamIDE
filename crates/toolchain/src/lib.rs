//! Finds the Java tooling the IDE needs: the patched jdtls 1.54.0, java-debug and
//! java-test bundles, lombok, the MavenProjectImporter patch agent, and installed JDKs.
//! Port of bootstrap.lua's lookup logic (downloading comes later - for now it reuses
//! what java-debug-model/nvim-java already installed).
pub mod install;
pub mod managed;

use serde::Serialize;
use std::path::{Path, PathBuf};

pub(crate) fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).unwrap_or_default())
}

/// `p` without Windows' verbatim prefix (`\\?\C:\…` → `C:\…`, `\\?\UNC\s\x` → `\\s\x`):
/// std::fs::canonicalize adds it on Windows, and with it Windows stops accepting `/` in the
/// rest of the path (a saved JDK path + "/bin/java" then can't be run). Also cleans paths
/// saved by older versions.
pub fn plain_path(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    match s.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => p.to_path_buf(),
    }
}

/// std::fs::canonicalize without the verbatim prefix.
pub fn canonical(p: &Path) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(p).map(|c| plain_path(&c))
}

/// `<jdk>/bin/<tool>` (java, javac…) with the platform's separators and `.exe`.
pub fn java_bin(jdk_home: &str, tool: &str) -> PathBuf {
    let home = plain_path(Path::new(jdk_home.trim_end_matches(['/', '\\'])));
    home.join("bin").join(if cfg!(windows) { format!("{tool}.exe") } else { tool.to_string() })
}

/// Where `tools install` puts things: ~/.local/share/chodilamide/tools
pub fn tools_dir() -> PathBuf {
    data_dirs()[0].join("tools")
}

fn data_dirs() -> Vec<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".local/share"));
    // our own dir first, then the nvim app dirs that already have the tools
    ["chodilamide", "chodilamide-nvim", "nvim"].iter().map(|d| base.join(d)).collect()
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Tools {
    pub jdtls: Option<PathBuf>,
    pub lombok: Option<PathBuf>,
    pub patch_agent: Option<PathBuf>,
    pub bundles: Vec<PathBuf>,
    /// Warnings worth showing (e.g. falling back to a newer, possibly incompatible jdtls).
    pub notes: Vec<String>,
}

fn first_existing(cands: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    cands.into_iter().find(|p| p.exists())
}

fn jars_in(dir: &Path, prefix: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    let n = p.file_name().unwrap_or_default().to_string_lossy();
                    n.ends_with(".jar") && n.starts_with(prefix)
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

pub fn find_tools() -> Tools {
    let mut t = Tools::default();
    let dirs = data_dirs();

    t.jdtls = first_existing(dirs.iter().map(|d| d.join("nvim-java/packages/jdtls/1.54.0")).chain(dirs.iter().map(|d| d.join("tools/jdtls"))))
        .filter(|p| p.join("bin/jdtls").exists());
    if t.jdtls.is_none() {
        t.jdtls = first_existing(dirs.iter().map(|d| d.join("mason/packages/jdtls"))).filter(|p| p.join("bin/jdtls").exists());
        if t.jdtls.is_some() {
            t.notes.push("không thấy jdtls 1.54.0 đã vá, dùng bản Mason - debug/test có thể lỗi ASM".into());
        }
    }

    // java-debug: pinned nvim-java copy, else Mason
    let debug_dirs = dirs
        .iter()
        .flat_map(|d| {
            let pinned = std::fs::read_dir(d.join("nvim-java/packages/java-debug"))
                .map(|rd| rd.flatten().map(|e| e.path().join("extension/server")).collect::<Vec<_>>())
                .unwrap_or_default();
            pinned.into_iter().chain([d.join("mason/packages/java-debug-adapter/extension/server")])
        })
        .collect::<Vec<_>>();
    if let Some(dir) = debug_dirs.iter().find(|d| !jars_in(d, "com.microsoft.java.debug.plugin-").is_empty()) {
        t.bundles.extend(jars_in(dir, "com.microsoft.java.debug.plugin-"));
    }
    // java-test 0.43.2 matches jdtls 1.54.0's ASM; newer Mason builds break hasJUnit6TestAnnotation
    let test_dir = first_existing(
        dirs.iter()
            .map(|d| d.join("nvim-java/packages/java-test/0.43.2/extension/server"))
            .chain(dirs.iter().map(|d| d.join("mason/packages/java-test/extension/server"))),
    );
    if let Some(dir) = test_dir {
        t.bundles.extend(jars_in(&dir, ""));
    }

    let mut lombok_cands: Vec<PathBuf> = Vec::new();
    if let Some(j) = &t.jdtls {
        lombok_cands.push(j.join("lombok.jar"));
    }
    for d in &dirs {
        lombok_cands.push(d.join("nvim-java/packages/lombok/1.18.42/lombok-1.18.42.jar"));
        lombok_cands.push(d.join("mason/packages/jdtls/lombok.jar"));
        lombok_cands.push(d.join("java-debug-model/lombok.jar"));
    }
    lombok_cands.push(tools_dir().join("lombok.jar"));
    t.lombok = first_existing(lombok_cands).or_else(newest_m2_lombok);

    t.patch_agent = first_existing([
        dirs[0].join("tools/jdtls-maven-importer-patch-agent.jar"),
        // shipped in the package next to the debugger
        find_debugger().map(|d| d.with_file_name("jdtls-maven-importer-patch-agent.jar")).unwrap_or_default(),
        home().join("Git-projects/java-debug-model/jdtls-patch/jdtls-maven-importer-patch-agent.jar"),
    ]);
    t
}

fn newest_m2_lombok() -> Option<PathBuf> {
    let dir = home().join(".m2/repository/org/projectlombok/lombok");
    let mut versions: Vec<(Vec<u32>, PathBuf)> = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let v = e.file_name().to_string_lossy().to_string();
            let jar = e.path().join(format!("lombok-{v}.jar"));
            jar.exists().then(|| (v.split('.').filter_map(|p| p.parse().ok()).collect(), jar))
        })
        .collect();
    versions.sort();
    versions.pop().map(|(_, p)| p)
}

/// chodilamide-debugger.jar (JDI debug adapter): env override, installed copy, or the
/// build output next to this source tree during development.
pub fn find_debugger() -> Option<PathBuf> {
    let mut cands: Vec<PathBuf> = Vec::new();
    if let Some(p) = std::env::var_os("CHODILAMIDE_DEBUGGER") {
        cands.push(PathBuf::from(p));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            cands.push(dir.join("chodilamide-debugger.jar"));
            cands.push(dir.join("../share/chodilamide/chodilamide-debugger.jar"));
        }
    }
    cands.push(data_dirs()[0].join("tools/chodilamide-debugger.jar"));
    cands.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../debugger/build/chodilamide-debugger.jar"));
    cands.into_iter().find(|p| p.is_file())
}

/// chodilamide-testrunner.jar (runs JUnit inside the launched test JVM), found like the debugger.
pub fn find_testrunner() -> Option<PathBuf> {
    find_debugger().map(|d| d.with_file_name("chodilamide-testrunner.jar")).filter(|p| p.is_file())
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Jdk {
    /// jdtls runtime name, e.g. "JavaSE-21" or "JavaSE-1.8"
    pub name: String,
    pub path: PathBuf,
    pub major: u32,
}

/// `release` file → major version (8 for "1.8.0_x").
pub fn jdk_major(home: &Path) -> Option<u32> {
    let release = std::fs::read_to_string(home.join("release")).ok()?;
    let line = release.lines().find(|l| l.starts_with("JAVA_VERSION="))?;
    let v = line.trim_start_matches("JAVA_VERSION=").trim_matches('"');
    let mut parts = v.split(['.', '_', '-', '+']);
    let first: u32 = parts.next()?.parse().ok()?;
    if first == 1 { parts.next()?.parse().ok() } else { Some(first) }
}

/// Installed JDKs: /usr/lib/jvm, sdkman, macOS JavaVirtualMachines, $JAVA_HOME. Deduped by
/// real path, one per major version (highest path wins ties deterministically).
pub fn find_jdks() -> Vec<Jdk> {
    let mut homes: Vec<PathBuf> = Vec::new();
    for root in [PathBuf::from("/usr/lib/jvm"), home().join(".sdkman/candidates/java"), PathBuf::from("/Library/Java/JavaVirtualMachines")] {
        if let Ok(rd) = std::fs::read_dir(&root) {
            for e in rd.flatten() {
                let p = e.path();
                homes.push(if p.join("Contents/Home").exists() { p.join("Contents/Home") } else { p });
            }
        }
    }
    if let Some(jh) = std::env::var_os("JAVA_HOME") {
        homes.push(PathBuf::from(jh));
    }
    // JDKs downloaded by `tools install` (tools/jdk-21/...)
    if let Ok(rd) = std::fs::read_dir(tools_dir()) {
        for e in rd.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("jdk")) {
            homes.push(e.path());
        }
    }
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<Jdk> = Vec::new();
    // one JDK per major: the app's own download wins over the OS copy
    let tools = tools_dir();
    homes.sort_by_key(|h| (!h.starts_with(&tools), h.clone()));
    for h in homes {
        let Ok(real) = canonical(&h) else { continue };
        if !(real.join("bin/javac").exists() || real.join("bin/javac.exe").exists()) || !seen.insert(real.clone()) {
            continue;
        }
        let Some(major) = jdk_major(&real) else { continue };
        if out.iter().any(|j| j.major == major) {
            continue;
        }
        let name = if major <= 8 { format!("JavaSE-1.{major}") } else { format!("JavaSE-{major}") };
        out.push(Jdk { name, path: real, major });
    }
    out.sort_by_key(|j| j.major);
    out
}

/// JDK to run jdtls itself on (it needs 21+).
impl Jdk {
    /// downloaded by the app (tools_dir), not the OS
    pub fn managed(&self) -> bool {
        canonical(&tools_dir()).is_ok_and(|t| self.path.starts_with(t))
    }

    pub fn label(&self) -> String {
        if self.managed() { format!("JDK {} (app)", self.major) } else { format!("JDK {}  {}", self.major, self.path.display()) }
    }
}

pub fn jdk_for_jdtls(jdks: &[Jdk]) -> Option<&Jdk> {
    jdks.iter().filter(|j| j.major >= 21).min_by_key(|j| j.major)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_verbatim_paths() {
        assert_eq!(plain_path(Path::new(r"\\?\C:\Users\a\jdk-11")), PathBuf::from(r"C:\Users\a\jdk-11"));
        assert_eq!(plain_path(Path::new(r"\\?\UNC\srv\share\jdk")), PathBuf::from(r"\\srv\share\jdk"));
        assert_eq!(plain_path(Path::new("/usr/lib/jvm/java-21")), PathBuf::from("/usr/lib/jvm/java-21"));
        assert_eq!(java_bin("/opt/jdk/", "java"), PathBuf::from("/opt/jdk/bin/java"));
    }

    #[test]
    fn parses_release_versions() {
        let d = std::env::temp_dir().join(format!("jdk-test-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        for (content, want) in [("JAVA_VERSION=\"21.0.11\"", 21), ("JAVA_VERSION=\"1.8.0_402\"", 8), ("JAVA_VERSION=\"26\"", 26)] {
            std::fs::write(d.join("release"), format!("IMPLEMENTOR=\"x\"\n{content}\n")).unwrap();
            assert_eq!(jdk_major(&d), Some(want));
        }
        std::fs::remove_dir_all(&d).unwrap();
    }
}
