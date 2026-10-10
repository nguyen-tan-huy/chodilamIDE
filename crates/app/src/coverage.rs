//! Run with Coverage (IntelliJ's coverage runner, JaCoCo engine): tests run with the JaCoCo
//! agent, then `jacococli report --xml` gives per-line hits; covered / partly / not covered
//! lines are marked in the gutter and a summary goes to the Run tab.
use crate::workspace::Workspace;
use gpui::Context;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Per-line coverage: 0 = not covered, 1 = partly (some branches), 2 = covered.
pub type LineCoverage = Vec<(usize, u8)>;

#[derive(Debug, Clone, PartialEq)]
pub struct FileCoverage {
    /// "vn/demo/Order.java" (package path + file)
    pub rel: String,
    pub lines: LineCoverage,
    pub covered: usize,
    pub missed: usize,
}

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let key = format!(" {name}=\"");
    let i = tag.find(&key)? + key.len();
    let j = tag[i..].find('"')?;
    Some(&tag[i..i + j])
}

/// JaCoCo XML report → coverage per source file.
pub fn parse_report(xml: &str) -> Vec<FileCoverage> {
    let mut out = Vec::new();
    let mut package = String::new();
    let mut cur: Option<FileCoverage> = None;
    let mut rest = xml;
    while let Some(i) = rest.find('<') {
        let Some(j) = rest[i..].find('>') else { break };
        let tag = &rest[i..i + j + 1];
        rest = &rest[i + j + 1..];
        if tag.starts_with("<package ") {
            package = attr(tag, "name").unwrap_or("").to_string();
        } else if tag.starts_with("<sourcefile ") {
            let name = attr(tag, "name").unwrap_or("");
            cur = Some(FileCoverage { rel: if package.is_empty() { name.to_string() } else { format!("{package}/{name}") }, lines: Vec::new(), covered: 0, missed: 0 });
        } else if tag.starts_with("</sourcefile") {
            if let Some(f) = cur.take() {
                out.push(f);
            }
        } else if tag.starts_with("<line ") {
            if let Some(f) = cur.as_mut() {
                let n = |k| attr(tag, k).and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
                let (nr, mi, ci, mb, cb) = (n("nr"), n("mi"), n("ci"), n("mb"), n("cb"));
                let kind = if ci == 0 { 0 } else if mi > 0 || mb > 0 { 1 } else { 2 };
                let _ = cb;
                f.lines.push((nr.saturating_sub(1), kind));
            }
        } else if tag.starts_with("<counter ") && attr(tag, "type") == Some("LINE") {
            if let Some(f) = cur.as_mut() {
                f.missed = attr(tag, "missed").and_then(|v| v.parse().ok()).unwrap_or(0);
                f.covered = attr(tag, "covered").and_then(|v| v.parse().ok()).unwrap_or(0);
            }
        }
    }
    out
}

impl Workspace {
    /// The exec file of a coverage run and the agent argument for the test JVM.
    pub(crate) fn coverage_agent_arg(&self, session: usize) -> (PathBuf, String) {
        let (agent, _) = toolchain::managed::jacoco_paths();
        let exec = std::env::temp_dir().join(format!("chodilamide-jacoco-{}-{session}.exec", std::process::id()));
        let arg = format!("-javaagent:{}=destfile={},append=false", agent.display(), exec.display());
        (exec, arg)
    }

    /// After the coverage test run: report → gutter marks + summary.
    pub(crate) fn finish_coverage(&mut self, id: usize, exec: PathBuf, module_path: String, cx: &mut Context<Self>) {
        let Some(project) = self.project.clone() else { return };
        let Some(module) = project.modules.iter().find(|m| m.path == module_path).cloned() else { return };
        let java = runner::config::project_jdk(&self.root_str()).map(|j| toolchain::java_bin(&j, "java").display().to_string()).unwrap_or_else(|| "java".into());
        self.session_output(id, "\nCoverage: đang tạo báo cáo JaCoCo…\n", false, cx);
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    let (_, cli) = toolchain::managed::ensure_jacoco().map_err(|e| e.to_string())?;
                    if !exec.is_file() {
                        return Err("không có file .exec (test JVM không chạy agent?)".to_string());
                    }
                    let xml = exec.with_extension("xml");
                    let roots: Vec<String> = module.source_roots.iter().filter(|s| s.kind == "main" && s.lang == "java").map(|s| s.path.clone()).collect();
                    let mut cmd = std::process::Command::new(&java);
                    cmd.arg("-jar").arg(&cli).arg("report").arg(&exec).arg("--classfiles").arg(Path::new(&module.path).join("target/classes")).arg("--xml").arg(&xml);
                    for r in &roots {
                        cmd.arg("--sourcefiles").arg(r);
                    }
                    let out = cmd.output().map_err(|e| e.to_string())?;
                    if !out.status.success() {
                        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
                    }
                    let report = parse_report(&std::fs::read_to_string(&xml).map_err(|e| e.to_string())?);
                    // rel path → absolute through the source roots
                    let mut abs: HashMap<PathBuf, FileCoverage> = HashMap::new();
                    for f in report {
                        if let Some(p) = roots.iter().map(|r| Path::new(r).join(&f.rel)).find(|p| p.is_file()) {
                            abs.insert(p, f);
                        }
                    }
                    Ok(abs)
                })
                .await;
            let _ = this.update(cx, |ws, cx| match res {
                Ok(files) => {
                    let (cov, miss): (usize, usize) = files.values().fold((0, 0), |(c, m), f| (c + f.covered, m + f.missed));
                    let pct = |c: usize, m: usize| if c + m == 0 { 0.0 } else { 100.0 * c as f64 / (c + m) as f64 };
                    let mut lines = vec![format!("Coverage (dòng): {:.1}%  ({cov}/{})", pct(cov, miss), cov + miss)];
                    let mut per: Vec<&FileCoverage> = files.values().filter(|f| f.covered + f.missed > 0).collect();
                    per.sort_by(|a, b| pct(a.covered, a.missed).partial_cmp(&pct(b.covered, b.missed)).unwrap_or(std::cmp::Ordering::Equal));
                    for f in per.iter().take(30) {
                        lines.push(format!("  {:>5.1}%  {}", pct(f.covered, f.missed), f.rel));
                    }
                    ws.session_output(id, &(lines.join("\n") + "\n"), false, cx);
                    ws.coverage = files.into_iter().map(|(p, f)| (p, f.lines)).collect();
                    ws.push_coverage_to_tabs(cx);
                }
                Err(e) => ws.session_output(id, &format!("Coverage lỗi: {e}\n"), true, cx),
            });
        })
        .detach();
    }

    pub(crate) fn push_coverage_to_tabs(&mut self, cx: &mut Context<Self>) {
        for t in self.tabs.clone() {
            let marks = t.read(cx).path().and_then(|p| self.coverage.get(p).cloned()).unwrap_or_default();
            t.update(cx, |v, cx| {
                v.coverage = marks;
                cx.notify()
            });
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn jacoco_xml() {
        let xml = r#"<?xml version="1.0"?><!DOCTYPE report><report name="x"><package name="vn/demo"><class name="vn/demo/A"/><sourcefile name="A.java"><line nr="3" mi="0" ci="2" mb="0" cb="0"/><line nr="5" mi="1" ci="2" mb="1" cb="1"/><line nr="7" mi="3" ci="0" mb="0" cb="0"/><counter type="INSTRUCTION" missed="4" covered="4"/><counter type="LINE" missed="1" covered="2"/></sourcefile></package></report>"#;
        let r = super::parse_report(xml);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].rel, "vn/demo/A.java");
        assert_eq!(r[0].lines, [(2, 2), (4, 1), (6, 0)]);
        assert_eq!((r[0].covered, r[0].missed), (2, 1));
    }
}

#[cfg(test)]
mod real_tests {
    /// Real JaCoCo round trip on debugger/test/sample: `cargo test -p chodilamide-app jacoco_real -- --ignored`
    #[test]
    #[ignore]
    fn jacoco_real() {
        let (agent, cli) = toolchain::managed::ensure_jacoco().unwrap();
        let s = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../debugger/test/sample");
        let out = std::env::temp_dir().join(format!("jacoco-real-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&out);
        std::fs::create_dir_all(&out).unwrap();
        let javac = "/usr/lib/jvm/java-21-openjdk/bin/javac";
        let java = "/usr/lib/jvm/java-21-openjdk/bin/java";
        let srcs = [s.join("core/src/vn/demo/core/Greeter.java"), s.join("app/src/vn/demo/app/Main.java")];
        assert!(std::process::Command::new(javac).arg("-g").arg("-d").arg(&out).args(&srcs).status().unwrap().success());
        let exec = out.join("t.exec");
        let st = std::process::Command::new(java).arg(format!("-javaagent:{}=destfile={}", agent.display(), exec.display())).arg("-cp").arg(&out).arg("vn.demo.app.Main").status().unwrap();
        assert!(st.success());
        let xml = out.join("r.xml");
        let st = std::process::Command::new(java)
            .arg("-jar").arg(&cli).arg("report").arg(&exec).arg("--classfiles").arg(&out)
            .arg("--sourcefiles").arg(s.join("core/src")).arg("--sourcefiles").arg(s.join("app/src")).arg("--xml").arg(&xml)
            .status().unwrap();
        assert!(st.success());
        let r = super::parse_report(&std::fs::read_to_string(&xml).unwrap());
        let g = r.iter().find(|f| f.rel.ends_with("Greeter.java")).expect("Greeter in report");
        assert!(g.covered > 0, "{g:?}");
        eprintln!("{:?}", r.iter().map(|f| (f.rel.clone(), f.covered, f.missed)).collect::<Vec<_>>());
        let _ = std::fs::remove_dir_all(&out);
    }
}
