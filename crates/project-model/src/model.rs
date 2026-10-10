//! Plain data for the Project Model - port of java-debug-model's `model.lua`.
//! Field names match the Lua tables exactly so `model-cache.json` stays shared.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Library {
    pub group_id: String,
    pub artifact_id: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jar_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sources_jar_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SourceRoot {
    pub path: String,
    /// "main" | "test"
    pub kind: String,
    /// "java" | "resources"
    pub lang: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Dependency {
    #[serde(default)]
    pub group_id: Option<String>,
    #[serde(default)]
    pub artifact_id: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default = "default_scope")]
    pub scope: String,
    #[serde(default)]
    pub is_sibling: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sibling_module_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub library: Option<Library>,
}

fn default_scope() -> String {
    "compile".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Module {
    pub path: String,
    #[serde(default)]
    pub group_id: Option<String>,
    #[serde(default)]
    pub artifact_id: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default = "default_packaging")]
    pub packaging: String,
    #[serde(default)]
    pub content_root: String,
    #[serde(default, deserialize_with = "lua_list")]
    pub source_roots: Vec<SourceRoot>,
    #[serde(default, deserialize_with = "lua_list")]
    pub dependencies: Vec<Dependency>,
    #[serde(default, deserialize_with = "lua_list")]
    pub profiles: Vec<String>,
    #[serde(default)]
    pub in_reactor: bool,
    #[serde(rename = "_classpath_jars", default, skip_serializing_if = "Option::is_none",
            deserialize_with = "lua_opt_list")]
    pub classpath_jars: Option<Vec<String>>,
}

fn default_packaging() -> String {
    "jar".into()
}

/// vim.json.encode writes an empty Lua table as `{}`, not `[]`.
fn lua_list<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let v = serde_json::Value::deserialize(d)?;
    match v {
        serde_json::Value::Object(ref m) if m.is_empty() => Ok(Vec::new()),
        serde_json::Value::Null => Ok(Vec::new()),
        other => serde_json::from_value(other).map_err(serde::de::Error::custom),
    }
}

fn lua_opt_list<'de, D, T>(d: D) -> Result<Option<Vec<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    lua_list(d).map(Some)
}

impl Module {
    pub fn ga(&self) -> String {
        format!("{}:{}", opt(&self.group_id), opt(&self.artifact_id))
    }

    pub fn coordinates(&self) -> String {
        format!("{}:{}:{}", opt(&self.group_id), opt(&self.artifact_id), opt(&self.version))
    }
}

fn opt(s: &Option<String>) -> &str {
    s.as_deref().unwrap_or("?")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Project {
    pub root: String,
    pub modules: Vec<Module>,
}

impl Project {
    pub fn find_module_by_ga(&self, ga: &str) -> Option<&Module> {
        self.modules.iter().find(|m| m.ga() == ga)
    }

    pub fn find_module_by_path(&self, path: &str) -> Option<&Module> {
        self.modules.iter().find(|m| m.path == path)
    }

    /// Longest source-root (then content-root) prefix match, like `find_module_for_file`.
    pub fn find_module_for_file(&self, file: &str) -> Option<&Module> {
        let mut best: Option<&Module> = None;
        let mut best_len: isize = -1;
        for m in &self.modules {
            for sr in &m.source_roots {
                if file.starts_with(&sr.path) && sr.path.len() as isize > best_len {
                    best = Some(m);
                    best_len = sr.path.len() as isize;
                }
            }
            if best.is_none()
                && file.starts_with(&format!("{}/", m.content_root))
                && m.content_root.len() as isize > best_len
            {
                best = Some(m);
                best_len = m.content_root.len() as isize;
            }
        }
        best
    }
}
