//! Compiling just what is about to run, like IntelliJ does: `javac -sourcepath …` on the
//! needed source files only, so an unrelated broken test file (or a pom with
//! `maven.test.skip=true`, which also hides src/test/java from jdtls) can't stop a run.
use std::path::{Path, PathBuf};
use std::process::Command;

/// `pkg/Class.java` under one of `roots`, for class `a.b.Class` (or `a.b.Class$Inner`).
pub fn source_file(roots: &[String], class: &str) -> Option<PathBuf> {
    let top = class.split('$').next().unwrap_or(class);
    let rel = format!("{}.java", top.replace('.', "/"));
    roots.iter().map(|r| Path::new(r).join(&rel)).find(|p| p.is_file())
}

/// javac for `files` into `out`; referenced sources are found through `sourcepath` and
/// compiled only when needed. Long classpaths go through an @argfile.
pub fn javac(java_home: Option<&str>, classpath: &[String], sourcepath: &[String], out: &Path, files: &[PathBuf], argfile: &Path) -> std::io::Result<Command> {
    let sep = if cfg!(windows) { ";" } else { ":" };
    let quote = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
    let mut lines = vec![
        "-encoding".to_string(),
        "UTF-8".into(),
        "-g".into(),
        "-parameters".into(),
        "-implicit:class".into(),
        "-nowarn".into(),
        "-d".into(),
        quote(&out.display().to_string()),
        "-cp".into(),
        quote(&classpath.join(sep)),
        "-sourcepath".into(),
        quote(&sourcepath.join(sep)),
    ];
    lines.extend(files.iter().map(|f| quote(&f.display().to_string())));
    std::fs::write(argfile, lines.join("\n"))?;
    std::fs::create_dir_all(out)?;
    let program = match java_home {
        Some(j) => toolchain::java_bin(j, "javac").display().to_string(),
        None => "javac".into(),
    };
    let mut c = Command::new(program);
    c.arg(format!("@{}", argfile.display()));
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A broken, unrelated test file does not stop compiling the one we want to run.
    #[test]
    fn compiles_only_what_is_needed() {
        let jdk = PathBuf::from("/usr/lib/jvm/java-21-openjdk");
        if !jdk.join("bin/javac").exists() {
            return;
        }
        let base = std::env::temp_dir().join(format!("javac-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let src = base.join("src/test/java/vn/demo");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("AppTest.java"), "package vn.demo;\npublic class AppTest { public static void main(String[] a) { System.out.println(Helper.hi()); } }\n").unwrap();
        std::fs::write(src.join("Helper.java"), "package vn.demo;\nclass Helper { static String hi() { return \"xin chào\"; } }\n").unwrap();
        std::fs::write(src.join("BrokenTest.java"), "package vn.demo;\nclass BrokenTest { void x() { int y = \"not an int\"; } }\n").unwrap();
        let roots = vec![base.join("src/test/java").display().to_string()];
        let file = source_file(&roots, "vn.demo.AppTest").unwrap();
        let out = base.join("target/test-classes");
        let mut c = javac(Some(jdk.to_str().unwrap()), &[], &roots, &out, &[file], &base.join("args.txt")).unwrap();
        let st = c.output().unwrap();
        assert!(st.status.success(), "{}", String::from_utf8_lossy(&st.stderr));
        assert!(out.join("vn/demo/AppTest.class").is_file());
        assert!(out.join("vn/demo/Helper.class").is_file()); // pulled in via -sourcepath
        assert!(!out.join("vn/demo/BrokenTest.class").exists()); // never touched
        std::fs::remove_dir_all(&base).unwrap();
    }
}
