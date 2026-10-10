//! Spring Boot configuration metadata (`META-INF/spring-configuration-metadata.json`) from
//! the module's jars and its own target/classes: key names, types, descriptions, defaults and
//! value hints - what IntelliJ completes in application.properties/yml.
use project_model::Project;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Prop {
    pub name: String,
    pub ty: String,
    pub description: String,
    pub default: Option<String>,
    pub deprecated: bool,
    /// allowed values from "hints" (value, description)
    pub values: Vec<(String, String)>,
}

const FILES: &[&str] = &["META-INF/spring-configuration-metadata.json", "META-INF/additional-spring-configuration-metadata.json"];

pub fn parse_json(text: &str, into: &mut BTreeMap<String, Prop>) {
    let Ok(v) = serde_json::from_str::<Value>(text) else { return };
    for p in v["properties"].as_array().into_iter().flatten() {
        let Some(name) = p["name"].as_str() else { continue };
        let e = into.entry(name.to_string()).or_default();
        e.name = name.to_string();
        if let Some(t) = p["type"].as_str() {
            e.ty = t.to_string();
        }
        if let Some(d) = p["description"].as_str() {
            e.description = d.to_string();
        }
        if !p["defaultValue"].is_null() {
            e.default = Some(match &p["defaultValue"] {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            });
        }
        e.deprecated |= p["deprecated"].as_bool().unwrap_or(false) || p["deprecation"].is_object();
    }
    for h in v["hints"].as_array().into_iter().flatten() {
        let Some(name) = h["name"].as_str() else { continue };
        let vals: Vec<(String, String)> = h["values"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|x| Some((x["value"].as_str().map(String::from).unwrap_or_else(|| x["value"].to_string()), x["description"].as_str().unwrap_or("").to_string())))
            .collect();
        if let Some(e) = into.get_mut(name) {
            e.values.extend(vals);
        }
    }
    // boolean keys complete true/false like IntelliJ
    for e in into.values_mut() {
        if e.values.is_empty() && (e.ty == "java.lang.Boolean" || e.ty == "boolean") {
            e.values = vec![("true".into(), String::new()), ("false".into(), String::new())];
        }
    }
}

fn read_jar(jar: &Path) -> Vec<String> {
    let Ok(f) = std::fs::File::open(jar) else { return Vec::new() };
    let Ok(mut z) = zip::ZipArchive::new(f) else { return Vec::new() };
    let mut out = Vec::new();
    for name in FILES {
        if let Ok(mut e) = z.by_name(name) {
            let mut s = String::new();
            if e.read_to_string(&mut s).is_ok() {
                out.push(s);
            }
        }
    }
    out
}

type JarCache = HashMap<PathBuf, (Option<SystemTime>, Vec<String>)>;
static CACHE: Mutex<Option<JarCache>> = Mutex::new(None);

fn cached_jar(jar: &Path) -> Vec<String> {
    let mtime = std::fs::metadata(jar).and_then(|m| m.modified()).ok();
    let mut guard = CACHE.lock().unwrap();
    let cache = guard.get_or_insert_with(HashMap::new);
    if let Some((t, v)) = cache.get(jar) {
        if *t == mtime {
            return v.clone();
        }
    }
    let v = read_jar(jar);
    cache.insert(jar.to_path_buf(), (mtime, v.clone()));
    v
}

/// Only jars that can carry Spring metadata are opened (a project has ~1000 jars).
fn may_have_metadata(jar: &str) -> bool {
    let n = Path::new(jar).file_name().and_then(|n| n.to_str()).unwrap_or("");
    n.contains("spring") || n.contains("boot") || n.contains("autoconfigure") || n.contains("starter")
}

/// Metadata of every module (jars on its classpath + its own annotation-processor output).
pub fn load(project: &Project) -> BTreeMap<String, Prop> {
    let mut all = BTreeMap::new();
    let mut jars: Vec<String> = project.modules.iter().flat_map(|m| m.classpath_jars.clone().unwrap_or_default()).filter(|j| j.ends_with(".jar") && may_have_metadata(j)).collect();
    jars.sort();
    jars.dedup();
    for j in jars {
        for text in cached_jar(Path::new(&j)) {
            parse_json(&text, &mut all);
        }
    }
    for m in &project.modules {
        for f in FILES {
            if let Ok(text) = std::fs::read_to_string(Path::new(&m.content_root).join("target/classes").join(f)) {
                parse_json(&text, &mut all);
            }
        }
    }
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_json() {
        let mut m = BTreeMap::new();
        parse_json(
            r#"{"properties":[{"name":"server.port","type":"java.lang.Integer","description":"Server HTTP port.","defaultValue":8080},
            {"name":"spring.main.lazy-initialization","type":"java.lang.Boolean"},
            {"name":"logging.level.root","type":"org.springframework.boot.logging.LogLevel","deprecation":{"level":"warning"}}],
            "hints":[{"name":"logging.level.root","values":[{"value":"info"},{"value":"debug","description":"d"}]}]}"#,
            &mut m,
        );
        assert_eq!(m["server.port"].default.as_deref(), Some("8080"));
        assert_eq!(m["spring.main.lazy-initialization"].values.len(), 2);
        assert_eq!(m["logging.level.root"].values[1], ("debug".to_string(), "d".to_string()));
        assert!(m["logging.level.root"].deprecated);
        assert!(may_have_metadata("/m2/org/springframework/boot/spring-boot-autoconfigure/3.2.0/spring-boot-autoconfigure-3.2.0.jar"));
        assert!(!may_have_metadata("/m2/commons-io/commons-io-2.11.0.jar"));
    }
}
