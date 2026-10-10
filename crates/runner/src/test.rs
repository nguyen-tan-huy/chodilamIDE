//! JUnit tests. Fast path (used by the IDE): chodilamide-testrunner inside a JVM launched by
//! the debugger - `launch_classpath`, `runner_args`, `parse_runner_line`. Maven Surefire path
//! (`mvn_test`, `parse_report`) stays for the CLI and CI-like runs.
use crate::maven::{MavenCommand, command};
use project_model::Module;
use std::path::Path;
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq)]
pub struct TestTarget {
    pub class: String,
    pub methods: Vec<String>,
}

/// `-Dtest=` value: `a.B#m1+m2,a.C`
pub fn test_filter(targets: &[TestTarget]) -> String {
    targets
        .iter()
        .map(|t| if t.methods.is_empty() { t.class.clone() } else { format!("{}#{}", t.class, t.methods.join("+")) })
        .collect::<Vec<_>>()
        .join(",")
}

/// `mvn test -Dtest=…` for `module`; `debug_port` makes Surefire's forked JVM wait for a debugger.
pub fn mvn_test(root: &Path, module: &Module, targets: &[TestTarget], profiles: &[String], java_home: Option<String>, debug_port: Option<u16>) -> MavenCommand {
    let mut cmd = command(root, module, &["test"], profiles, false, java_home);
    cmd.args.push(format!("-Dtest={}", test_filter(targets)));
    // other reactor modules built by -am have no matching tests
    cmd.args.push("-Dsurefire.failIfNoSpecifiedTests=false".into());
    cmd.args.push("-DfailIfNoTests=false".into());
    // one JVM, so a debugger attaches once
    cmd.args.push("-DforkCount=1".into());
    cmd.args.push("-DreuseForks=true".into());
    if let Some(p) = debug_port {
        cmd.args.push(format!("-Dmaven.surefire.debug=-agentlib:jdwp=transport=dt_socket,server=y,suspend=y,address=127.0.0.1:{p}"));
    }
    cmd
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Status {
    Passed,
    Failed,
    Error,
    Skipped,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TestResult {
    pub class: String,
    /// Java method name (display names of parameterized tests differ from it)
    pub method: String,
    pub name: String,
    pub status: Status,
    pub seconds: f64,
    pub message: String,
    pub trace: String,
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let key = format!("{name}=\"");
    let s = tag.find(&key)? + key.len();
    let e = tag[s..].find('"')? + s;
    Some(unescape(&tag[s..e]))
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&#10;", "\n").replace("&amp;", "&")
}

/// Parses one Surefire `TEST-*.xml` report.
pub fn parse_report(xml: &str) -> Vec<TestResult> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<testcase") {
        rest = &rest[start..];
        let tag_end = rest.find('>').unwrap_or(rest.len());
        let tag = &rest[..tag_end];
        let self_closing = tag.ends_with('/');
        let body_end = if self_closing { tag_end } else { rest.find("</testcase>").unwrap_or(tag_end) };
        let body = &rest[tag_end.min(body_end)..body_end];
        let (status, kind) = if body.contains("<failure") {
            (Status::Failed, "failure")
        } else if body.contains("<error") {
            (Status::Error, "error")
        } else if body.contains("<skipped") {
            (Status::Skipped, "skipped")
        } else {
            (Status::Passed, "")
        };
        let (mut message, mut trace) = (String::new(), String::new());
        if !kind.is_empty() {
            if let Some(i) = body.find(&format!("<{kind}")) {
                let t_end = body[i..].find('>').map(|e| i + e).unwrap_or(body.len());
                message = attr(&body[i..t_end], "message").unwrap_or_default();
                if !body[i..t_end].ends_with('/') {
                    let close = format!("</{kind}>");
                    let c_end = body[t_end..].find(&close).map(|e| t_end + e).unwrap_or(body.len());
                    trace = unescape(body[t_end + 1..c_end].trim().trim_start_matches("<![CDATA[").trim_end_matches("]]>"));
                }
            }
        }
        let name = attr(tag, "name").unwrap_or_default();
        out.push(TestResult {
            class: attr(tag, "classname").unwrap_or_default(),
            method: name.split('(').next().unwrap_or(&name).to_string(),
            name,
            status,
            seconds: attr(tag, "time").and_then(|t| t.replace(',', "").parse().ok()).unwrap_or(0.0),
            message,
            trace,
        });
        rest = &rest[body_end.max(1)..];
    }
    out
}

/// Results of every report in `<module>/target/surefire-reports` written after `since`.
pub fn collect_results(module_dir: &Path, since: SystemTime) -> Vec<TestResult> {
    let dir = module_dir.join("target/surefire-reports");
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(&dir) else { return out };
    let mut files: Vec<_> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("TEST-") && n.to_string_lossy().ends_with(".xml")))
        .filter(|p| std::fs::metadata(p).and_then(|m| m.modified()).is_ok_and(|t| t >= since))
        .collect();
    files.sort();
    for f in files {
        if let Ok(xml) = std::fs::read_to_string(&f) {
            out.extend(parse_report(&xml));
        }
    }
    out
}

/// Targets for "Rerun failed tests".
pub fn failed_targets(results: &[TestResult]) -> Vec<TestTarget> {
    let mut out: Vec<TestTarget> = Vec::new();
    for r in results.iter().filter(|r| matches!(r.status, Status::Failed | Status::Error)) {
        if r.method.is_empty() {
            // a class-level failure: rerun the whole class
            if !out.iter().any(|t| t.class == r.class) {
                out.push(TestTarget { class: r.class.clone(), methods: vec![] });
            }
            continue;
        }
        let m = r.method.clone();
        match out.iter_mut().find(|t| t.class == r.class) {
            Some(t) if !t.methods.is_empty() && !t.methods.contains(&m) => t.methods.push(m),
            Some(_) => {}
            None => out.push(TestTarget { class: r.class.clone(), methods: vec![m] }),
        }
    }
    out
}

/// Program arguments for chodilamide.testrunner.Main: "a.B a.C#m1 a.C#m2".
pub fn runner_args(targets: &[TestTarget]) -> String {
    let mut out = Vec::new();
    for t in targets {
        if t.methods.is_empty() {
            out.push(t.class.clone());
        } else {
            out.extend(t.methods.iter().map(|m| format!("{}#{m}", t.class)));
        }
    }
    out.join(" ")
}

pub const RESULT_PREFIX: &str = "@@CDL ";

/// A "finished" line printed by the test runner → result. Other lines → None.
pub fn parse_runner_line(line: &str) -> Option<TestResult> {
    let v: serde_json::Value = serde_json::from_str(line.strip_prefix(RESULT_PREFIX)?).ok()?;
    if v["event"] != "finished" {
        return None;
    }
    let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
    Some(TestResult {
        class: s("class"),
        method: s("method"),
        name: s("name"),
        status: match v["status"].as_str() {
            Some("passed") => Status::Passed,
            Some("failed") => Status::Failed,
            Some("skipped") => Status::Skipped,
            _ => Status::Error,
        },
        seconds: v["ms"].as_f64().unwrap_or(0.0) / 1000.0,
        message: s("message"),
        trace: s("trace"),
    })
}

/// Test-launch classpath: module test+main output, siblings, test-scope jars, plus the
/// junit-platform-launcher matching the project's junit-platform-engine (from ~/.m2) and
/// the runner jar itself.
pub fn launch_classpath(project: &project_model::Project, module: &Module, runner_jar: &Path) -> anyhow::Result<Vec<String>> {
    let mut cp = crate::classpath::classpath(project, module, true);
    let has_launcher = cp.iter().any(|j| j.contains("junit-platform-launcher-"));
    let engine = cp.iter().find_map(|j| {
        let f = j.rsplit('/').next()?;
        f.strip_prefix("junit-platform-engine-")?.strip_suffix(".jar").map(String::from)
    });
    if !has_launcher {
        if let Some(ver) = engine {
            let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default();
            let jar = home.join(format!(".m2/repository/org/junit/platform/junit-platform-launcher/{ver}/junit-platform-launcher-{ver}.jar"));
            if !jar.is_file() {
                anyhow::bail!(
                    "thiếu junit-platform-launcher {ver} trong ~/.m2 - chạy một lần: mvn dependency:get -Dartifact=org.junit.platform:junit-platform-launcher:{ver}"
                );
            }
            cp.push(jar.display().to_string());
        }
    }
    cp.push(runner_jar.display().to_string());
    Ok(cp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runner_protocol() {
        let l = r#"@@CDL {"event":"finished","class":"vn.demo.GiaTest","method":"giamSai","name":"giamSai()","status":"failed","ms":12,"message":"Tiếng Việt: sai giá","trace":"at vn.demo.GiaTest.giamSai(GiaTest.java:15)"}"#;
        let r = parse_runner_line(l).unwrap();
        assert_eq!((r.status, r.method.as_str(), r.seconds), (Status::Failed, "giamSai", 0.012));
        assert!(parse_runner_line(r#"@@CDL {"event":"started","class":"a","name":"b"}"#).is_none());
        assert!(parse_runner_line("normal output").is_none());
        let targets = vec![TestTarget { class: "a.B".into(), methods: vec![] }, TestTarget { class: "a.C".into(), methods: vec!["x".into(), "y".into()] }];
        assert_eq!(runner_args(&targets), "a.B a.C#x a.C#y");
        let failed = failed_targets(&[r.clone(), TestResult { method: "".into(), class: "a.Ctx".into(), ..r }]);
        assert_eq!(runner_args(&failed), "vn.demo.GiaTest#giamSai a.Ctx");
    }

    const REPORT: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<testsuite name="net.lvs.product.PriceTest" tests="3" failures="1" errors="1" skipped="0">
  <testcase name="giaDung()" classname="net.lvs.product.PriceTest" time="0.012"/>
  <testcase name="giaSai" classname="net.lvs.product.PriceTest" time="0,5">
    <failure message="expected: &lt;100&gt; but was: &lt;90&gt;" type="org.opentest4j.AssertionFailedError"><![CDATA[org.opentest4j.AssertionFailedError: expected: <100> but was: <90>
	at net.lvs.product.PriceTest.giaSai(PriceTest.java:21)]]></failure>
  </testcase>
  <testcase name="loi" classname="net.lvs.product.PriceTest" time="0.001">
    <error message="Tiếng Việt NPE" type="java.lang.NullPointerException">java.lang.NullPointerException: Tiếng Việt NPE</error>
    <system-out><![CDATA[log]]></system-out>
  </testcase>
</testsuite>"#;

    #[test]
    fn parses_surefire_report_and_rerun_filter() {
        let r = parse_report(REPORT);
        assert_eq!(r.len(), 3);
        assert_eq!(r[0].status, Status::Passed);
        assert_eq!(r[1].status, Status::Failed);
        assert_eq!(r[1].message, "expected: <100> but was: <90>");
        assert!(r[1].trace.contains("PriceTest.java:21"));
        assert_eq!(r[2].status, Status::Error);
        assert_eq!(r[2].message, "Tiếng Việt NPE");
        let failed = failed_targets(&r);
        assert_eq!(test_filter(&failed), "net.lvs.product.PriceTest#giaSai+loi");
        assert_eq!(test_filter(&[TestTarget { class: "a.B".into(), methods: vec![] }]), "a.B");
    }
}
