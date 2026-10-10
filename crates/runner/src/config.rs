//! `.nvim/java-debug-model/debug-configs.json` (list of configs) and `active-config.json`
//! (`{"name": ...}`) - same files and field names as java-debug-model's config_store.
use anyhow::Result;
use project_model::cache::{state_dir, write_atomic};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct RunConfig {
    pub name: String,
    pub main_class: String,
    pub module_path: String,
    #[serde(default)]
    pub working_directory: String,
    #[serde(default)]
    pub vm_args: String,
    #[serde(default)]
    pub program_args: String,
    /// Lua writes an empty map as `[]`: accept both.
    #[serde(default, deserialize_with = "map_or_empty")]
    pub env_vars: BTreeMap<String, String>,
    #[serde(default)]
    pub maven_profiles: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jdk_path: Option<String>,
    /// Fields written by newer java-debug-model versions: kept untouched.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn map_or_empty<'de, D: serde::Deserializer<'de>>(d: D) -> Result<BTreeMap<String, String>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Object(m) => m.into_iter().map(|(k, v)| (k, v.as_str().map(String::from).unwrap_or_else(|| v.to_string()))).collect(),
        _ => BTreeMap::new(),
    })
}

fn configs_path(root: &str) -> std::path::PathBuf {
    state_dir(root).join("debug-configs.json")
}

pub fn load(root: &str) -> Vec<RunConfig> {
    std::fs::read_to_string(configs_path(root)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn save(root: &str, configs: &[RunConfig]) -> Result<()> {
    write_atomic(&configs_path(root), &serde_json::to_vec(configs)?)
}

pub fn active(root: &str) -> Option<String> {
    let s = std::fs::read_to_string(state_dir(root).join("active-config.json")).ok()?;
    serde_json::from_str::<Value>(&s).ok()?["name"].as_str().map(String::from)
}

pub fn set_active(root: &str, name: &str) -> Result<()> {
    write_atomic(&state_dir(root).join("active-config.json"), &serde_json::to_vec(&serde_json::json!({ "name": name }))?)
}

/// Config for a main class, as java-debug-model's auto-create does.
pub fn for_main_class(module_path: &str, main_class: &str) -> RunConfig {
    let simple = main_class.rsplit('.').next().unwrap_or(main_class).to_string();
    RunConfig {
        name: simple,
        main_class: main_class.into(),
        module_path: module_path.into(),
        working_directory: module_path.into(),
        ..Default::default()
    }
}

/// Sets (Some) or clears (None = system default) the JDK Maven runs on - the same
/// maven-jdk.json java-debug-model's :JavaMavenSelectJdk writes.
pub fn set_maven_jdk(root: &str, jdk_path: Option<&str>) -> Result<()> {
    write_atomic(&state_dir(root).join("maven-jdk.json"), &serde_json::to_vec(&serde_json::json!({ "jdk_path": jdk_path }))?)
}

/// IDE-side Maven options per project (profiles, skip tests) - chodilamide-maven.json.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MavenPrefs {
    #[serde(default)]
    pub profiles: Vec<String>,
    #[serde(default)]
    pub skip_tests: bool,
}

pub fn maven_prefs(root: &str) -> MavenPrefs {
    std::fs::read_to_string(state_dir(root).join("chodilamide-maven.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn set_maven_prefs(root: &str, prefs: &MavenPrefs) -> Result<()> {
    write_atomic(&state_dir(root).join("chodilamide-maven.json"), &serde_json::to_vec(prefs)?)
}

/// `root/.nvim/java-debug-model/maven-jdk.json` → JDK used to run Maven.
pub fn maven_jdk(root: &str) -> Option<String> {
    let s = std::fs::read_to_string(state_dir(root).join("maven-jdk.json")).ok()?;
    serde_json::from_str::<Value>(&s).ok()?["jdk_path"].as_str().map(String::from)
}

/// JDK for Maven/Run/Debug of a project: its maven-jdk.json, else the app-wide default
/// (Settings → Java & Maven), else None = JAVA_HOME / PATH.
pub fn project_jdk(root: &str) -> Option<String> {
    maven_jdk(root).or_else(|| toolchain::managed::default_jdk().map(|p| p.display().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_keeps_lua_fields() {
        let root = std::env::temp_dir().join(format!("cfg-test-{}", std::process::id()));
        let root_s = root.display().to_string();
        std::fs::create_dir_all(state_dir(&root_s)).unwrap();
        // exactly what java-debug-model writes (empty env as [], an unknown field)
        let lua = r#"[{"program_args":"","main_class":"a.b.App","vm_args":"-Dx=1","name":"App","module_path":"/p/m","working_directory":"/p/m","maven_profiles":["dev"],"env_vars":[],"future_field":42},
                      {"name":"Web","main_class":"a.W","module_path":"/p/w","env_vars":{"TZ":"Asia/Ho_Chi_Minh"},"jdk_path":"/usr/lib/jvm/java-11-openjdk"}]"#;
        std::fs::write(configs_path(&root_s), lua).unwrap();
        let cfgs = load(&root_s);
        assert_eq!(cfgs.len(), 2);
        assert_eq!(cfgs[0].maven_profiles, vec!["dev"]);
        assert!(cfgs[0].env_vars.is_empty());
        assert_eq!(cfgs[1].env_vars["TZ"], "Asia/Ho_Chi_Minh");
        assert_eq!(cfgs[1].jdk_path.as_deref(), Some("/usr/lib/jvm/java-11-openjdk"));
        save(&root_s, &cfgs).unwrap();
        let again = load(&root_s);
        assert_eq!(again, cfgs);
        let raw: Value = serde_json::from_str(&std::fs::read_to_string(configs_path(&root_s)).unwrap()).unwrap();
        assert_eq!(raw[0]["future_field"], 42);
        set_active(&root_s, "Web").unwrap();
        assert_eq!(active(&root_s).as_deref(), Some("Web"));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
