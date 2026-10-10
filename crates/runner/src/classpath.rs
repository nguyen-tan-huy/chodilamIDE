//! Launch classpath / sourcepath from the project model - port of jdtls.lua's
//! resolve_classpath/resolve_sourcepaths. Sibling modules contribute their live
//! `target/classes` (not the possibly stale jar in ~/.m2), which is what makes
//! cross-module debugging and hot swap work.
use project_model::{Module, Project};

fn jar_artifact(jar: &str) -> Option<&str> {
    project_model::pom::jar_artifact_version(jar).map(|(a, _)| a)
}

fn module_by_artifact<'a>(p: &'a Project, artifact: &str) -> Option<&'a Module> {
    p.modules.iter().find(|m| m.artifact_id.as_deref() == Some(artifact))
}

pub fn classpath(project: &Project, module: &Module, include_test: bool) -> Vec<String> {
    let mut paths = vec![format!("{}/target/classes", module.path)];
    if include_test {
        paths.push(format!("{}/target/test-classes", module.path));
    }
    // test-scoped jars stay off a normal launch classpath
    let test_only: Vec<&str> = if include_test {
        Vec::new()
    } else {
        module.dependencies.iter().filter(|d| d.scope == "test" && !d.is_sibling).filter_map(|d| d.artifact_id.as_deref()).collect()
    };
    for jar in module.classpath_jars.as_deref().unwrap_or_default() {
        let artifact = jar_artifact(jar);
        if artifact.is_some_and(|a| test_only.contains(&a)) {
            continue;
        }
        match artifact.and_then(|a| module_by_artifact(project, a)) {
            Some(sib) if sib.path != module.path => paths.push(format!("{}/target/classes", sib.path)),
            _ => paths.push(jar.clone()),
        }
    }
    let mut seen = std::collections::HashSet::new();
    paths.retain(|p| seen.insert(p.clone()));
    paths
}

/// True when `main_class` is defined under one of the module's test source roots
/// (e.g. a `main` in src/test/java) - it then needs target/test-classes and test jars.
pub fn main_in_test_sources(module: &Module, main_class: &str) -> bool {
    let rel = format!("{}.java", main_class.split('$').next().unwrap_or(main_class).replace('.', "/"));
    module.source_roots.iter().any(|s| s.kind == "test" && std::path::Path::new(&s.path).join(&rel).is_file())
}

pub fn sourcepaths(project: &Project, module: &Module, include_test: bool) -> Vec<String> {
    let mut paths: Vec<String> = module.source_roots.iter().filter(|s| s.kind == "main" || include_test).map(|s| s.path.clone()).collect();
    let mut seen = vec![module.path.clone()];
    for jar in module.classpath_jars.as_deref().unwrap_or_default() {
        if let Some(sib) = jar_artifact(jar).and_then(|a| module_by_artifact(project, a)) {
            if !seen.contains(&sib.path) {
                seen.push(sib.path.clone());
                paths.extend(sib.source_roots.iter().filter(|s| s.kind == "main").map(|s| s.path.clone()));
            }
        }
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;
    use project_model::{Dependency, SourceRoot};

    fn module(path: &str, artifact: &str, jars: Vec<&str>, deps: Vec<Dependency>) -> Module {
        Module {
            path: path.into(),
            group_id: Some("g".into()),
            artifact_id: Some(artifact.into()),
            version: Some("1".into()),
            packaging: "jar".into(),
            content_root: path.into(),
            source_roots: vec![SourceRoot { path: format!("{path}/src/main/java"), kind: "main".into(), lang: "java".into() }],
            dependencies: deps,
            profiles: vec![],
            in_reactor: false,
            classpath_jars: Some(jars.into_iter().map(String::from).collect()),
        }
    }

    #[test]
    fn sibling_output_replaces_m2_jar_and_test_jars_dropped() {
        let junit = Dependency { group_id: Some("org.junit".into()), artifact_id: Some("junit".into()), version: Some("4".into()), scope: "test".into(), is_sibling: false, sibling_module_path: None, library: None };
        let web = module("/p/web", "web", vec!["/m2/g/core/1/core-1.jar", "/m2/org/junit/junit/4/junit-4.jar", "/m2/x/lib/2/lib-2.jar"], vec![junit]);
        let core = module("/p/core", "core", vec![], vec![]);
        let p = Project { root: "/p".into(), modules: vec![web.clone(), core] };
        assert_eq!(classpath(&p, &web, false), ["/p/web/target/classes", "/p/core/target/classes", "/m2/x/lib/2/lib-2.jar"]);
        assert!(classpath(&p, &web, true).contains(&"/m2/org/junit/junit/4/junit-4.jar".to_string()));
        assert_eq!(sourcepaths(&p, &web, false), ["/p/web/src/main/java", "/p/core/src/main/java"]);
    }

    #[test]
    fn main_class_in_test_sources() {
        let dir = std::env::temp_dir().join(format!("cp-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src/test/java/vn/longvan")).unwrap();
        std::fs::write(dir.join("src/test/java/vn/longvan/AppTest.java"), "class AppTest { public static void main(String[] a) {} }").unwrap();
        let mut m = module(&dir.display().to_string(), "m", vec![], vec![]);
        m.source_roots.push(SourceRoot { path: dir.join("src/test/java").display().to_string(), kind: "test".into(), lang: "java".into() });
        assert!(main_in_test_sources(&m, "vn.longvan.AppTest"));
        assert!(!main_in_test_sources(&m, "vn.longvan.App"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
