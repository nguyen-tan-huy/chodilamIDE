//! Shared on-disk files under `<root>/.nvim/java-debug-model/` - same schema as the
//! Lua plugin so nvim and the Rust IDE can run side by side on one project.
use crate::model::{Module, Project};
use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub type Mtimes = BTreeMap<String, i64>;

pub fn state_dir(root: &str) -> PathBuf {
    Path::new(root).join(".nvim/java-debug-model")
}

pub fn cache_path(root: &str) -> PathBuf {
    state_dir(root).join("model-cache.json")
}

/// `root|profile-a,profile-b` (sorted), like `cache_key` in maven.lua.
pub fn cache_key(root: &str, profiles: &[String]) -> String {
    let mut p = profiles.to_vec();
    p.sort();
    format!("{root}|{}", p.join(","))
}

pub fn pom_mtimes(dirs: &[String]) -> Mtimes {
    dirs.iter()
        .map(|d| {
            let pom = format!("{d}/pom.xml");
            // seconds since the epoch, like Lua's vim.uv.fs_stat().mtime.sec
            let t = std::fs::metadata(&pom)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(-1);
            (pom, t)
        })
        .collect()
}

fn load_all(root: &str) -> Map<String, Value> {
    std::fs::read_to_string(cache_path(root))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| match v {
            Value::Object(m) => Some(m),
            _ => None,
        })
        .unwrap_or_default()
}

#[derive(Deserialize)]
struct Entry {
    #[serde(default)]
    mtimes: Value,
    #[serde(default)]
    modules: Value,
}

fn parse_mtimes(v: &Value) -> Option<Mtimes> {
    match v {
        Value::Object(m) => m.iter().map(|(k, v)| v.as_i64().map(|t| (k.clone(), t))).collect(),
        _ => None,
    }
}

/// Cached project for `key` if every pom mtime still matches.
pub fn load_entry(root: &str, key: &str, current: &Mtimes) -> Option<Project> {
    let all = load_all(root);
    let entry: Entry = serde_json::from_value(all.get(key)?.clone()).ok()?;
    let mtimes = parse_mtimes(&entry.mtimes)?;
    if &mtimes != current {
        return None;
    }
    let modules: Vec<Module> = match entry.modules {
        Value::Object(ref m) if m.is_empty() => Vec::new(),
        v => serde_json::from_value(v).ok()?,
    };
    // written by a version that split Windows classpaths on ':' (jars lost their drive
    // letter): resolve again
    if modules.iter().flat_map(|m| m.classpath_jars.iter().flatten()).any(|j| !Path::new(j).is_absolute()) {
        return None;
    }
    Some(Project { root: root.into(), modules })
}

/// Writes `key`, keeping every other key already in the file. Atomic (tmp + rename).
pub fn save_entry(root: &str, key: &str, mtimes: &Mtimes, project: &Project) -> Result<()> {
    let mut all = load_all(root);
    let entry = serde_json::json!({ "mtimes": mtimes, "modules": project.modules });
    all.insert(key.into(), entry);
    write_atomic(&cache_path(root), &serde_json::to_vec(&Value::Object(all))?)
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().context("no parent dir")?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{}.tmp{}", path.file_name().unwrap().to_string_lossy(), std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// `manifest.json`: modules added/removed by hand (:JavaModelAddModule/RemoveModule).
#[derive(Debug, Default, Deserialize)]
pub struct Manifest {
    #[serde(default, deserialize_with = "list_or_empty")]
    pub added: Vec<String>,
    #[serde(default, deserialize_with = "list_or_empty")]
    pub excluded: Vec<String>,
}

fn list_or_empty<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Array(a) => a.into_iter().filter_map(|v| v.as_str().map(String::from)).collect(),
        _ => Vec::new(),
    })
}

/// Adds a module directory to manifest.json (`added`, removed from `excluded`), like
/// :JavaModelAddModule - for poms outside the project folder.
pub fn manifest_add(root: &str, module_dir: &str) -> Result<()> {
    let mut m = load_manifest(root);
    m.excluded.retain(|p| p != module_dir);
    if !m.added.iter().any(|p| p == module_dir) {
        m.added.push(module_dir.to_string());
    }
    save_manifest(root, &m)
}

/// Hides a module from the model (never touches its files), like :JavaModelRemoveModule.
pub fn manifest_remove(root: &str, module_dir: &str) -> Result<()> {
    let mut m = load_manifest(root);
    m.added.retain(|p| p != module_dir);
    if !m.excluded.iter().any(|p| p == module_dir) {
        m.excluded.push(module_dir.to_string());
    }
    save_manifest(root, &m)
}

fn save_manifest(root: &str, m: &Manifest) -> Result<()> {
    let v = serde_json::json!({ "added": m.added, "excluded": m.excluded });
    write_atomic(&state_dir(root).join("manifest.json"), &serde_json::to_vec(&v)?)
}

pub fn load_manifest(root: &str) -> Manifest {
    std::fs::read_to_string(state_dir(root).join("manifest.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}
