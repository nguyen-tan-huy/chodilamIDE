//! `tools install`: downloads what the IDE needs into ~/.local/share/chodilamide/tools when a
//! machine has no nvim-java/Mason copies - the patched jdtls 1.54.0 (same release the Lua
//! plugin uses), lombok, and optionally a JDK 21 (Eclipse Temurin). Uses the system `curl`
//! and `tar` (present on Linux, macOS and Windows 10+), like bootstrap.lua does.
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Same prebuilt jdtls as java-debug-model's DEFAULT_JDTLS_PREBUILT_URL.
pub const JDTLS_URL: &str =
    "https://github.com/nguyen-tan-huy/eclipse.jdt.ls/releases/download/1.54.0/jdt-language-server-1.54.0-202609010342.tar.gz";
pub const LOMBOK_URL: &str = "https://projectlombok.org/downloads/lombok.jar";

pub fn jdk_url(major: u32) -> String {
    let os = if cfg!(target_os = "macos") { "mac" } else if cfg!(windows) { "windows" } else { "linux" };
    let arch = if cfg!(target_arch = "aarch64") { "aarch64" } else { "x64" };
    format!("https://api.adoptium.net/v3/binary/latest/{major}/ga/{os}/{arch}/jdk/hotspot/normal/eclipse")
}

pub(crate) fn curl(url: &str, dest: &Path) -> Result<()> {
    let st = Command::new("curl")
        .args(["-fL", "--retry", "2", "--progress-bar", "-o"])
        .arg(dest)
        .arg(url)
        .status()
        .context("cần 'curl' trong PATH")?;
    if !st.success() {
        bail!("tải thất bại: {url}");
    }
    Ok(())
}

pub(crate) fn untar(archive: &Path, into: &Path, strip: u32) -> Result<()> {
    std::fs::create_dir_all(into)?;
    let mut c = Command::new("tar");
    // -xf: GNU tar detects gzip, Windows bsdtar also reads .zip
    c.arg("-xf").arg(archive).arg("-C").arg(into);
    if strip > 0 {
        c.arg(format!("--strip-components={strip}"));
    }
    if !c.status().context("cần 'tar' trong PATH")?.success() {
        bail!("giải nén thất bại: {}", archive.display());
    }
    Ok(())
}

/// Installs into `dir` (normally `tools_dir()`); skips what is already there.
/// Returns a line per item for the user.
pub fn install(dir: &Path, with_jdk: bool, log: &mut dyn FnMut(&str)) -> Result<Vec<String>> {
    std::fs::create_dir_all(dir)?;
    let mut done = Vec::new();

    let jdtls = dir.join("jdtls");
    if jdtls.join("bin/jdtls").is_file() {
        done.push(format!("jdtls: đã có ({})", jdtls.display()));
    } else {
        log("Tải jdtls 1.54.0 (bản đã vá)…");
        let tmp = dir.join("jdtls.tar.gz.part");
        curl(JDTLS_URL, &tmp)?;
        let staging = dir.join("jdtls.new");
        let _ = std::fs::remove_dir_all(&staging);
        untar(&tmp, &staging, 0)?;
        let _ = std::fs::remove_file(&tmp);
        if !staging.join("bin/jdtls").is_file() {
            bail!("gói jdtls không có bin/jdtls");
        }
        std::fs::rename(&staging, &jdtls)?;
        done.push(format!("jdtls: đã cài vào {}", jdtls.display()));
    }

    let lombok = dir.join("lombok.jar");
    if lombok.is_file() {
        done.push("lombok: đã có".into());
    } else {
        log("Tải lombok.jar…");
        let tmp = dir.join("lombok.jar.part");
        curl(LOMBOK_URL, &tmp)?;
        std::fs::rename(&tmp, &lombok)?;
        done.push(format!("lombok: đã cài vào {}", lombok.display()));
    }

    if with_jdk {
        log("Tải JDK 21 (Eclipse Temurin, ~200 MB) nếu chưa có…");
        done.push(format!("JDK 21: {}", crate::managed::install_jdk(21)?.display()));
    }
    Ok(done)
}

#[derive(Debug)]
pub struct Check {
    pub ok: bool,
    pub name: String,
    pub detail: String,
    /// what to do when not ok
    pub fix: String,
}

use crate::managed::which;

/// `doctor`: everything the IDE depends on, with a fix for each missing piece.
pub fn doctor() -> Vec<Check> {
    let tools = crate::find_tools();
    let jdks = crate::find_jdks();
    let mut out = Vec::new();
    let mut add = |ok: bool, name: &str, detail: String, fix: &str| out.push(Check { ok, name: name.into(), detail, fix: fix.into() });
    let fmt = |p: &Option<PathBuf>| p.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "không thấy".into());

    add(tools.jdtls.is_some(), "jdtls", fmt(&tools.jdtls), "chodilamide-ide-cli tools install");
    add(tools.lombok.is_some(), "lombok", fmt(&tools.lombok), "chodilamide-ide-cli tools install");
    add(tools.patch_agent.is_some(), "agent vá MavenProjectImporter", fmt(&tools.patch_agent), "cài lại gói chodilamide-ide (jar đi kèm)");
    let list = jdks.iter().map(|j| format!("{} ({})", j.name, j.path.display())).collect::<Vec<_>>().join(", ");
    add(jdks.iter().any(|j| j.major >= 21), "JDK 21+ (chạy jdtls)", if list.is_empty() { "không có JDK nào".into() } else { list.clone() }, "cài jdk21-openjdk, hoặc: chodilamide-ide-cli tools install --jdk");
    add(jdks.iter().any(|j| j.major >= 17), "JDK 17+ (chạy debugger)", String::new(), "như trên");
    add(crate::find_debugger().is_some(), "debugger JDI", fmt(&crate::find_debugger()), "cài lại gói, hoặc chạy debugger/build.sh");
    add(crate::find_testrunner().is_some(), "test runner JUnit", fmt(&crate::find_testrunner()), "cài lại gói, hoặc chạy debugger/build.sh");
    let mvn = crate::managed::mvn_program();
    add(mvn.is_some(), "Maven (mvn)", fmt(&mvn), "tự tải khi mở project, hoặc Cài đặt → Java & Maven → Tải Maven");
    let git = which("git");
    add(git.is_some(), "git", fmt(&git), "cài git");
    for n in &tools.notes {
        add(false, "ghi chú", n.clone(), "");
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn jdk_url_for_platform() {
        let u = super::jdk_url(21);
        assert!(u.starts_with("https://api.adoptium.net/v3/binary/latest/21/ga/"));
        assert!(u.contains("/jdk/hotspot/"));
    }
}
