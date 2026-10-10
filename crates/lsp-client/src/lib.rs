//! Minimal, executor-agnostic LSP client over stdio (JSON-RPC 2.0). Generic LSP - the
//! jdtls specifics live in `jdtls`, so another server (nbcode) can be swapped in.
//!
//! Threads: one writer, one reader, one stderr drain. Responses resolve `futures`
//! oneshots, so callers can await them on gpui's executor (or block in the CLI).
pub mod jdtls;
pub mod text;

use anyhow::{Context, Result, anyhow};
use futures::channel::{mpsc, oneshot};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

/// Something the server sent that isn't a response to us.
#[derive(Debug, Clone)]
pub enum ServerMessage {
    Notification { method: String, params: Value },
    /// Server → client request we did not answer automatically (already replied with null).
    Request { method: String, params: Value },
    Exited,
}

type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, RpcError>>>>>;

#[derive(Debug, Clone)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "LSP error {}: {}", self.code, self.message)
    }
}

impl std::error::Error for RpcError {}

pub struct Client {
    next_id: AtomicI64,
    tx: mpsc::UnboundedSender<Vec<u8>>,
    pending: Pending,
    child: Mutex<Option<Child>>,
    /// Answer for `workspace/configuration` requests.
    settings: Arc<Mutex<Value>>,
    pub capabilities: Mutex<Value>,
}

/// An in-flight request that can be cancelled (sends `$/cancelRequest`).
pub struct Request {
    pub id: i64,
    rx: oneshot::Receiver<Result<Value, RpcError>>,
}

impl Request {
    pub async fn response(self) -> Result<Value> {
        match self.rx.await {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(e.into()),
            Err(_) => Err(anyhow!("LSP server went away")),
        }
    }
}

fn frame(msg: &Value) -> Vec<u8> {
    let body = serde_json::to_vec(msg).expect("json");
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend(body);
    out
}

impl Client {
    /// Spawns the server. Returns the client and the stream of server-initiated messages.
    pub fn spawn(program: &Path, args: &[String], cwd: &Path, env: &[(String, String)], settings: Value) -> Result<(Arc<Client>, mpsc::UnboundedReceiver<ServerMessage>)> {
        let mut cmd = Command::new(program);
        cmd.args(args).current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().with_context(|| format!("không chạy được {}", program.display()))?;
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();

        let (tx, rx) = mpsc::unbounded::<Vec<u8>>();
        let (events_tx, events_rx) = mpsc::unbounded::<ServerMessage>();
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let settings = Arc::new(Mutex::new(settings));

        std::thread::Builder::new().name("lsp-writer".into()).spawn(move || writer(stdin, rx))?;
        {
            let pending = pending.clone();
            let settings = settings.clone();
            let reply_tx = tx.clone();
            std::thread::Builder::new()
                .name("lsp-reader".into())
                .spawn(move || reader(stdout, pending, events_tx, reply_tx, settings))?;
        }
        std::thread::Builder::new().name("lsp-stderr".into()).spawn(move || {
            // jdtls logs a lot on stderr; drain it so the pipe never blocks the server
            let mut buf = [0u8; 8192];
            let mut s = stderr;
            while matches!(s.read(&mut buf), Ok(n) if n > 0) {}
        })?;

        let client = Arc::new(Client {
            next_id: AtomicI64::new(1),
            tx,
            pending,
            child: Mutex::new(Some(child)),
            settings,
            capabilities: Mutex::new(Value::Null),
        });
        Ok((client, events_rx))
    }

    pub fn request(&self, method: &str, params: Value) -> Request {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (otx, orx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, otx);
        let _ = self.tx.unbounded_send(frame(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })));
        Request { id, rx: orx }
    }

    pub fn notify(&self, method: &str, params: Value) {
        let _ = self.tx.unbounded_send(frame(&json!({ "jsonrpc": "2.0", "method": method, "params": params })));
    }

    /// Tells the server to drop request `id` (e.g. stale completion while typing).
    pub fn cancel(&self, id: i64) {
        if self.pending.lock().unwrap().remove(&id).is_some() {
            self.notify("$/cancelRequest", json!({ "id": id }));
        }
    }

    pub fn set_settings(&self, settings: Value) {
        *self.settings.lock().unwrap() = settings.clone();
        self.notify("workspace/didChangeConfiguration", json!({ "settings": settings }));
    }

    /// `shutdown` + `exit`, then kill if it hasn't gone after `grace`.
    pub fn shutdown_blocking(&self, grace: std::time::Duration) {
        let req = self.request("shutdown", Value::Null);
        let _ = futures::executor::block_on(async {
            let timeout = async {
                std::thread::sleep(grace);
            };
            futures::pin_mut!(timeout);
            let r = req.response();
            futures::pin_mut!(r);
            futures::future::select(r, timeout).await;
        });
        self.notify("exit", Value::Null);
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let deadline = std::time::Instant::now() + grace;
            while std::time::Instant::now() < deadline {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.lock().unwrap().as_ref().map(|c| c.id())
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.lock().unwrap().take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

fn writer(mut stdin: ChildStdin, rx: mpsc::UnboundedReceiver<Vec<u8>>) {
    let mut rx = futures::executor::block_on_stream(rx);
    while let Some(bytes) = rx.next() {
        if stdin.write_all(&bytes).and_then(|_| stdin.flush()).is_err() {
            break;
        }
    }
}

fn read_message(r: &mut impl BufRead) -> Option<Value> {
    let mut len = None;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(v) = line.strip_prefix("Content-Length:") {
            len = v.trim().parse::<usize>().ok();
        }
    }
    let mut body = vec![0u8; len?];
    r.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

fn reader(
    stdout: std::process::ChildStdout,
    pending: Pending,
    events: mpsc::UnboundedSender<ServerMessage>,
    reply: mpsc::UnboundedSender<Vec<u8>>,
    settings: Arc<Mutex<Value>>,
) {
    let mut r = BufReader::new(stdout);
    while let Some(msg) = read_message(&mut r) {
        let method = msg.get("method").and_then(Value::as_str).map(String::from);
        let id = msg.get("id").cloned();
        match (method, id) {
            // response to one of our requests
            (None, Some(id)) => {
                let Some(id) = id.as_i64() else { continue };
                if let Some(tx) = pending.lock().unwrap().remove(&id) {
                    let res = match msg.get("error") {
                        Some(e) => Err(RpcError {
                            code: e.get("code").and_then(Value::as_i64).unwrap_or(0),
                            message: e.get("message").and_then(Value::as_str).unwrap_or("").to_string(),
                        }),
                        None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
                    };
                    let _ = tx.send(res);
                }
            }
            // server → client request: answer the ones every client must, forward the rest
            (Some(method), Some(id)) => {
                let params = msg.get("params").cloned().unwrap_or(Value::Null);
                let result = match method.as_str() {
                    "workspace/configuration" => {
                        let s = settings.lock().unwrap().clone();
                        let items = params.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
                        Value::Array(items.iter().map(|it| section(&s, it.get("section").and_then(Value::as_str))).collect())
                    }
                    "workspace/workspaceFolders" => Value::Null,
                    _ => Value::Null,
                };
                let _ = reply.unbounded_send(frame(&json!({ "jsonrpc": "2.0", "id": id, "result": result })));
                if !matches!(method.as_str(), "workspace/configuration" | "client/registerCapability" | "window/workDoneProgress/create") {
                    let _ = events.unbounded_send(ServerMessage::Request { method, params });
                }
            }
            (Some(method), None) => {
                let params = msg.get("params").cloned().unwrap_or(Value::Null);
                let _ = events.unbounded_send(ServerMessage::Notification { method, params });
            }
            (None, None) => {}
        }
    }
    // server exited: fail everything still waiting
    pending.lock().unwrap().clear();
    let _ = events.unbounded_send(ServerMessage::Exited);
}

fn section(settings: &Value, name: Option<&str>) -> Value {
    let Some(name) = name else { return settings.clone() };
    let mut cur = settings;
    for part in name.split('.') {
        match cur.get(part) {
            Some(v) => cur = v,
            None => return Value::Null,
        }
    }
    cur.clone()
}

pub fn path_to_uri(p: &Path) -> String {
    url::Url::from_file_path(p).map(|u| u.to_string()).unwrap_or_else(|_| format!("file://{}", p.display()))
}

pub fn uri_to_path(uri: &str) -> Option<std::path::PathBuf> {
    url::Url::parse(uri).ok()?.to_file_path().ok()
}
