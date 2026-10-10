//! JDKs and Maven installs the app manages itself, like IntelliJ's "Project SDK" +
//! "Maven home path (Bundled)": the app works on a machine with neither `java` nor `mvn`
//! and downloads what is missing into `tools_dir()` (Eclipse Temurin JDKs as `jdk-<major>`,
//! Apache Maven as `maven/apache-maven-<version>`).
//!
//! The choice (Settings → Java & Maven) is process-global: `configure()` is called at start
//! and after every save, and every Maven/java launch goes through `mvn_program()` /
//! `default_jdk()`.
use crate::install::{curl, untar};
use crate::{find_jdks, tools_dir, Jdk};
use anyhow::{bail, Result};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};

/// Maven downloaded when the machine has none ("Bundled (Maven 3)").
pub const MAVEN_VERSION: &str = "3.9.11";
/// JDK downloaded when none is installed (jdtls needs 21+).
pub const DEFAULT_JDK: u32 = 21;
/// Offered by "Tải JDK" in Settings (Temurin LTS releases + current).
pub const JDK_CHOICES: &[u32] = &[8, 11, 17, 21, 25];

#[derive(Debug, Clone, PartialEq)]
pub struct Selection {
    /// None = automatic: JAVA_HOME / `java` on PATH, else the newest known JDK
    pub jdk_home: Option<PathBuf>,
    /// None = automatic: `mvn` on PATH, else the newest managed Maven
    pub maven_home: Option<PathBuf>,
    /// run the project's ./mvnw when it has one (IntelliJ "Use Maven wrapper")
    pub use_wrapper: bool,
    /// download a JDK / Maven when none is found
    pub auto_download: bool,
}

impl Default for Selection {
    fn default() -> Self {
        Self { jdk_home: None, maven_home: None, use_wrapper: true, auto_download: true }
    }
}

static SELECTION: RwLock<Option<Selection>> = RwLock::new(None);
/// Held while downloading so two projects opening at once don't fetch the same archive twice.
static INSTALL: Mutex<()> = Mutex::new(());
/// What is being downloaded right now, for the status bar.
static PROGRESS: Mutex<Option<String>> = Mutex::new(None);

pub fn configure(sel: Selection) {
    *SELECTION.write().unwrap() = Some(sel);
}

pub fn selection() -> Selection {
    SELECTION.read().unwrap().clone().unwrap_or_default()
}

pub fn progress() -> Option<String> {
    PROGRESS.lock().unwrap().clone()
}

fn set_progress(msg: Option<String>) {
    *PROGRESS.lock().unwrap() = msg;
}

fn exe(name: &str) -> String {
    if cfg!(windows) { format!("{name}.exe") } else { name.to_string() }
}

pub fn which(cmd: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let names: Vec<String> = if cfg!(windows) { vec![format!("{cmd}.exe"), format!("{cmd}.cmd"), cmd.to_string()] } else { vec![cmd.to_string()] };
    std::env::split_paths(&path).flat_map(|p| names.iter().map(move |n| p.join(n))).find(|p| p.is_file())
}

// ---------------------------------------------------------------- JDK

/// The JDK Maven and Run/Debug use when neither the run configuration nor the project picks
/// one: the configured one, else the newest JDK the app downloaded (never silently the OS
/// one when the app has its own), else None = JAVA_HOME / PATH, else any JDK found.
pub fn default_jdk() -> Option<PathBuf> {
    if let Some(j) = selection().jdk_home.filter(|j| j.join("bin").join(exe("java")).is_file()) {
        return Some(j);
    }
    let jdks = find_jdks();
    if let Some(j) = jdks.iter().rev().find(|j| j.managed()) {
        return Some(j.path.clone());
    }
    if system_java() {
        return None;
    }
    jdks.last().map(|j| j.path.clone())
}

fn system_java() -> bool {
    std::env::var_os("JAVA_HOME").is_some_and(|j| Path::new(&j).join("bin").join(exe("java")).is_file()) || which("java").is_some()
}

/// Makes sure a JDK >= `min_major` exists, downloading Temurin `max(min_major, 21)` when not
/// (and auto-download is on). Blocking - call from a background task.
pub fn ensure_jdk(min_major: u32) -> Result<Jdk> {
    if let Some(j) = find_jdks().into_iter().find(|j| j.major >= min_major) {
        return Ok(j);
    }
    if !selection().auto_download {
        bail!("không có JDK {min_major}+ (tự tải đang tắt trong Cài đặt → Java & Maven)");
    }
    let major = min_major.max(DEFAULT_JDK);
    install_jdk(major)?;
    find_jdks().into_iter().find(|j| j.major >= min_major).ok_or_else(|| anyhow::anyhow!("đã tải JDK {major} nhưng không nhận ra"))
}

/// Java for Maven when nothing is configured and the system has none: downloads one.
pub fn ensure_any_java() -> Result<Option<PathBuf>> {
    if let Some(j) = default_jdk() {
        return Ok(Some(j));
    }
    if system_java() {
        return Ok(None);
    }
    ensure_jdk(1).map(|j| Some(j.path))
}

/// Downloads Eclipse Temurin `major` into tools_dir()/jdk-<major> (skipped when present).
pub fn install_jdk(major: u32) -> Result<PathBuf> {
    let _guard = INSTALL.lock().unwrap();
    let dir = tools_dir();
    let dest = dir.join(format!("jdk-{major}"));
    if dest.join("bin").join(exe("java")).is_file() || dest.join("Contents/Home/bin/java").is_file() {
        return Ok(dest);
    }
    std::fs::create_dir_all(&dir)?;
    set_progress(Some(format!("Đang tải JDK {major} (Temurin, ~200 MB)…")));
    let res = (|| {
        let ext = if cfg!(windows) { "zip" } else { "tar.gz" };
        let tmp = dir.join(format!("jdk-{major}.{ext}.part"));
        curl(&crate::install::jdk_url(major), &tmp)?;
        set_progress(Some(format!("Đang giải nén JDK {major}…")));
        let staging = dir.join(format!("jdk-{major}.new"));
        let _ = std::fs::remove_dir_all(&staging);
        untar(&tmp, &staging, 1)?;
        let _ = std::fs::remove_file(&tmp);
        let _ = std::fs::remove_dir_all(&dest);
        std::fs::rename(&staging, &dest)?;
        Ok(dest.clone())
    })();
    set_progress(None);
    res
}

// ---------------------------------------------------------------- Maven

#[derive(Debug, Clone, PartialEq)]
pub struct Maven {
    pub home: PathBuf,
    /// from lib/maven-core-<v>.jar, "?" when unknown
    pub version: String,
    pub managed: bool,
}

impl Maven {
    pub fn program(&self) -> PathBuf {
        self.home.join("bin").join(if cfg!(windows) { "mvn.cmd" } else { "mvn" })
    }

    pub fn label(&self) -> String {
        format!("Maven {}{}  {}", self.version, if self.managed { " (app)" } else { "" }, self.home.display())
    }
}

pub fn mavens_dir() -> PathBuf {
    tools_dir().join("maven")
}

fn maven_version(home: &Path) -> Option<String> {
    std::fs::read_dir(home.join("lib")).ok()?.flatten().find_map(|e| {
        let n = e.file_name().to_string_lossy().to_string();
        n.strip_prefix("maven-core-").and_then(|v| v.strip_suffix(".jar")).map(String::from)
    })
}

fn maven_at(home: &Path, managed: bool) -> Option<Maven> {
    let home = crate::canonical(&home).ok()?;
    let m = Maven { version: maven_version(&home).unwrap_or_else(|| "?".into()), home, managed };
    m.program().is_file().then_some(m)
}

fn version_key(v: &str) -> Vec<u32> {
    v.split(['.', '-']).filter_map(|p| p.parse().ok()).collect()
}

/// Every Maven found: managed ones, MAVEN_HOME/M2_HOME, `mvn` on PATH, sdkman, common dirs.
pub fn find_mavens() -> Vec<Maven> {
    let mut out: Vec<Maven> = Vec::new();
    let mut push = |m: Option<Maven>| {
        if let Some(m) = m {
            if !out.iter().any(|o| o.home == m.home) {
                out.push(m);
            }
        }
    };
    if let Ok(rd) = std::fs::read_dir(mavens_dir()) {
        let mut managed: Vec<Maven> = rd.flatten().filter_map(|e| maven_at(&e.path(), true)).collect();
        managed.sort_by(|a, b| version_key(&b.version).cmp(&version_key(&a.version)));
        managed.into_iter().for_each(|m| push(Some(m)));
    }
    for var in ["MAVEN_HOME", "M2_HOME"] {
        if let Some(h) = std::env::var_os(var) {
            push(maven_at(Path::new(&h), false));
        }
    }
    // /usr/bin/mvn → /usr/share/maven/bin/mvn → home /usr/share/maven
    if let Some(bin) = which("mvn").and_then(|p| crate::canonical(&p).ok()) {
        if let Some(home) = bin.parent().and_then(Path::parent) {
            push(maven_at(home, false));
        }
    }
    let home = crate::home();
    let mut dirs = vec![PathBuf::from("/usr/share/maven"), PathBuf::from("/opt/maven"), PathBuf::from("/usr/local/opt/maven/libexec")];
    if let Ok(rd) = std::fs::read_dir(home.join(".sdkman/candidates/maven")) {
        dirs.extend(rd.flatten().map(|e| e.path()).filter(|p| !p.ends_with("current")));
    }
    for d in dirs {
        push(maven_at(&d, false));
    }
    out
}

/// The Maven executable to run when the project has no (usable) wrapper: the configured
/// home, else `mvn` on PATH, else the newest managed install. Never downloads.
pub fn mvn_program() -> Option<PathBuf> {
    if let Some(m) = selection().maven_home.and_then(|h| maven_at(&h, false)) {
        return Some(m.program());
    }
    if let Some(p) = which("mvn") {
        return Some(p);
    }
    find_mavens().into_iter().find(|m| m.managed).map(|m| m.program())
}

/// `mvn_program()`, downloading Maven MAVEN_VERSION first when there is none. Blocking.
pub fn ensure_maven() -> Result<PathBuf> {
    if let Some(p) = mvn_program() {
        return Ok(p);
    }
    if !selection().auto_download {
        bail!("không có Maven (tự tải đang tắt trong Cài đặt → Java & Maven)");
    }
    Ok(install_maven(MAVEN_VERSION)?.program())
}

pub fn maven_url(version: &str) -> String {
    let major = version.split('.').next().unwrap_or("3");
    let ext = if cfg!(windows) { "zip" } else { "tar.gz" };
    format!("https://archive.apache.org/dist/maven/maven-{major}/{version}/binaries/apache-maven-{version}-bin.{ext}")
}

/// Downloads Apache Maven into tools_dir()/maven/apache-maven-<version>.
pub fn install_maven(version: &str) -> Result<Maven> {
    let _guard = INSTALL.lock().unwrap();
    let dir = mavens_dir();
    let dest = dir.join(format!("apache-maven-{version}"));
    if let Some(m) = maven_at(&dest, true) {
        return Ok(m);
    }
    std::fs::create_dir_all(&dir)?;
    set_progress(Some(format!("Đang tải Maven {version}…")));
    let res = (|| {
        let tmp = dir.join(format!("apache-maven-{version}.part"));
        curl(&maven_url(version), &tmp)?;
        let staging = dir.join(format!("apache-maven-{version}.new"));
        let _ = std::fs::remove_dir_all(&staging);
        untar(&tmp, &staging, 1)?;
        let _ = std::fs::remove_file(&tmp);
        let _ = std::fs::remove_dir_all(&dest);
        std::fs::rename(&staging, &dest)?;
        maven_at(&dest, true).ok_or_else(|| anyhow::anyhow!("gói Maven không có bin/mvn"))
    })();
    set_progress(None);
    res
}

// ---------------------------------------------------------------- jdtls

/// jdtls (patched 1.54.0) + lombok, downloaded into tools_dir() when neither the app nor an
/// nvim install has them (a fresh machine right after `pacman -U`). Blocking.
pub fn ensure_jdtls() -> Result<()> {
    let t = crate::find_tools();
    if t.jdtls.is_some() && t.lombok.is_some() {
        return Ok(());
    }
    if !selection().auto_download {
        bail!("không có jdtls - chạy: chodilamide-ide-cli tools install (hoặc bật Tự tải trong Cài đặt)");
    }
    let _guard = INSTALL.lock().unwrap();
    set_progress(Some("Đang tải jdtls + lombok (~55 MB, lần đầu)…".into()));
    let r = crate::install::install(&tools_dir(), false, &mut |m| set_progress(Some(m.to_string())));
    set_progress(None);
    r.map(|_| ())
}

// ---------------------------------------------------------------- JaCoCo (coverage)

pub const JACOCO_VERSION: &str = "0.8.12";

/// (agent jar, cli jar) paths the app uses - tools_dir()/jacoco; may not exist yet.
pub fn jacoco_paths() -> (PathBuf, PathBuf) {
    let d = tools_dir().join("jacoco");
    (d.join("jacocoagent.jar"), d.join("jacococli.jar"))
}

/// Makes sure the JaCoCo agent + CLI jars exist: copied from ~/.m2 when a build already
/// fetched them, else downloaded from Maven Central. Blocking.
pub fn ensure_jacoco() -> Result<(PathBuf, PathBuf)> {
    let (agent, cli) = jacoco_paths();
    if agent.is_file() && cli.is_file() {
        return Ok((agent, cli));
    }
    let _guard = INSTALL.lock().unwrap();
    std::fs::create_dir_all(agent.parent().unwrap())?;
    let v = JACOCO_VERSION;
    let m2 = crate::home().join(".m2/repository/org/jacoco");
    for (dest, artifact, classifier) in [(&agent, "org.jacoco.agent", "runtime"), (&cli, "org.jacoco.cli", "nodeps")] {
        if dest.is_file() {
            continue;
        }
        let local = m2.join(artifact).join(v).join(format!("{artifact}-{v}-{classifier}.jar"));
        if local.is_file() {
            std::fs::copy(&local, dest)?;
            continue;
        }
        set_progress(Some(format!("Đang tải {artifact} {v}…")));
        let url = format!("https://repo1.maven.org/maven2/org/jacoco/{artifact}/{v}/{artifact}-{v}-{classifier}.jar");
        let tmp = dest.with_extension("part");
        let r = curl(&url, &tmp).and_then(|_| Ok(std::fs::rename(&tmp, dest)?));
        set_progress(None);
        r?;
    }
    Ok((agent, cli))
}

/// Sets JAVA_HOME + PATH for `java_home`, or for `default_jdk()` when None.
pub fn apply_java_env(c: &mut std::process::Command, java_home: Option<&str>) {
    let Some(j) = java_home.map(PathBuf::from).or_else(default_jdk).map(|j| crate::plain_path(&j)) else { return };
    c.env("JAVA_HOME", &j);
    let mut paths = vec![j.join("bin")];
    paths.extend(std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default());
    if let Ok(p) = std::env::join_paths(paths) {
        c.env("PATH", p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maven_detection_and_urls() {
        assert!(maven_url("3.9.11").ends_with("/maven-3/3.9.11/binaries/apache-maven-3.9.11-bin.tar.gz") || cfg!(windows));
        let home = std::env::temp_dir().join(format!("mvn-test-{}", std::process::id()));
        std::fs::create_dir_all(home.join("bin")).unwrap();
        std::fs::create_dir_all(home.join("lib")).unwrap();
        std::fs::write(home.join("bin/mvn"), "").unwrap();
        std::fs::write(home.join("lib/maven-core-3.9.11.jar"), "").unwrap();
        let m = maven_at(&home, true).unwrap();
        assert_eq!(m.version, "3.9.11");
        assert!(version_key("3.10.0") > version_key("3.9.11"));
        std::fs::remove_dir_all(&home).unwrap();
    }
}

#[cfg(test)]
mod net_tests {
    /// Real download: `XDG_DATA_HOME=/tmp/x cargo test -p toolchain -- --ignored`
    #[test]
    #[ignore]
    fn downloads_maven() {
        let m = super::install_maven(super::MAVEN_VERSION).unwrap();
        assert_eq!(m.version, super::MAVEN_VERSION);
        assert!(super::find_mavens().iter().any(|f| f.home == m.home && f.managed));
    }
}

#[cfg(test)]
mod jdtls_net_test {
    /// Fresh machine: `XDG_DATA_HOME=/tmp/x HOME=/tmp/x cargo test -p toolchain jdtls_fresh -- --ignored`
    #[test]
    #[ignore]
    fn jdtls_fresh() {
        super::ensure_jdtls().unwrap();
        let t = crate::find_tools();
        assert!(t.jdtls.is_some() && t.lombok.is_some(), "{t:?}");
    }
}
