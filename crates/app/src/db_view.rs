//! Database window (Navicat for MongoDB): saved connections on the left (connection →
//! databases → collections), opened collections as tabs, each a grid of documents with a
//! filter / sort, paging, inline cell editing, insert / delete, and the selected document as
//! JSON on the right. Connections live in `~/.config/chodilamide/db.json`; `mongodb://` URIs
//! found in the project's config files are offered as ready-made ones.
use crate::dialogs::{self, DialogCancel, DialogConfirm, DialogNext, Field};
use crate::git_style::icon_btn;
use crate::theme::{self, c};
use crate::ui_icons::{Ico, icon, tip};
use crate::workspace::Workspace;
use db::{Conn, MongoClient, Page, bson::Bson, bson::Document};
use gpui::{
    AnyElement, Context, FocusHandle, Focusable, MouseButton, MouseDownEvent, SharedString, UniformListScrollHandle, Window, actions, div, prelude::*, px, uniform_list,
};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

actions!(db_view, [DbOpenFirst, DbToggle, DbApply, DbCommitEdit, DbCancelEdit, DbClose, DbDeleteRows, DbRefresh]);

const ROW_H: f32 = 26.;
const COL_W: f32 = 180.;
const ID_W: f32 = 210.;
const NUM_W: f32 = 52.;
const PAGE_SIZES: [i64; 4] = [50, 100, 200, 500];

pub fn bind_keys(cx: &mut gpui::App) {
    use gpui::KeyBinding;
    cx.bind_keys([
        KeyBinding::new("alt-7", DbToggle, None),
        KeyBinding::new("escape", DbClose, Some("DbView")),
        KeyBinding::new("f5", DbRefresh, Some("DbView")),
        KeyBinding::new("ctrl-r", DbRefresh, Some("DbView")),
        KeyBinding::new("delete", DbDeleteRows, Some("DbGrid")),
        KeyBinding::new("enter", DbApply, Some("DbQuery")),
        KeyBinding::new("enter", DbOpenFirst, Some("DbSearch")),
        KeyBinding::new("enter", DbCommitEdit, Some("DbCell")),
        KeyBinding::new("escape", DbCancelEdit, Some("DbCell")),
    ]);
}

#[derive(Clone, PartialEq)]
enum Status {
    Idle,
    Connecting,
    Ok,
    Err(String),
}

struct DbConn {
    conn: Conn,
    client: Option<MongoClient>,
    status: Status,
    open: bool,
    dbs: Vec<DbNode>,
}

struct DbNode {
    name: String,
    size: u64,
    open: bool,
    /// None = not loaded yet
    colls: Option<Vec<String>>,
}

/// An opened collection.
struct CollTab {
    id: u64,
    conn: usize,
    db: String,
    coll: String,
    filter: Field,
    sort: Field,
    skip: u64,
    limit: i64,
    page: Page,
    /// what the grid / JSON views draw, computed once per load (off the UI thread)
    grid: Arc<GridData>,
    loading: bool,
    error: Option<String>,
    selected: BTreeSet<usize>,
    cursor: Option<usize>,
    /// (row, column, value field) being edited in the grid
    edit: Option<(usize, String, Field)>,
    json: bool,
    scroll: UniformListScrollHandle,
}

/// One line of pretty JSON, already split and colored.
#[derive(Clone)]
struct JLine {
    indent: f32,
    key: Option<SharedString>,
    rest: SharedString,
    color: u32,
    /// the document it belongs to
    doc: usize,
}

/// The grid and JSON views of a page: rendering only copies what is visible.
#[derive(Clone, Default)]
struct GridData {
    cols: Vec<String>,
    titles: Vec<SharedString>,
    kinds: Vec<&'static str>,
    widths: Vec<f32>,
    total_w: f32,
    /// per row, per column: text, color, missing
    cells: Vec<Vec<(SharedString, u32, bool)>>,
    json: Vec<JLine>,
    /// each document's lines in `json`
    doc_lines: Vec<std::ops::Range<usize>>,
}

fn build_grid(docs: &[Document]) -> GridData {
    let cols = db::columns(docs);
    let widths: Vec<f32> = cols.iter().map(|k| if k == "_id" { ID_W } else { COL_W }).collect();
    let kinds = cols.iter().map(|k| docs.iter().find_map(|d| d.get(k)).map(db::kind).unwrap_or("")).collect();
    let cells = docs.iter().map(|d| row_cells(d, &cols)).collect();
    let mut json = Vec::new();
    let mut doc_lines = Vec::new();
    for (i, d) in docs.iter().enumerate() {
        let start = json.len();
        json.extend(json_lines(&db::pretty(d), i));
        doc_lines.push(start..json.len());
    }
    GridData { titles: cols.iter().map(|k| SharedString::from(k.clone())).collect(), total_w: NUM_W + widths.iter().sum::<f32>(), cols, kinds, widths, cells, json, doc_lines }
}

fn row_cells(d: &Document, cols: &[String]) -> Vec<(SharedString, u32, bool)> {
    cols.iter()
        .map(|k| {
            let v = d.get(k);
            let mut text = v.map(db::cell).unwrap_or_else(|| "—".into());
            // a cell shows one short line: never shape a huge string
            if text.len() > 200 {
                let mut cut = 200;
                while !text.is_char_boundary(cut) {
                    cut -= 1;
                }
                text.truncate(cut);
                text.push('…');
            }
            (SharedString::from(text), value_color(v), v.is_none())
        })
        .collect()
}

enum DbDialog {
    /// new (None) or edit a connection
    Conn { edit: Option<usize>, name: Field, uri: Field, test: Option<Result<String, String>>, testing: bool },
    NewColl { conn: usize, db: String, name: Field },
    Confirm { title: String, body: String, op: DbOp },
    /// whole document in a JSON editor
    Doc { title: String, path: PathBuf, view: gpui::Entity<crate::editor_view::EditorView> },
}

#[derive(Clone)]
enum DbOp {
    DropColl(usize, String, String),
    DeleteDocs(Vec<Bson>),
    RemoveConn(usize),
}

pub struct DbView {
    conns: Vec<DbConn>,
    tabs: Vec<CollTab>,
    active: Option<usize>,
    next_id: u64,
    /// URIs found in the project that are not saved yet
    hints: Vec<String>,
    /// JSON files opened in the editor for a document
    edits: std::collections::HashMap<PathBuf, DocEdit>,
    /// quick search over every loaded collection name
    search: Field,
    dialog: Option<DbDialog>,
    error: Option<String>,
    focus: FocusHandle,
    needs_focus: bool,
}

/// Where a document lands when it is edited in the editor.
struct DocEdit {
    conn: usize,
    db: String,
    coll: String,
    /// None = a new document (inserted on the first save)
    id: Option<Bson>,
    tab: u64,
}

fn edit_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).unwrap_or_default()).join(".cache"));
    base.join("chodilamide/db")
}

fn store_path() -> PathBuf {
    crate::settings::Settings::path().with_file_name("db.json")
}

fn load_conns() -> Vec<Conn> {
    let Ok(text) = std::fs::read_to_string(store_path()) else { return Vec::new() };
    let Ok(serde_json::Value::Array(list)) = serde_json::from_str::<serde_json::Value>(&text) else { return Vec::new() };
    list.iter()
        .filter_map(|v| Some(Conn { name: v.get("name")?.as_str()?.to_string(), uri: v.get("uri")?.as_str()?.to_string() }))
        .collect()
}

fn save_conns(conns: &[Conn]) -> std::io::Result<()> {
    let list: Vec<serde_json::Value> = conns.iter().map(|c| serde_json::json!({ "name": c.name, "uri": c.uri })).collect();
    let p = store_path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(&p, serde_json::to_string_pretty(&list).unwrap_or_default())?;
    // the URIs carry passwords
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// "mongodb://u:p@db.example.com:27017/shop" → "shop @ db.example.com".
fn conn_name(uri: &str) -> String {
    let host = uri.split_once("://").map(|x| x.1).unwrap_or(uri);
    let host = host.rsplit_once('@').map(|x| x.1).unwrap_or(host);
    let host = host.split(['/', '?', ',']).next().unwrap_or(host);
    let host = host.strip_suffix(":27017").unwrap_or(host);
    match db::default_db(uri) {
        Some(d) => format!("{d} @ {host}"),
        None => host.to_string(),
    }
}

fn human_size(b: u64) -> String {
    match b {
        0 => String::new(),
        b if b < 1 << 20 => format!("{} KB", b >> 10),
        b if b < 1 << 30 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
    }
}

fn tint(color: u32, a: f32) -> gpui::Hsla {
    let mut h = c(color);
    h.a = a;
    h
}

/// The color of a value in the grid by its type.
fn value_color(v: Option<&Bson>) -> u32 {
    match v {
        None | Some(Bson::Null) => theme::FG_DIM,
        Some(Bson::String(_)) => theme::FG,
        Some(Bson::ObjectId(_)) => theme::GIT_MODIFIED,
        Some(Bson::Int32(_) | Bson::Int64(_) | Bson::Double(_) | Bson::Decimal128(_)) => theme::WARNING,
        Some(Bson::Boolean(_)) => theme::ACCENT,
        Some(Bson::DateTime(_)) => theme::GIT_ADDED,
        Some(_) => theme::FG_DIM,
    }
}

impl Workspace {
    pub(crate) fn toggle_db(&mut self, cx: &mut Context<Self>) {
        if self.db_open {
            self.db_open = false;
            return cx.notify();
        }
        if self.db.is_none() {
            let conns: Vec<DbConn> = load_conns().into_iter().map(|conn| DbConn { conn, client: None, status: Status::Idle, open: false, dbs: Vec::new() }).collect();
            let search = Field::new("Tìm collection", "Tìm collection…", "", cx);
            search.input.update(cx, |i, _| i.bare = true);
            cx.subscribe(&search.input, |_, _, _: &crate::input::InputEvent, cx| cx.notify()).detach();
            self.db = Some(DbView { conns, tabs: Vec::new(), active: None, next_id: 1, hints: Vec::new(), edits: Default::default(), search, dialog: None, error: None, focus: cx.focus_handle(), needs_focus: true });
            let root = self.root.clone();
            self.db_run(move || db::uris_in_project(&root), |ws, uris, _| {
                if let Some(v) = ws.db.as_mut() {
                    v.hints = uris.into_iter().filter(|u| !v.conns.iter().any(|c| &c.conn.uri == u)).collect();
                }
            }, cx);
        }
        if let Some(v) = self.db.as_mut() {
            v.needs_focus = true;
        }
        self.db_open = true;
        cx.notify();
    }

    /// Runs a blocking database call in the background, then `done` on the UI thread.
    fn db_run<T: Send + 'static>(&mut self, job: impl FnOnce() -> T + Send + 'static, done: impl FnOnce(&mut Workspace, T, &mut Context<Workspace>) + 'static, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { job() }).await;
            let _ = this.update(cx, |ws, cx| {
                done(ws, r, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn db_conn_toggle(&mut self, ci: usize, cx: &mut Context<Self>) {
        let Some(v) = self.db.as_mut() else { return };
        let Some(dc) = v.conns.get_mut(ci) else { return };
        if dc.status == Status::Ok {
            dc.open = !dc.open;
            return cx.notify();
        }
        self.db_connect(ci, cx);
    }

    fn db_connect(&mut self, ci: usize, cx: &mut Context<Self>) {
        let Some(dc) = self.db.as_mut().and_then(|v| v.conns.get_mut(ci)) else { return };
        if dc.status == Status::Connecting {
            return;
        }
        dc.status = Status::Connecting;
        let uri = dc.conn.uri.clone();
        cx.notify();
        self.db_run(
            move || {
                let client = db::connect(&uri)?;
                let dbs = db::databases(&client, &uri)?;
                anyhow::Ok((client, dbs))
            },
            move |ws, r, cx| {
                let Some(dc) = ws.db.as_mut().and_then(|v| v.conns.get_mut(ci)) else { return };
                match r {
                    Ok((client, dbs)) => {
                        // a database named in the URI opens by itself
                        let want = db::default_db(&dc.conn.uri);
                        let old: Vec<String> = dc.dbs.iter().filter(|d| d.open).map(|d| d.name.clone()).collect();
                        dc.dbs = dbs.into_iter().map(|(name, size)| DbNode { open: old.contains(&name), name, size, colls: None }).collect();
                        dc.client = Some(client);
                        dc.status = Status::Ok;
                        dc.open = true;
                        let open: Vec<usize> = dc.dbs.iter().enumerate().filter(|(_, d)| d.open || Some(&d.name) == want.as_ref()).map(|(i, _)| i).collect();
                        for di in open {
                            ws.db_load_colls(ci, di, cx);
                        }
                    }
                    Err(e) => {
                        dc.status = Status::Err(e.to_string());
                        dc.client = None;
                    }
                }
            },
            cx,
        );
    }

    fn db_load_colls(&mut self, ci: usize, di: usize, cx: &mut Context<Self>) {
        let Some(dc) = self.db.as_mut().and_then(|v| v.conns.get_mut(ci)) else { return };
        let (Some(client), Some(node)) = (dc.client.clone(), dc.dbs.get_mut(di)) else { return };
        node.open = true;
        let name = node.name.clone();
        self.db_run(move || db::collections(&client, &name), move |ws, r, _| {
            let Some(v) = ws.db.as_mut() else { return };
            match r {
                Ok(colls) => {
                    if let Some(n) = v.conns.get_mut(ci).and_then(|d| d.dbs.get_mut(di)) {
                        n.colls = Some(colls);
                    }
                }
                Err(e) => v.error = Some(e.to_string()),
            }
        }, cx);
    }

    fn db_toggle_db(&mut self, ci: usize, di: usize, cx: &mut Context<Self>) {
        let Some(n) = self.db.as_mut().and_then(|v| v.conns.get_mut(ci)).and_then(|d| d.dbs.get_mut(di)) else { return };
        if n.open {
            n.open = false;
            return cx.notify();
        }
        if n.colls.is_some() {
            n.open = true;
            return cx.notify();
        }
        self.db_load_colls(ci, di, cx);
    }

    fn db_open_coll(&mut self, ci: usize, db_name: String, coll: String, cx: &mut Context<Self>) {
        let Some(v) = self.db.as_mut() else { return };
        if let Some(i) = v.tabs.iter().position(|t| t.conn == ci && t.db == db_name && t.coll == coll) {
            v.active = Some(i);
            return cx.notify();
        }
        let id = v.next_id;
        v.next_id += 1;
        let filter = Field::new("Filter", "{ status: 'ACTIVE' }   ·   Enter để lọc", "", cx);
        let sort = Field::new("Sort", "{ _id: -1 }", "", cx);
        let v = self.db.as_mut().unwrap();
        v.tabs.push(CollTab {
            id,
            conn: ci,
            db: db_name,
            coll,
            filter,
            sort,
            skip: 0,
            limit: 100,
            page: Page::default(),
            grid: Arc::default(),
            loading: true,
            error: None,
            selected: BTreeSet::new(),
            cursor: None,
            edit: None,
            json: false,
            scroll: UniformListScrollHandle::new(),
        });
        v.active = Some(v.tabs.len() - 1);
        self.db_reload(cx);
    }

    /// Enter in the collection search: open the first match.
    fn db_open_first_match(&mut self, cx: &mut Context<Self>) {
        let Some(v) = self.db.as_ref() else { return };
        let q = v.search.text(cx).to_lowercase();
        if q.is_empty() {
            return;
        }
        let hit = v.conns.iter().enumerate().filter(|(_, d)| d.open && d.status == Status::Ok).find_map(|(ci, d)| {
            d.dbs.iter().find_map(|n| n.colls.iter().flatten().find(|c| c.to_lowercase().contains(&q)).map(|c| (ci, n.name.clone(), c.clone())))
        });
        if let Some((ci, d, c)) = hit {
            self.db_open_coll(ci, d, c, cx);
        }
    }

    fn db_tab(&self) -> Option<&CollTab> {
        let v = self.db.as_ref()?;
        v.tabs.get(v.active?)
    }

    fn db_tab_mut(&mut self) -> Option<&mut CollTab> {
        let v = self.db.as_mut()?;
        v.tabs.get_mut(v.active?)
    }

    /// Re-runs the active tab's query.
    fn db_reload(&mut self, cx: &mut Context<Self>) {
        let Some(t) = self.db_tab() else { return };
        let (filter, sort) = (t.filter.text(cx), t.sort.text(cx));
        let (id, ci, db_name, coll, skip, limit) = (t.id, t.conn, t.db.clone(), t.coll.clone(), t.skip, t.limit);
        let Some(client) = self.db.as_ref().and_then(|v| v.conns.get(ci)).and_then(|d| d.client.clone()) else { return };
        if let Some(t) = self.db_tab_mut() {
            t.loading = true;
        }
        cx.notify();
        let job = move || {
            let p = db::find(&client, &db_name, &coll, &filter, &sort, skip, limit)?;
            let g = build_grid(&p.docs);
            anyhow::Ok((p, g))
        };
        self.db_run(job, move |ws, r, _| {
            let Some(t) = ws.db.as_mut().and_then(|v| v.tabs.iter_mut().find(|t| t.id == id)) else { return };
            t.loading = false;
            t.edit = None;
            match r {
                Ok((page, grid)) => {
                    t.grid = Arc::new(grid);
                    t.cursor = t.cursor.filter(|&i| i < page.docs.len()).or((!page.docs.is_empty()).then_some(0));
                    t.selected.retain(|&i| i < page.docs.len());
                    t.page = page;
                    t.error = None;
                }
                Err(e) => t.error = Some(format!("{e:#}")),
            }
        }, cx);
    }

    fn db_apply(&mut self, cx: &mut Context<Self>) {
        if let Some(t) = self.db_tab_mut() {
            t.skip = 0;
        }
        self.db_reload(cx);
    }

    fn db_page(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(t) = self.db_tab_mut() else { return };
        let lim = t.limit as u64;
        if forward {
            if t.skip + lim >= t.page.total {
                return;
            }
            t.skip += lim;
        } else {
            if t.skip == 0 {
                return;
            }
            t.skip = t.skip.saturating_sub(lim);
        }
        t.cursor = Some(0);
        t.selected.clear();
        self.db_reload(cx);
    }

    fn db_row_click(&mut self, row: usize, col: Option<String>, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.db_tab_mut() else { return };
        if ev.modifiers.control || ev.modifiers.platform {
            if !t.selected.remove(&row) {
                t.selected.insert(row);
            }
        } else if ev.modifiers.shift {
            let a = t.cursor.unwrap_or(row);
            t.selected = (a.min(row)..=a.max(row)).collect();
        } else {
            t.selected.clear();
        }
        t.cursor = Some(row);
        if ev.click_count == 2 {
            if let Some(col) = col {
                self.db_start_edit(row, col, window, cx);
            }
        }
        cx.notify();
    }

    fn db_start_edit(&mut self, row: usize, col: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.db_tab() else { return };
        if col == "_id" {
            return;
        }
        let Some(d) = t.page.docs.get(row) else { return };
        let value = match d.get(&col) {
            Some(Bson::String(s)) => serde_json::to_string(s).unwrap_or_default(),
            Some(v) => db::compact(v),
            None => String::new(),
        };
        let field = Field::new("Giá trị", "giá trị JSON: \"chuỗi\", 12, true, null, { … }", &value, cx);
        let focus = field.input.read(cx).focus_handle(cx);
        if let Some(t) = self.db_tab_mut() {
            t.edit = Some((row, col, field));
        }
        dialogs::focus_later(focus, window, cx);
    }

    fn db_commit_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.db_tab_mut() else { return };
        let Some((row, col, field)) = t.edit.take() else { return };
        let value = field.text(cx);
        let Some(id) = t.page.docs.get(row).and_then(|d| d.get("_id")).cloned() else { return };
        let (ci, db_name, coll, tid) = (t.conn, t.db.clone(), t.coll.clone(), t.id);
        // a bare word is meant as a string
        let value = if value.is_empty() { "null".to_string() } else if db::parse_value(&value).is_err() { serde_json::to_string(&value).unwrap_or_default() } else { value };
        let Some(client) = self.db.as_ref().and_then(|v| v.conns.get(ci)).and_then(|d| d.client.clone()) else { return };
        if let Some(v) = self.db.as_mut() {
            v.needs_focus = true;
        }
        let _ = window;
        let (c2, v2) = (col.clone(), value.clone());
        self.db_run(move || db::set_field(&client, &db_name, &coll, &id, &c2, &v2), move |ws, r, cx| match r {
            Ok(()) => {
                if let Some(t) = ws.db.as_mut().and_then(|v| v.tabs.iter_mut().find(|t| t.id == tid)) {
                    if let (Some(d), Ok(b)) = (t.page.docs.get_mut(row), db::parse_value(&value)) {
                        d.insert(col, b);
                        t.grid = Arc::new(build_grid(&t.page.docs));
                    }
                }
                ws.message = Some("Đã lưu".into());
                let _ = cx;
            }
            Err(e) => {
                if let Some(v) = ws.db.as_mut() {
                    v.error = Some(e.to_string());
                }
            }
        }, cx);
    }

    /// Esc: close the dialog, then a cell edit, then the window.
    pub(crate) fn db_escape(&mut self, cx: &mut Context<Self>) {
        let Some(v) = self.db.as_mut() else { return };
        if v.dialog.take().is_some() {
            v.needs_focus = true;
        } else if self.db_tab().is_some_and(|t| t.edit.is_some()) {
            return self.db_cancel_edit(cx);
        } else {
            self.db_open = false;
        }
        cx.notify();
    }

    fn db_cancel_edit(&mut self, cx: &mut Context<Self>) {
        if let Some(t) = self.db_tab_mut() {
            t.edit = None;
        }
        if let Some(v) = self.db.as_mut() {
            v.needs_focus = true;
        }
        cx.notify();
    }

    fn db_ask_delete(&mut self, cx: &mut Context<Self>) {
        let Some(t) = self.db_tab() else { return };
        let rows: Vec<usize> = if t.selected.is_empty() { t.cursor.into_iter().collect() } else { t.selected.iter().copied().collect() };
        let ids: Vec<Bson> = rows.iter().filter_map(|&r| t.page.docs.get(r)?.get("_id").cloned()).collect();
        if ids.is_empty() {
            return;
        }
        let title = format!("Xóa {} document khỏi {}?", ids.len(), t.coll);
        let body = ids.iter().take(5).map(db::cell).collect::<Vec<_>>().join("\n") + if ids.len() > 5 { "\n…" } else { "" };
        if let Some(v) = self.db.as_mut() {
            v.dialog = Some(DbDialog::Confirm { title, body, op: DbOp::DeleteDocs(ids) });
        }
        cx.notify();
    }

    fn db_confirm(&mut self, op: DbOp, cx: &mut Context<Self>) {
        let Some(v) = self.db.as_mut() else { return };
        v.dialog = None;
        match op {
            DbOp::RemoveConn(ci) => {
                if ci < v.conns.len() {
                    v.conns.remove(ci);
                    v.tabs.retain(|t| t.conn != ci);
                    for t in v.tabs.iter_mut().filter(|t| t.conn > ci) {
                        t.conn -= 1;
                    }
                    v.active = (!v.tabs.is_empty()).then_some(0);
                    let list: Vec<Conn> = v.conns.iter().map(|d| d.conn.clone()).collect();
                    if let Err(e) = save_conns(&list) {
                        v.error = Some(e.to_string());
                    }
                }
            }
            DbOp::DropColl(ci, d, coll) => {
                let Some(client) = v.conns.get(ci).and_then(|x| x.client.clone()) else { return };
                v.tabs.retain(|t| !(t.conn == ci && t.db == d && t.coll == coll));
                v.active = v.active.filter(|&a| a < v.tabs.len()).or((!v.tabs.is_empty()).then_some(0));
                let d2 = d.clone();
                self.db_run(move || db::drop_collection(&client, &d2, &coll), move |ws, r, cx| {
                    if let Err(e) = r {
                        ws.db.as_mut().unwrap().error = Some(e.to_string());
                    }
                    ws.db_refresh_db(ci, &d, cx);
                }, cx);
            }
            DbOp::DeleteDocs(ids) => {
                let Some(t) = v.active.and_then(|a| v.tabs.get(a)) else { return };
                let Some(client) = v.conns.get(t.conn).and_then(|x| x.client.clone()) else { return };
                let (d, coll) = (t.db.clone(), t.coll.clone());
                self.db_run(move || db::delete(&client, &d, &coll, &ids), |ws, r, cx| {
                    match r {
                        Ok(n) => ws.message = Some(format!("Đã xóa {n} document")),
                        Err(e) => ws.db.as_mut().unwrap().error = Some(e.to_string()),
                    }
                    if let Some(t) = ws.db_tab_mut() {
                        t.selected.clear();
                    }
                    ws.db_reload(cx);
                }, cx);
            }
        }
        cx.notify();
    }

    fn db_refresh_db(&mut self, ci: usize, name: &str, cx: &mut Context<Self>) {
        let di = self.db.as_ref().and_then(|v| v.conns.get(ci)).and_then(|d| d.dbs.iter().position(|n| n.name == name));
        if let Some(di) = di {
            self.db_load_colls(ci, di, cx);
        }
    }

    /// Screenshot check (CHODILAMIDE_OPEN=db-dialog).
    pub(crate) fn debug_open_db_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.db_conn_dialog(None, "", window, cx)
    }

    fn db_conn_dialog(&mut self, edit: Option<usize>, uri: &str, window: &mut Window, cx: &mut Context<Self>) {
        let cur = edit.and_then(|i| self.db.as_ref()?.conns.get(i)).map(|d| d.conn.clone());
        let uri = cur.as_ref().map(|c| c.uri.clone()).unwrap_or_else(|| uri.to_string());
        let name = cur.map(|c| c.name).unwrap_or_else(|| if uri.is_empty() { String::new() } else { conn_name(&uri) });
        let name = Field::new("Tên kết nối", "VD: computing (dev)", &name, cx);
        let uri_f = Field::new("Connection string", "mongodb://user:pass@host:27017/db?authSource=admin", if uri.is_empty() { "mongodb://localhost:27017" } else { &uri }, cx);
        let focus = uri_f.input.read(cx).focus_handle(cx);
        if let Some(v) = self.db.as_mut() {
            v.dialog = Some(DbDialog::Conn { edit, name, uri: uri_f, test: None, testing: false });
        }
        dialogs::focus_later(focus, window, cx);
        cx.notify();
    }

    fn db_test_conn(&mut self, cx: &mut Context<Self>) {
        let Some(DbDialog::Conn { uri, testing, test, .. }) = self.db.as_mut().and_then(|v| v.dialog.as_mut()) else { return };
        let u = uri.text(cx);
        *testing = true;
        *test = None;
        self.db_run(move || db::connect(&u).and_then(|c| db::databases(&c, &u)), |ws, r, _| {
            if let Some(DbDialog::Conn { testing, test, .. }) = ws.db.as_mut().and_then(|v| v.dialog.as_mut()) {
                *testing = false;
                *test = Some(r.map(|d| format!("Kết nối được · {} database", d.len())).map_err(|e| e.to_string()));
            }
        }, cx);
    }

    fn db_dialog_ok(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(v) = self.db.as_mut() else { return };
        match v.dialog.take() {
            Some(DbDialog::Conn { edit, name, uri, test: _, testing }) => {
                let (n, u) = (name.text(cx), uri.text(cx));
                if u.is_empty() || !u.starts_with("mongodb") {
                    v.dialog = Some(DbDialog::Conn { edit, name, uri, test: Some(Err("Connection string phải bắt đầu bằng mongodb:// hoặc mongodb+srv://".into())), testing });
                    return cx.notify();
                }
                let conn = Conn { name: if n.is_empty() { conn_name(&u) } else { n }, uri: u };
                v.hints.retain(|h| h != &conn.uri);
                let ci = match edit.filter(|&i| i < v.conns.len()) {
                    Some(i) => {
                        let dc = &mut v.conns[i];
                        if dc.conn.uri != conn.uri {
                            dc.client = None;
                            dc.status = Status::Idle;
                            dc.dbs.clear();
                        }
                        dc.conn = conn;
                        i
                    }
                    None => {
                        v.conns.push(DbConn { conn, client: None, status: Status::Idle, open: false, dbs: Vec::new() });
                        v.conns.len() - 1
                    }
                };
                let list: Vec<Conn> = v.conns.iter().map(|d| d.conn.clone()).collect();
                if let Err(e) = save_conns(&list) {
                    v.error = Some(format!("Không lưu được {}: {e}", store_path().display()));
                }
                v.needs_focus = true;
                if v.conns[ci].status != Status::Ok {
                    self.db_connect(ci, cx);
                }
            }
            Some(DbDialog::NewColl { conn, db: d, name }) => {
                let n = name.text(cx);
                let Some(client) = v.conns.get(conn).and_then(|x| x.client.clone()) else { return };
                if n.is_empty() {
                    v.dialog = Some(DbDialog::NewColl { conn, db: d, name });
                    return cx.notify();
                }
                v.needs_focus = true;
                let (d2, n2) = (d.clone(), n.clone());
                self.db_run(move || db::create_collection(&client, &d2, &n2), move |ws, r, cx| match r {
                    Ok(()) => {
                        ws.db_refresh_db(conn, &d, cx);
                        ws.db_open_coll(conn, d, n, cx);
                    }
                    Err(e) => ws.db.as_mut().unwrap().error = Some(e.to_string()),
                }, cx);
            }
            Some(DbDialog::Confirm { op, .. }) => self.db_confirm(op, cx),
            Some(DbDialog::Doc { title, path, view }) => {
                view.update(cx, |v, _| {
                    v.save();
                    v.just_saved = false;
                });
                v.dialog = Some(DbDialog::Doc { title, path: path.clone(), view });
                self.db_file_saved(&path, cx);
            }
            None => {}
        }
        let _ = window;
        cx.notify();
    }

    /// Opens the selected document (or a new one) as a JSON file in an editor tab; saving the
    /// tab writes it to MongoDB (replace by `_id`, or insert).
    fn db_doc_dialog(&mut self, edit: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.db_tab() else { return };
        let doc = t.cursor.and_then(|r| t.page.docs.get(r)).filter(|_| edit);
        if edit && doc.is_none() {
            return;
        }
        let id = doc.and_then(|d| d.get("_id")).cloned();
        let text = match doc {
            Some(d) => db::pretty(d),
            None => "{\n  \n}".to_string(),
        };
        let file = match &id {
            Some(i) => db::cell(i),
            None => format!("moi-{}", t.id * 1000 + t.page.docs.len() as u64),
        };
        let safe = |s: &str| s.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '.' { c } else { '_' }).collect::<String>();
        let dir = edit_dir().join(safe(&format!("{}.{}", t.db, t.coll)));
        let path = dir.join(format!("{}.json", safe(&file)));
        let title = if edit { format!("Sửa document {file}") } else { format!("Document mới trong {}", t.coll) };
        let target = DocEdit { conn: t.conn, db: t.db.clone(), coll: t.coll.clone(), id, tab: t.id };
        if let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, text)) {
            if let Some(v) = self.db.as_mut() {
                v.error = Some(format!("Không ghi được {}: {e}", path.display()));
            }
            return cx.notify();
        }
        if let Some(v) = self.db.as_mut() {
            v.edits.insert(path.clone(), target);
        }
        let buffer = match editor_core::Buffer::open(&path) {
            Ok(b) => b,
            Err(e) => {
                if let Some(v) = self.db.as_mut() {
                    v.error = Some(format!("Không mở được {}: {e}", path.display()));
                }
                return cx.notify();
            }
        };
        let tab_width = self.settings.tab_width;
        let view = cx.new(|cx| crate::editor_view::EditorView::new(buffer, tab_width, cx));
        cx.observe(&view, |_, _, cx| cx.notify()).detach();
        let focus = view.read(cx).focus_handle(cx);
        if let Some(v) = self.db.as_mut() {
            v.dialog = Some(DbDialog::Doc { title, path, view });
        }
        dialogs::focus_later(focus, window, cx);
        cx.notify();
    }

    /// A tab was saved: if it is a document opened from the Database window, write it to MongoDB.
    pub(crate) fn db_file_saved(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        let Some(v) = self.db.as_ref() else { return };
        let Some(e) = v.edits.get(path) else { return };
        let Some(client) = v.conns.get(e.conn).and_then(|d| d.client.clone()) else {
            self.message = Some("Chưa kết nối MongoDB: mở Database (Alt+7) và kết nối lại rồi lưu lại".into());
            return;
        };
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let (d, coll, id, tab) = (e.db.clone(), e.coll.clone(), e.id.clone(), e.tab);
        let p = path.to_path_buf();
        self.db_run(
            move || match &id {
                Some(id) => db::replace(&client, &d, &coll, id, &text).map(|_| (None, format!("Đã lưu document {} lên {coll}", db::cell(id)))),
                None => db::insert(&client, &d, &coll, &text).map(|n| (Some(n.clone()), format!("Đã thêm document {} vào {coll}", db::cell(&n)))),
            },
            move |ws, r, cx| match r {
                Ok((new_id, msg)) => {
                    ws.message = Some(msg);
                    let Some(v) = ws.db.as_mut() else { return };
                    if matches!(&v.dialog, Some(DbDialog::Doc { path, .. }) if *path == p) {
                        v.dialog = None;
                        v.needs_focus = true;
                    }
                    // inserted: saving again updates the same document
                    if let (Some(n), Some(e)) = (new_id, v.edits.get_mut(&p)) {
                        e.id = Some(n);
                    }
                    if v.active.and_then(|a| v.tabs.get(a)).is_some_and(|t| t.id == tab) {
                        ws.db_reload(cx);
                    } else if let Some(t) = v.tabs.iter_mut().find(|t| t.id == tab) {
                        // reloaded when shown again
                        t.loading = false;
                    }
                }
                Err(e) => {
                    let m = format!("Lưu lên MongoDB lỗi: {e:#}");
                    if let Some(v) = ws.db.as_mut() {
                        v.error = Some(m.clone());
                    }
                    ws.message = Some(m);
                }
            },
            cx,
        );
    }

    fn db_close_tab(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(v) = self.db.as_mut() else { return };
        if i < v.tabs.len() {
            v.tabs.remove(i);
        }
        v.active = match v.active {
            Some(a) if a > i => Some(a - 1),
            Some(a) if a < v.tabs.len() => Some(a),
            _ => v.tabs.len().checked_sub(1),
        };
        cx.notify();
    }

    // ---- rendering ----------------------------------------------------------------------

    pub(crate) fn render_db_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.db_open {
            return None;
        }
        let v = self.db.as_mut()?;
        if std::mem::take(&mut v.needs_focus) {
            dialogs::focus_later(v.focus.clone(), window, cx);
        }
        let saved = match &v.dialog {
            Some(DbDialog::Doc { path, view, .. }) if view.read(cx).just_saved => Some((path.clone(), view.clone())),
            _ => None,
        };
        if let Some((p, view)) = saved {
            view.update(cx, |v, _| v.just_saved = false);
            self.db_file_saved(&p, cx);
        }
        let tree = self.render_db_tree(cx);
        let main = self.render_db_main(cx);
        let dialog = self.render_db_dialog(cx);
        let v = self.db.as_ref()?;
        let error = v.error.clone();
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_3()
            .py(px(8.))
            .border_b_1()
            .border_color(c(theme::BORDER))
            .child(icon(Ico::Database, 16., c(theme::GIT_ADDED)))
            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(theme::FG)).child("Database"))
            .child(div().text_size(px(11.5)).text_color(c(theme::FG_DIM)).child("MongoDB"))
            .child(div().flex_1())
            .children(error.map(|e| {
                div()
                    .id("db-error")
                    .max_w(px(560.))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py(px(2.))
                    .rounded_full()
                    .bg(tint(theme::ERROR, 0.14))
                    .text_size(px(12.))
                    .text_color(c(theme::ERROR))
                    .child(div().truncate().child(SharedString::from(e)))
                    .child(icon(Ico::Close, 11., c(theme::ERROR)))
                    .on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| {
                        if let Some(v) = ws.db.as_mut() {
                            v.error = None;
                        }
                        cx.notify()
                    }))
            }))
            .child(icon_btn("db-close", Ico::Close, "Đóng (Esc · Alt+7)").on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.toggle_db(cx))));
        Some(
            div()
                .occlude()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .p(px(16.))
                .bg(gpui::rgba(0x00000066))
                .child(
                    div()
                        .key_context("DbView")
                        .track_focus(&v.focus)
                        .size_full()
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .bg(c(theme::POPUP_BG))
                        .border_1()
                        .border_color(c(theme::BORDER))
                        .rounded(px(14.))
                        .shadow_lg()
                        .text_size(px(13.))
                        .on_action(cx.listener(|ws, _: &DbClose, _, cx| {
                            ws.db_open = false;
                            cx.notify()
                        }))
                        .on_action(cx.listener(|ws, _: &DbRefresh, _, cx| ws.db_reload(cx)))
                        .on_action(cx.listener(|ws, _: &DbApply, _, cx| ws.db_apply(cx)))
                        .on_action(cx.listener(|ws, _: &DbOpenFirst, _, cx| ws.db_open_first_match(cx)))
                        .on_action(cx.listener(|ws, _: &DbCommitEdit, window, cx| ws.db_commit_edit(window, cx)))
                        .on_action(cx.listener(|ws, _: &DbCancelEdit, _, cx| ws.db_cancel_edit(cx)))
                        .on_action(cx.listener(|ws, _: &DbDeleteRows, _, cx| ws.db_ask_delete(cx)))
                        .child(header)
                        .child(div().flex_1().min_h_0().flex().flex_row().child(tree).child(main))
                        .children(dialog),
                )
                .into_any_element(),
        )
    }

    fn render_db_tree(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let v = self.db.as_ref().unwrap();
        let active = v.active.and_then(|a| v.tabs.get(a)).map(|t| (t.conn, t.db.clone(), t.coll.clone()));
        // flatten what is expanded; only the visible rows become elements
        let q = v.search.text(cx).to_lowercase();
        let mut items: Vec<TItem> = Vec::new();
        for (ci, dc) in v.conns.iter().enumerate() {
            let (dot, sub) = match &dc.status {
                Status::Ok => (theme::GIT_ADDED, None),
                Status::Connecting => (theme::WARNING, Some("đang kết nối…".to_string())),
                Status::Err(e) => (theme::ERROR, Some(e.clone())),
                Status::Idle => (theme::FG_DIM, None),
            };
            let open = dc.open && dc.status == Status::Ok;
            items.push(TItem::Conn { ci, name: dc.conn.name.clone().into(), uri: db::redact(&dc.conn.uri).into(), dot, sub: sub.map(Into::into), open, err: matches!(dc.status, Status::Err(_)) });
            if !open {
                continue;
            }
            for (di, n) in dc.dbs.iter().enumerate() {
                // searching: only databases with a matching collection, shown open
                let hits: Option<Vec<&String>> = (!q.is_empty()).then(|| n.colls.iter().flatten().filter(|c| c.to_lowercase().contains(&q)).collect());
                if let Some(h) = &hits {
                    if h.is_empty() {
                        continue;
                    }
                    items.push(TItem::Db { ci, di, name: n.name.clone().into(), size: human_size(n.size).into(), open: true });
                    for coll in h {
                        let on = active.as_ref().is_some_and(|(a, d, k)| *a == ci && *d == n.name && k == *coll);
                        items.push(TItem::Coll { ci, db: n.name.clone().into(), name: (*coll).clone().into(), active: on });
                    }
                    continue;
                }
                items.push(TItem::Db { ci, di, name: n.name.clone().into(), size: human_size(n.size).into(), open: n.open });
                if !n.open {
                    continue;
                }
                match &n.colls {
                    None => items.push(TItem::Note("đang tải…".into())),
                    Some(cl) if cl.is_empty() => items.push(TItem::Note("(chưa có collection)".into())),
                    Some(cl) => {
                        for coll in cl {
                            let on = active.as_ref().is_some_and(|(a, d, k)| *a == ci && *d == n.name && k == coll);
                            items.push(TItem::Coll { ci, db: n.name.clone().into(), name: coll.clone().into(), active: on });
                        }
                    }
                }
            }
        }
        if !q.is_empty() && !items.iter().any(|i| matches!(i, TItem::Coll { .. })) {
            items.push(TItem::Note("Không có collection nào khớp".into()));
        }
        if v.conns.is_empty() && v.hints.is_empty() {
            items.push(TItem::Note("Chưa có kết nối nào. Bấm + Kết nối để thêm MongoDB.".into()));
        }
        if !v.hints.is_empty() {
            items.push(TItem::Header("TÌM THẤY TRONG PROJECT"));
            for u in &v.hints {
                items.push(TItem::Hint { uri: u.clone(), name: conn_name(u).into(), shown: db::redact(u).into() });
            }
        }
        let this = cx.entity().downgrade();
        let list = uniform_list("db-tree", items.len(), move |range, _, _| range.map(|i| tree_row(i, &items[i], &this)).collect()).size_full();
        div()
            .w(px(300.))
            .flex_none()
            .flex()
            .flex_col()
            .text_size(px(13.))
            .border_r_1()
            .border_color(c(theme::BORDER))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py(px(6.))
                    .child(div().flex_1().text_size(px(10.5)).font_weight(gpui::FontWeight::BOLD).text_color(c(theme::FG_DIM)).child("KẾT NỐI"))
                    .child(
                        div()
                            .id("db-new-conn")
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .px(px(10.))
                            .h(px(24.))
                            .rounded_full()
                            .bg(tint(theme::ACCENT, 0.16))
                            .text_size(px(12.))
                            .text_color(c(theme::ACCENT))
                            .hover(|s| s.bg(tint(theme::ACCENT, 0.28)))
                            .child(icon(Ico::Plus, 12., c(theme::ACCENT)))
                            .child("Kết nối")
                            .on_mouse_down(MouseButton::Left, cx.listener(|ws, _, window, cx| ws.db_conn_dialog(None, "", window, cx))),
                    ),
            )
            .child(
                div()
                    .key_context("DbSearch")
                    .mx_2()
                    .mb_2()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px(px(10.))
                    .h(px(30.))
                    .rounded_lg()
                    .border_1()
                    .border_color(c(theme::BORDER))
                    .bg(c(theme::BG))
                    .text_size(px(13.))
                    .child(icon(Ico::Search, 13., c(theme::FG_DIM)))
                    .child(div().flex_1().min_w_0().child(v.search.input.clone())),
            )
            .child(div().flex_1().min_h_0().pb_2().child(list))
            .into_any_element()
    }

    fn render_db_main(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let v = self.db.as_ref().unwrap();
        let Some(ai) = v.active.filter(|&a| a < v.tabs.len()) else {
            return div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .bg(c(theme::BG))
                .child(icon(Ico::Database, 40., c(theme::BORDER)))
                .child(div().text_color(c(theme::FG_DIM)).child("Chọn một collection bên trái để xem dữ liệu"))
                .into_any_element();
        };
        // tabs
        let tabs = div()
            .id("db-tabs")
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_2()
            .pt(px(6.))
            .overflow_x_scroll()
            .children(v.tabs.iter().enumerate().map(|(i, t)| {
                let on = i == ai;
                div()
                    .id(("db-tab", i))
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.))
                    .pl(px(10.))
                    .pr(px(4.))
                    .h(px(28.))
                    .rounded_t(px(8.))
                    .when(on, |d| d.bg(c(theme::BG)).border_1().border_b_0().border_color(c(theme::BORDER)))
                    .when(!on, |d| d.hover(|s| s.bg(c(theme::ROW_HOVER))))
                    .child(icon(Ico::List, 12., c(if on { theme::ACCENT } else { theme::FG_DIM })))
                    .child(div().text_size(px(12.5)).text_color(c(if on { theme::FG } else { theme::FG_DIM })).child(SharedString::from(t.coll.clone())))
                    .child(div().text_size(px(10.5)).text_color(c(theme::FG_DIM)).child(SharedString::from(t.db.clone())))
                    .child(
                        div()
                            .id(("db-tab-x", i))
                            .size(px(18.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .hover(|s| s.bg(c(theme::BORDER)))
                            .child(icon(Ico::Close, 10., c(theme::FG_DIM)))
                            .on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, cx| {
                                cx.stop_propagation();
                                ws.db_close_tab(i, cx)
                            })),
                    )
                    .on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, cx| {
                        if let Some(v) = ws.db.as_mut() {
                            v.active = Some(i);
                        }
                        cx.notify()
                    }))
            }));
        let t = &v.tabs[ai];
        let font = SharedString::from(self.settings.font_family.clone());
        let tool = |id: &'static str, ico: Ico, label: &'static str, color: u32| {
            div()
                .id(id)
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .px(px(9.))
                .h(px(26.))
                .rounded_full()
                .text_size(px(12.))
                .text_color(c(color))
                .hover(|s| s.bg(c(theme::ROW_HOVER)))
                .child(icon(ico, 13., c(color)))
                .child(label)
        };
        let query_box = |label: &'static str, f: &Field, w: Option<f32>| {
            div()
                .key_context("DbQuery")
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .when_some(w, |d, w| d.w(px(w)).flex_none())
                .when(w.is_none(), |d| d.flex_1().min_w_0())
                .px(px(10.))
                .h(px(30.))
                .rounded_full()
                .border_1()
                .border_color(c(theme::BORDER))
                .bg(c(theme::BG))
                .child(div().flex_none().text_size(px(11.)).font_weight(gpui::FontWeight::BOLD).text_color(c(theme::FG_DIM)).child(label))
                .child(div().flex_1().min_w_0().font_family(font.clone()).child(f.input.clone()))
        };
        let has_sel = t.cursor.is_some();
        let toolbar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_3()
            .py(px(8.))
            .bg(c(theme::BG))
            .child(icon(Ico::Search, 14., c(theme::FG_DIM)))
            .child(query_box("FILTER", &t.filter, None))
            .child(query_box("SORT", &t.sort, Some(220.)))
            .child(
                div()
                    .id("db-run")
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .px(px(12.))
                    .h(px(28.))
                    .rounded_full()
                    .bg(c(theme::ACCENT))
                    .text_size(px(12.))
                    .text_color(gpui::white())
                    .hover(|s| s.opacity(0.85))
                    .tooltip(tip("Chạy truy vấn (Enter)"))
                    .child(icon(Ico::Play, 11., gpui::white()))
                    .child("Lọc")
                    .on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.db_apply(cx))),
            );
        let view_toggle = |id: &'static str, label: &'static str, on: bool, json: bool| {
            div()
                .id(id)
                .px(px(10.))
                .h(px(22.))
                .flex()
                .items_center()
                .rounded_full()
                .text_size(px(12.))
                .when(on, |d| d.bg(c(theme::POPUP_BG)).shadow_sm())
                .text_color(c(if on { theme::FG } else { theme::FG_DIM }))
                .child(label)
                .on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, cx| {
                    if let Some(t) = ws.db_tab_mut() {
                        t.json = json;
                    }
                    cx.notify()
                }))
        };
        let actions_bar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_3()
            .pb(px(6.))
            .bg(c(theme::BG))
            .border_b_1()
            .border_color(c(theme::BORDER))
            .child(div().flex().flex_row().p(px(2.)).rounded_full().bg(c(theme::ROW_HOVER)).child(view_toggle("db-grid", "Lưới", !t.json, false)).child(view_toggle("db-json", "JSON", t.json, true)))
            .child(div().w(px(8.)))
            .child(tool("db-add", Ico::Plus, "Thêm", theme::FG).tooltip(tip("Thêm document")).on_mouse_down(MouseButton::Left, cx.listener(|ws, _, window, cx| ws.db_doc_dialog(false, window, cx))))
            .when(has_sel, |d| {
                d.child(tool("db-edit", Ico::Gear, "Sửa", theme::FG).tooltip(tip("Sửa cả document dạng JSON (double-click một ô để sửa riêng ô đó)")).on_mouse_down(MouseButton::Left, cx.listener(|ws, _, window, cx| ws.db_doc_dialog(true, window, cx))))
                    .child(tool("db-del", Ico::Minus, "Xóa", theme::ERROR).tooltip(tip("Xóa các dòng đang chọn (Delete)")).on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.db_ask_delete(cx))))
            })
            .child(tool("db-refresh", Ico::Refresh, "Làm mới", theme::FG).tooltip(tip("Làm mới (F5)")).on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.db_reload(cx))))
            .child(div().flex_1())
            .when(t.loading, |d| d.child(div().text_size(px(11.5)).text_color(c(theme::FG_DIM)).child("đang tải…")))
            .when(!t.selected.is_empty(), |d| d.child(div().text_size(px(11.5)).text_color(c(theme::ACCENT)).child(SharedString::from(format!("{} dòng đang chọn", t.selected.len())))));
        let body: AnyElement = match &t.error {
            Some(e) => div()
                .flex_1()
                .p_4()
                .bg(c(theme::BG))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_2()
                        .p_3()
                        .rounded(px(10.))
                        .bg(tint(theme::ERROR, 0.12))
                        .child(icon(Ico::Problems, 16., c(theme::ERROR)))
                        .child(div().flex_1().text_color(c(theme::FG)).child(SharedString::from(e.clone()))),
                )
                .into_any_element(),
            None if t.json => self.render_db_json(ai, font.clone()),
            None => self.render_db_grid(ai, font.clone(), cx),
        };
        let v = self.db.as_ref().unwrap();
        let t = &v.tabs[ai];
        let n = t.page.docs.len() as u64;
        let range = if n == 0 { "0".to_string() } else { format!("{}–{}", t.skip + 1, t.skip + n) };
        let limit = t.limit;
        let can_prev = t.skip > 0;
        let can_next = t.skip + n < t.page.total;
        let pager_btn = |id: &'static str, ico: Ico, on: bool| {
            div()
                .id(id)
                .size(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_md()
                .when(on, |d| d.hover(|s| s.bg(c(theme::ROW_HOVER))))
                .child(icon(ico, 12., c(if on { theme::FG } else { theme::BORDER })))
        };
        let footer = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_3()
            .h(px(32.))
            .border_t_1()
            .border_color(c(theme::BORDER))
            .text_size(px(12.))
            .text_color(c(theme::FG_DIM))
            .child(SharedString::from(format!("{range} / {} document", t.page.total)))
            .child(div().whitespace_nowrap().child(SharedString::from(format!("· {} cột", t.grid.cols.len()))))
            .child(SharedString::from(format!("· {} ms", t.page.millis)))
            .child(div().flex_1())
            .child("Mỗi trang")
            .children(PAGE_SIZES.iter().map(|&s| {
                let on = s == limit;
                div()
                    .id(SharedString::from(format!("db-lim-{s}")))
                    .px(px(7.))
                    .rounded_full()
                    .when(on, |d| d.bg(tint(theme::ACCENT, 0.16)).text_color(c(theme::ACCENT)))
                    .hover(|s| s.bg(c(theme::ROW_HOVER)))
                    .child(s.to_string())
                    .on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, cx| {
                        if let Some(t) = ws.db_tab_mut() {
                            t.limit = s;
                            t.skip = 0;
                        }
                        ws.db_reload(cx)
                    }))
            }))
            .child(pager_btn("db-prev", Ico::ChevronUp, can_prev).on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.db_page(false, cx))))
            .child(pager_btn("db-next", Ico::ChevronDown, can_next).on_mouse_down(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.db_page(true, cx))));
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(tabs)
            .child(toolbar)
            .child(actions_bar)
            .child(div().flex_1().min_h_0().flex().flex_row().child(div().flex_1().min_w_0().flex().flex_col().child(body)))
            .child(footer)
            .into_any_element()
    }

    fn render_db_grid(&mut self, ai: usize, font: SharedString, cx: &mut Context<Self>) -> AnyElement {
        let v = self.db.as_ref().unwrap();
        let t = &v.tabs[ai];
        let g = t.grid.clone();
        let total_w = g.total_w;
        let header = div()
            .flex()
            .flex_row()
            .w(px(total_w))
            .h(px(38.))
            .border_b_1()
            .border_color(c(theme::BORDER))
            .bg(c(theme::PANEL_BG))
            .child(div().w(px(NUM_W)).flex_none())
            .children((0..g.cols.len()).map(|k| {
                div()
                    .w(px(g.widths[k]))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .px_2()
                    .border_l_1()
                    .border_color(c(theme::BORDER))
                    .child(div().truncate().text_size(px(12.)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(theme::FG)).child(g.titles[k].clone()))
                    .child(div().text_size(px(9.5)).text_color(c(theme::FG_DIM)).child(g.kinds[k]))
            }));
        let skip = t.skip;
        let selected = t.selected.clone();
        let cursor = t.cursor;
        let edit = t.edit.as_ref().map(|(r, k, f)| (*r, k.clone(), f.input.clone()));
        let this = cx.entity().downgrade();
        let rows = g.cells.len();
        let empty = rows == 0 && !t.loading;
        let g2 = g.clone();
        let line = tint(theme::BORDER, 0.5);
        let (stripe, sel_bg, cur_bg) = (tint(theme::FG, 0.025), tint(theme::ACCENT, 0.14), tint(theme::ACCENT, 0.22));
        let list = uniform_list("db-rows", rows, move |range, _, _| {
            let g = &g2;
            range
                .map(|r| {
                    let this1 = this.clone();
                    div()
                        .id(("db-row", r))
                        .flex()
                        .flex_row()
                        .w(px(total_w))
                        .h(px(ROW_H))
                        .border_b_1()
                        .border_color(line)
                        .when(r % 2 == 1, |d| d.bg(stripe))
                        .when(selected.contains(&r), |d| d.bg(sel_bg))
                        .when(cursor == Some(r), |d| d.bg(cur_bg))
                        .child(
                            div()
                                .w(px(NUM_W))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_end()
                                .pr_2()
                                .text_size(px(11.))
                                .text_color(c(theme::GUTTER_FG))
                                .child(SharedString::from((skip + r as u64 + 1).to_string()))
                                .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
                                    let _ = this1.update(cx, |ws, cx| ws.db_row_click(r, None, ev, window, cx));
                                }),
                        )
                        .children(g.cells[r].iter().enumerate().map(|(k, (text, color, missing))| {
                            let cell = div().w(px(g.widths[k])).flex_none().flex().items_center().px_2().border_l_1().border_color(line).overflow_hidden();
                            if let Some((_, _, input)) = edit.as_ref().filter(|(er, ek, _)| *er == r && *ek == g.cols[k]) {
                                return cell.key_context("DbCell").border_1().border_color(c(theme::ACCENT)).bg(c(theme::BG)).font_family(font.clone()).child(div().w_full().child(input.clone()));
                            }
                            let this = this.clone();
                            let key = g.cols[k].clone();
                            cell.child(
                                div()
                                    .truncate()
                                    .text_color(c(*color))
                                    .when(*missing, |d| d.italic())
                                    .when(k == 0 && key == "_id", |d| d.font_family(font.clone()).text_size(px(12.)))
                                    .child(text.clone()),
                            )
                            .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
                                let _ = this.update(cx, |ws, cx| ws.db_row_click(r, Some(key.clone()), ev, window, cx));
                            })
                        }))
                })
                .collect()
        })
        .track_scroll(t.scroll.clone())
        .h_full()
        .w(px(total_w));
        let mut grid = div();
        // the wheel scrolls the rows; only a horizontal swipe / Shift+wheel moves the columns
        grid.style().restrict_scroll_to_axis = Some(true);
        grid
            .key_context("DbGrid")
            .id("db-grid-x")
            .flex_1()
            .min_h_0()
            .bg(c(theme::BG))
            .overflow_x_scroll()
            .child(
                div()
                    .h_full()
                    .w(px(total_w))
                    .flex()
                    .flex_col()
                    .child(header)
                    .child(div().flex_1().min_h_0().child(list))
                    .when(empty, |d| d.child(div().absolute().top(px(60.)).left(px(24.)).text_color(c(theme::FG_DIM)).child("Không có document nào khớp"))),
            )
            .into_any_element()
    }

    /// Every document as JSON, one virtual list of lines.
    fn render_db_json(&mut self, ai: usize, font: SharedString) -> AnyElement {
        let t = &self.db.as_ref().unwrap().tabs[ai];
        let g = t.grid.clone();
        let cursor = t.cursor;
        div()
            .flex_1()
            .min_h_0()
            .bg(c(theme::BG))
            .px_3()
            .font_family(font)
            .text_size(px(12.5))
            .child(
                uniform_list("db-json-list", g.json.len(), move |range, _, _| {
                    range
                        .map(|i| {
                            let l = &g.json[i];
                            let first = g.doc_lines[l.doc].start == i;
                            json_line(l)
                                .border_l_2()
                                .border_color(if cursor == Some(l.doc) { c(theme::ACCENT) } else { tint(theme::BORDER, 0.6) })
                                .when(first && l.doc > 0, |d| d.mt(px(0.)))
                                .into_any_element()
                        })
                        .collect()
                })
                .size_full(),
            )
            .into_any_element()
    }

    fn render_db_dialog(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let v = self.db.as_ref()?;
        let dialog = v.dialog.as_ref()?;
        let cancel = |ws: &mut Workspace, _: &mut Window, cx: &mut Context<Workspace>| {
            if let Some(v) = ws.db.as_mut() {
                v.dialog = None;
                v.needs_focus = true;
            }
            cx.notify()
        };
        let (title, width, body, ok_label): (String, f32, AnyElement, &'static str) = match dialog {
            DbDialog::Conn { edit, name, uri, test, testing } => {
                let result = match (testing, test) {
                    (true, _) => Some(div().text_color(c(theme::FG_DIM)).child("Đang thử kết nối…")),
                    (_, Some(Ok(m))) => Some(div().text_color(c(theme::GIT_ADDED)).child(SharedString::from(format!("✓ {m}")))),
                    (_, Some(Err(e))) => Some(div().text_color(c(theme::ERROR)).child(SharedString::from(e.clone()))),
                    _ => None,
                };
                (
                    if edit.is_some() { "Sửa kết nối MongoDB".into() } else { "Kết nối MongoDB mới".into() },
                    640.,
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(name.row())
                        .child(uri.row())
                        .child(div().text_size(px(11.5)).text_color(c(theme::FG_DIM)).child("Hỗ trợ mongodb:// và mongodb+srv://, replica set, authSource, TLS… như trong application.properties. Lưu ở ~/.config/chodilamide/db.json (chỉ bạn đọc được)."))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_3()
                                .child(dialogs::button("db-test", "Thử kết nối", false, cx, |ws: &mut Workspace, _, cx| ws.db_test_conn(cx)))
                                .children(result),
                        )
                        .into_any_element(),
                    "Lưu & kết nối",
                )
            }
            DbDialog::NewColl { db: d, name, .. } => (format!("Collection mới trong {d}"), 460., name.row().into_any_element(), "Tạo"),
            DbDialog::Doc { title, view, .. } => (
                title.clone(),
                820.,
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().h(px(480.)).border_1().border_color(c(theme::BORDER)).rounded_md().overflow_hidden().child(view.clone()))
                    .child(div().text_size(px(11.5)).text_color(c(theme::FG_DIM)).child("Extended JSON ({\"$oid\": …}, {\"$date\": …}) hoặc cú pháp shell (ObjectId(…), ISODate(…)) · Ctrl+S để lưu"))
                    .into_any_element(),
                "Lưu",
            ),
            DbDialog::Confirm { title, body, .. } => (
                title.clone(),
                480.,
                div().flex().flex_col().gap_1().children(body.lines().map(|l| div().text_color(c(theme::FG)).child(SharedString::from(l.to_string())))).into_any_element(),
                "Xóa",
            ),
        };
        let footer = div()
            .flex()
            .flex_row()
            .gap_2()
            .child(dialogs::button("db-dlg-cancel", "Hủy", false, cx, cancel))
            .child(dialogs::button("db-dlg-ok", ok_label, true, cx, |ws: &mut Workspace, window, cx| ws.db_dialog_ok(window, cx)));
        Some(
            dialogs::modal("db-dialog", &title, width, body, footer)
                .on_action(cx.listener(|ws, _: &DialogConfirm, window, cx| ws.db_dialog_ok(window, cx)))
                .on_action(cx.listener(move |ws, _: &DialogCancel, window, cx| cancel(ws, window, cx)))
                .on_action(cx.listener(|ws, _: &DialogNext, window, cx| {
                    if let Some(DbDialog::Conn { name, uri, .. }) = ws.db.as_ref().and_then(|v| v.dialog.as_ref()) {
                        dialogs::focus_step(&[name, uri], true, window, cx);
                    }
                }))
                .into_any_element(),
        )
    }
}

fn json_line(l: &JLine) -> gpui::Div {
    div()
        .flex()
        .flex_row()
        .h(px(19.))
        .whitespace_nowrap()
        .pl(px(l.indent))
        .when_some(l.key.clone(), |d, k| d.child(div().text_color(c(theme::GIT_MODIFIED)).child(k)))
        .child(div().text_color(c(l.color)).child(l.rest.clone()))
}

/// Pretty JSON split into lines with keys, strings, numbers and literals colored.
fn json_lines(text: &str, doc: usize) -> Vec<JLine> {
    text.lines()
        .map(|l| {
            let indent = l.len() - l.trim_start().len();
            let t = l.trim_start();
            let (key, rest) = match t.strip_prefix('"').and_then(|s| s.split_once("\": ")) {
                Some((k, r)) => (Some(SharedString::from(format!("{k}: "))), r.to_string()),
                None => (None, t.to_string()),
            };
            let trimmed = rest.trim_end_matches(',');
            let color = if trimmed.starts_with('"') {
                theme::GIT_ADDED
            } else if trimmed.starts_with(['{', '[', '}', ']']) {
                theme::FG_DIM
            } else if trimmed == "true" || trimmed == "false" || trimmed == "null" {
                theme::ACCENT
            } else {
                theme::WARNING
            };
            // one line is drawn whole: cut very long strings
            let mut rest = rest;
            if rest.len() > 400 {
                let mut cut = 400;
                while !rest.is_char_boundary(cut) {
                    cut -= 1;
                }
                rest.truncate(cut);
                rest.push('…');
            }
            JLine { indent: indent as f32 * 4., key, rest: SharedString::from(rest), color, doc }
        })
        .collect()
}

/// A row of the connection tree (flattened, so the list draws only what is visible).
enum TItem {
    Conn { ci: usize, name: SharedString, uri: SharedString, dot: u32, sub: Option<SharedString>, open: bool, err: bool },
    Db { ci: usize, di: usize, name: SharedString, size: SharedString, open: bool },
    Coll { ci: usize, db: SharedString, name: SharedString, active: bool },
    Note(SharedString),
    Header(&'static str),
    Hint { uri: String, name: SharedString, shown: SharedString },
}

/// Fixed height: uniform_list measures one row for all.
const TREE_H: f32 = 30.;

/// "name @ host" → ("name", "host"); a bare name has no host.
fn split_name(n: &SharedString) -> (SharedString, SharedString) {
    match n.split_once(" @ ") {
        Some((a, b)) => (a.to_string().into(), b.to_string().into()),
        None => (n.clone(), SharedString::default()),
    }
}

fn tree_row(i: usize, item: &TItem, this: &gpui::WeakEntity<Workspace>) -> AnyElement {
    let row = |depth: usize| {
        div()
            .id(("db-t", i))
            .group("db-row")
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .h(px(TREE_H))
            .overflow_hidden()
            .pl(px(8. + depth as f32 * 16.))
            .pr_1()
            .mx_1()
            .rounded_md()
            .cursor_pointer()
            .hover(|s| s.bg(c(theme::ROW_HOVER)))
    };
    // a hover button calling `f` on the workspace
    let hb = |k: usize, ico: Ico, label: &'static str, f: Box<dyn Fn(&mut Workspace, &mut Window, &mut Context<Workspace>)>| {
        let this = this.clone();
        div()
            .id(SharedString::from(format!("db-t-{i}-{k}")))
            .flex_none()
            .size(px(20.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .invisible()
            .group_hover("db-row", |s| s.visible())
            .hover(|s| s.bg(c(theme::BORDER)))
            .tooltip(tip(label))
            .child(icon(ico, 12., c(theme::FG_DIM)))
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                cx.stop_propagation();
                let _ = this.update(cx, |ws, cx| f(ws, window, cx));
            })
    };
    let click = |f: Box<dyn Fn(&mut Workspace, &mut Window, &mut Context<Workspace>)>| {
        let this = this.clone();
        move |_: &MouseDownEvent, window: &mut Window, cx: &mut gpui::App| {
            let _ = this.update(cx, |ws, cx| f(ws, window, cx));
        }
    };
    let dim = |t: SharedString| div().h(px(TREE_H)).flex().items_center().pl(px(56.)).text_size(px(12.)).text_color(c(theme::FG_DIM)).child(t);
    match item {
        TItem::Note(t) => dim(t.clone()).into_any_element(),
        TItem::Header(t) => div().h(px(TREE_H)).flex().items_end().px_3().pb_1().text_size(px(10.5)).font_weight(gpui::FontWeight::BOLD).text_color(c(theme::FG_DIM)).child(*t).into_any_element(),
        TItem::Conn { ci, name, uri, dot, sub, open, err } => {
            let ci = *ci;
            row(0)
                .tooltip(tip(uri.clone()))
                .child(icon(if *open { Ico::ChevronDown } else { Ico::ChevronRight }, 11., c(theme::FG_DIM)))
                .child(
                    div()
                        .flex_none()
                        .relative()
                        .child(icon(Ico::Database, 15., c(theme::FG)))
                        .child(div().absolute().right(px(-2.)).bottom(px(-1.)).size(px(7.)).rounded_full().border_1().border_color(c(theme::POPUP_BG)).bg(c(*dot))),
                )
                .child({
                    let (title, host) = split_name(name);
                    let (second, col) = match sub {
                        Some(s) => (s.clone(), if *err { theme::ERROR } else { theme::WARNING }),
                        None => (host, theme::FG_DIM),
                    };
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_row()
                        .items_baseline()
                        .gap(px(6.))
                        .overflow_hidden()
                        .child(div().flex_shrink_0().max_w(px(150.)).truncate().font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(theme::FG)).child(title))
                        .child(div().flex_1().min_w_0().truncate().text_size(px(11.)).text_color(c(col)).child(second))
                })
                .child(hb(0, Ico::Refresh, "Kết nối lại / làm mới", Box::new(move |ws, _, cx| {
                    if let Some(d) = ws.db.as_mut().and_then(|v| v.conns.get_mut(ci)) {
                        d.status = Status::Idle;
                    }
                    ws.db_connect(ci, cx)
                })))
                .child(hb(1, Ico::Gear, "Sửa kết nối (hoặc chuột phải)", Box::new(move |ws, window, cx| ws.db_conn_dialog(Some(ci), "", window, cx))))
                .on_mouse_down(MouseButton::Right, click(Box::new(move |ws, window, cx| ws.db_conn_dialog(Some(ci), "", window, cx))))
                .child(hb(2, Ico::Close, "Xóa kết nối", Box::new(move |ws, _, cx| {
                    if let Some(v) = ws.db.as_mut() {
                        let name = v.conns.get(ci).map(|d| d.conn.name.clone()).unwrap_or_default();
                        v.dialog = Some(DbDialog::Confirm { title: format!("Xóa kết nối \"{name}\"?"), body: "Chỉ xóa kết nối đã lưu trong IDE, không đụng tới dữ liệu trên server.".into(), op: DbOp::RemoveConn(ci) });
                    }
                    cx.notify()
                })))
                .on_mouse_down(MouseButton::Left, click(Box::new(move |ws, _, cx| ws.db_conn_toggle(ci, cx))))
                .into_any_element()
        }
        TItem::Db { ci, di, name, size, open } => {
            let (ci, di) = (*ci, *di);
            let n = name.to_string();
            row(1)
                .child(icon(if *open { Ico::ChevronDown } else { Ico::ChevronRight }, 11., c(theme::FG_DIM)))
                .child(icon(Ico::Stash, 14., c(theme::WARNING)))
                .child(div().flex_1().min_w_0().truncate().text_color(c(theme::FG)).child(name.clone()))
                .child(div().flex_none().text_size(px(10.5)).text_color(c(theme::FG_DIM)).child(size.clone()))
                .child(hb(0, Ico::Plus, "Tạo collection", Box::new(move |ws, window, cx| {
                    let f = Field::new("Tên collection", "orders", "", cx);
                    let focus = f.input.read(cx).focus_handle(cx);
                    if let Some(v) = ws.db.as_mut() {
                        v.dialog = Some(DbDialog::NewColl { conn: ci, db: n.clone(), name: f });
                    }
                    dialogs::focus_later(focus, window, cx);
                    cx.notify()
                })))
                .child(hb(1, Ico::Refresh, "Làm mới", Box::new(move |ws, _, cx| ws.db_load_colls(ci, di, cx))))
                .on_mouse_down(MouseButton::Left, click(Box::new(move |ws, _, cx| ws.db_toggle_db(ci, di, cx))))
                .into_any_element()
        }
        TItem::Coll { ci, db: d, name, active } => {
            let ci = *ci;
            let (d1, c1, d2, c2) = (d.to_string(), name.to_string(), d.to_string(), name.to_string());
            row(2)
                .when(*active, |r| r.bg(tint(theme::ACCENT, 0.16)))
                .child(icon(Ico::List, 13., c(if *active { theme::ACCENT } else { theme::FG_DIM })))
                .child(div().flex_1().min_w_0().truncate().text_color(c(theme::FG)).child(name.clone()))
                .child(hb(0, Ico::Close, "Xóa (drop) collection", Box::new(move |ws, _, cx| {
                    if let Some(v) = ws.db.as_mut() {
                        v.dialog = Some(DbDialog::Confirm {
                            title: format!("Drop collection \"{c2}\"?"),
                            body: format!("Toàn bộ document và index của {d2}.{c2} sẽ bị xóa vĩnh viễn trên server."),
                            op: DbOp::DropColl(ci, d2.clone(), c2.clone()),
                        });
                    }
                    cx.notify()
                })))
                .on_mouse_down(MouseButton::Left, click(Box::new(move |ws, _, cx| ws.db_open_coll(ci, d1.clone(), c1.clone(), cx))))
                .into_any_element()
        }
        TItem::Hint { uri, name, shown } => {
            let u = uri.clone();
            row(0)
                .tooltip(tip(shown.clone()))
                .child(div().flex_none().size(px(18.)).flex().items_center().justify_center().rounded_md().bg(tint(theme::ACCENT, 0.14)).child(icon(Ico::Plus, 11., c(theme::ACCENT))))
                .child({
                    let (title, host) = split_name(name);
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_row()
                        .items_baseline()
                        .gap(px(6.))
                        .overflow_hidden()
                        .child(div().flex_shrink_0().max_w(px(150.)).truncate().text_color(c(theme::FG)).child(title))
                        .child(div().flex_1().min_w_0().truncate().text_size(px(11.)).text_color(c(theme::FG_DIM)).child(host))
                })
                .child(div().flex_none().invisible().group_hover("db-row", |s| s.visible()).px(px(8.)).h(px(20.)).flex().items_center().rounded_full().bg(tint(theme::ACCENT, 0.16)).text_size(px(11.)).text_color(c(theme::ACCENT)).child("Thêm"))
                .on_mouse_down(MouseButton::Left, click(Box::new(move |ws, window, cx| ws.db_conn_dialog(None, &u, window, cx))))
                .into_any_element()
        }
    }
}
