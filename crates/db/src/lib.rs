//! MongoDB access for the Database window (like Navicat for MongoDB): saved connections,
//! databases / collections, a page of documents for a filter + sort, and editing them. Every
//! call blocks: the UI runs them on its background executor.
use anyhow::{Context, Result, anyhow, bail};
use bson::{Bson, Document, doc, oid::ObjectId};
use mongodb::sync::Client;
use regex::Regex;
use std::path::Path;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

pub use bson;
pub use mongodb::sync::Client as MongoClient;

/// A saved connection.
#[derive(Clone, Debug, PartialEq)]
pub struct Conn {
    pub name: String,
    pub uri: String,
}

/// Connects and checks the server answers (a ping, 5 s at most).
pub fn connect(uri: &str) -> Result<Client> {
    let mut opts = mongodb::options::ClientOptions::parse(uri).run().map_err(short)?;
    opts.server_selection_timeout = Some(Duration::from_secs(5));
    opts.connect_timeout = Some(Duration::from_secs(5));
    opts.app_name = Some("ChoDiLamIDE".into());
    let client = Client::with_options(opts).map_err(short)?;
    let db = default_db(uri).unwrap_or_else(|| "admin".into());
    client.database(&db).run_command(doc! { "ping": 1 }).run().map_err(short)?;
    Ok(client)
}

/// The database named in the URI path (`mongodb://h/shop?…` → shop).
pub fn default_db(uri: &str) -> Option<String> {
    let rest = uri.split_once("://")?.1;
    let path = rest.split_once('/')?.1;
    let name = path.split('?').next()?.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// The URI with its password hidden (titles, messages).
pub fn redact(uri: &str) -> String {
    static PW: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(://[^:/@]+:)[^@]*@").unwrap());
    PW.replace(uri, "${1}••••@").into_owned()
}

/// Driver errors are long; keep the first sentence that says something.
fn short(e: mongodb::error::Error) -> anyhow::Error {
    let s = e.kind.to_string();
    let s = s.split(", Topology:").next().unwrap_or(&s).trim().to_string();
    anyhow!(s)
}

/// Database names with their size on disk; when the user may not list them, the URI's database.
pub fn databases(client: &Client, uri: &str) -> Result<Vec<(String, u64)>> {
    match client.list_databases().run() {
        Ok(list) => Ok(list.into_iter().map(|d| (d.name, d.size_on_disk)).collect()),
        Err(e) => match default_db(uri) {
            Some(db) => Ok(vec![(db, 0)]),
            None => Err(short(e)),
        },
    }
}

pub fn collections(client: &Client, db: &str) -> Result<Vec<String>> {
    let mut v = client.database(db).list_collection_names().run().map_err(short)?;
    v.sort_by_key(|s| s.to_lowercase());
    Ok(v)
}

/// One page of a collection.
#[derive(Clone, Debug, Default)]
pub struct Page {
    pub docs: Vec<Document>,
    /// documents matching the filter
    pub total: u64,
    pub millis: u128,
}

pub fn find(client: &Client, db: &str, coll: &str, filter: &str, sort: &str, skip: u64, limit: i64) -> Result<Page> {
    let filter = parse_doc(filter).context("Filter")?;
    let sort = parse_doc(sort).context("Sort")?;
    let c = client.database(db).collection::<Document>(coll);
    let t = Instant::now();
    let mut find = c.find(filter.clone()).skip(skip).limit(limit);
    if !sort.is_empty() {
        find = find.sort(sort);
    }
    let docs = find.run().map_err(short)?.collect::<std::result::Result<Vec<_>, _>>().map_err(short)?;
    let total = if filter.is_empty() { c.estimated_document_count().run() } else { c.count_documents(filter).run() }.map_err(short)?;
    Ok(Page { docs, total, millis: t.elapsed().as_millis() })
}

/// `$set` one top-level field of the document with this `_id` (the value as JSON).
pub fn set_field(client: &Client, db: &str, coll: &str, id: &Bson, key: &str, value: &str) -> Result<()> {
    let v = parse_value(value)?;
    let r = client.database(db).collection::<Document>(coll).update_one(doc! { "_id": id.clone() }, doc! { "$set": { key: v } }).run().map_err(short)?;
    if r.matched_count == 0 {
        bail!("Không còn document này (đã bị xóa?)");
    }
    Ok(())
}

/// Replaces the whole document with this `_id` by `json` (its `_id` kept).
pub fn replace(client: &Client, db: &str, coll: &str, id: &Bson, json: &str) -> Result<()> {
    let mut d = parse_doc(json)?;
    d.insert("_id", id.clone());
    let r = client.database(db).collection::<Document>(coll).replace_one(doc! { "_id": id.clone() }, d).run().map_err(short)?;
    if r.matched_count == 0 {
        bail!("Không còn document này (đã bị xóa?)");
    }
    Ok(())
}

pub fn insert(client: &Client, db: &str, coll: &str, json: &str) -> Result<Bson> {
    let d = parse_doc(json)?;
    Ok(client.database(db).collection::<Document>(coll).insert_one(d).run().map_err(short)?.inserted_id)
}

pub fn delete(client: &Client, db: &str, coll: &str, ids: &[Bson]) -> Result<u64> {
    let r = client.database(db).collection::<Document>(coll).delete_many(doc! { "_id": { "$in": ids.to_vec() } }).run().map_err(short)?;
    Ok(r.deleted_count)
}

pub fn create_collection(client: &Client, db: &str, name: &str) -> Result<()> {
    client.database(db).create_collection(name).run().map_err(short)
}

pub fn drop_collection(client: &Client, db: &str, name: &str) -> Result<()> {
    client.database(db).collection::<Document>(name).drop().run().map_err(short)
}

/// Index names and keys of a collection.
pub fn indexes(client: &Client, db: &str, coll: &str) -> Result<Vec<(String, String)>> {
    let c = client.database(db).collection::<Document>(coll);
    let list = c.list_indexes().run().map_err(short)?;
    Ok(list
        .filter_map(|i| i.ok())
        .map(|i| (i.options.and_then(|o| o.name).unwrap_or_default(), compact(&Bson::Document(i.keys))))
        .collect())
}

// ---- JSON ↔ BSON ------------------------------------------------------------------------

/// A filter / sort / document as typed: (extended) JSON, or mongo shell style with
/// unquoted keys, 'single quotes', ObjectId("…"), ISODate("…"). Empty = {}.
pub fn parse_doc(s: &str) -> Result<Document> {
    if s.trim().is_empty() {
        return Ok(Document::new());
    }
    match parse_value(s)? {
        Bson::Document(d) => Ok(d),
        _ => bail!("cần một object {{ … }}"),
    }
}

pub fn parse_value(s: &str) -> Result<Bson> {
    let v: serde_json::Value = match serde_json::from_str(s) {
        Ok(v) => v,
        Err(e) => serde_json::from_str(&shell_to_json(s)).map_err(|_| anyhow!("JSON không hợp lệ: {e}"))?,
    };
    Bson::try_from(v).map_err(|e| anyhow!("{e}"))
}

/// Mongo shell syntax → JSON (good enough for filters typed by hand).
fn shell_to_json(s: &str) -> String {
    static OID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"ObjectId\(\s*["']([0-9a-fA-F]{24})["']\s*\)"#).unwrap());
    static DATE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?:ISODate|new Date)\(\s*["']([^"']+)["']\s*\)"#).unwrap());
    static LONG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?:NumberLong|Long)\(\s*["']?(-?\d+)["']?\s*\)"#).unwrap());
    static INT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?:NumberInt|Int32)\(\s*["']?(-?\d+)["']?\s*\)"#).unwrap());
    static SQ: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"'([^'\\]*)'"#).unwrap());
    static KEY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"([{,]\s*)([A-Za-z_$][\w.$]*)\s*:"#).unwrap());
    let s = OID.replace_all(s, r#"{"$$oid":"$1"}"#);
    let s = DATE.replace_all(&s, r#"{"$$date":"$1"}"#);
    let s = LONG.replace_all(&s, r#"{"$$numberLong":"$1"}"#);
    let s = INT.replace_all(&s, "$1");
    let s = SQ.replace_all(&s, r#""$1""#);
    KEY.replace_all(&s, r#"$1"$2":"#).into_owned()
}

/// One line of relaxed extended JSON.
pub fn compact(v: &Bson) -> String {
    v.clone().into_relaxed_extjson().to_string()
}

/// Indented relaxed extended JSON (the document panel, editing).
pub fn pretty(d: &Document) -> String {
    serde_json::to_string_pretty(&Bson::Document(d.clone()).into_relaxed_extjson()).unwrap_or_default()
}

/// A value as a grid cell: short and readable, nested values summarized.
pub fn cell(v: &Bson) -> String {
    match v {
        Bson::String(s) => s.replace('\n', " "),
        Bson::ObjectId(o) => o.to_hex(),
        Bson::DateTime(d) => d.try_to_rfc3339_string().map(|s| s.replace('T', " ").trim_end_matches('Z').to_string()).unwrap_or_else(|_| d.to_string()),
        Bson::Int32(n) => n.to_string(),
        Bson::Int64(n) => n.to_string(),
        Bson::Double(n) => n.to_string(),
        Bson::Boolean(b) => b.to_string(),
        Bson::Null => "null".into(),
        Bson::Document(d) => format!("{{ {} }}", d.keys().take(4).cloned().collect::<Vec<_>>().join(", ") + if d.len() > 4 { ", …" } else { "" }),
        Bson::Array(a) => format!("[ {} phần tử ]", a.len()),
        Bson::Decimal128(d) => d.to_string(),
        other => compact(other),
    }
}

/// The kind of a value (column hints, the document tree).
pub fn kind(v: &Bson) -> &'static str {
    match v {
        Bson::String(_) => "String",
        Bson::ObjectId(_) => "ObjectId",
        Bson::DateTime(_) => "Date",
        Bson::Int32(_) => "Int32",
        Bson::Int64(_) => "Int64",
        Bson::Double(_) => "Double",
        Bson::Decimal128(_) => "Decimal128",
        Bson::Boolean(_) => "Boolean",
        Bson::Null => "Null",
        Bson::Document(_) => "Object",
        Bson::Array(_) => "Array",
        _ => "Khác",
    }
}

/// Top-level keys of the documents, `_id` first then in order of first appearance (columns).
pub fn columns(docs: &[Document]) -> Vec<String> {
    let mut cols: Vec<String> = vec!["_id".into()];
    for d in docs {
        for k in d.keys() {
            if !cols.contains(k) {
                cols.push(k.clone());
            }
        }
    }
    if !docs.iter().any(|d| d.contains_key("_id")) {
        cols.remove(0);
    }
    cols
}

/// A new ObjectId as typed in a filter (handy for "insert").
pub fn new_id() -> String {
    ObjectId::new().to_hex()
}

// ---- project hints ------------------------------------------------------------------------

/// `mongodb://…` URIs written in the project's config files (application.properties / .yml,
/// .env…): offered as connections.
pub fn uris_in_project(root: &Path) -> Vec<String> {
    static URI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"mongodb(?:\+srv)?://[^\s"'`,;<>]+"#).unwrap());
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if p.is_dir() {
                if depth < 6 && !name.starts_with('.') && !matches!(name.as_str(), "target" | "node_modules" | "build" | "out") {
                    stack.push((p, depth + 1));
                }
            } else if name.ends_with(".properties") || name.ends_with(".yml") || name.ends_with(".yaml") || name.starts_with(".env") {
                let Ok(text) = std::fs::read_to_string(&p) else { continue };
                for m in URI.find_iter(&text) {
                    let u = m.as_str().trim_end_matches(['}', ')']).to_string();
                    // ${VAR} placeholders cannot be used as is
                    if !u.contains("${") && !out.contains(&u) {
                        out.push(u);
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_syntax() {
        let d = parse_doc(r#"{status: 'ACTIVE', _id: ObjectId("650c1f6a2b9e4c0012345678"), n: {$gt: NumberLong(5)}}"#).unwrap();
        assert_eq!(d.get_str("status").unwrap(), "ACTIVE");
        assert!(matches!(d.get("_id"), Some(Bson::ObjectId(_))));
        assert_eq!(d.get_document("n").unwrap().get("$gt"), Some(&Bson::Int64(5)));
        assert!(parse_doc("").unwrap().is_empty());
        assert!(parse_doc("[1]").is_err());
    }

    #[test]
    fn uri_parts() {
        assert_eq!(default_db("mongodb://u:p@h:27017/shop?authSource=admin").as_deref(), Some("shop"));
        assert_eq!(default_db("mongodb://h:27017/"), None);
        assert_eq!(redact("mongodb://u:secret@h/x"), "mongodb://u:••••@h/x");
    }

    #[test]
    fn columns_id_first() {
        let docs = vec![doc! { "a": 1, "_id": 1 }, doc! { "b": 2, "_id": 2 }];
        assert_eq!(columns(&docs), vec!["_id", "a", "b"]);
    }
}
