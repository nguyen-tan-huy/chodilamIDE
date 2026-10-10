//! Debug Adapter Protocol client over stdio - executor-agnostic like lsp-client
//! (threads + futures channels). One `DapClient` per debug session.
use anyhow::{Context, Result, anyhow};
use futures::channel::{mpsc, oneshot};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub enum DapEvent {
    Event { event: String, body: Value },
    /// The adapter process is gone.
    Closed,
}

type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, String>>>>>;

pub struct DapClient {
    seq: AtomicI64,
    tx: mpsc::UnboundedSender<Vec<u8>>,
    pending: Pending,
    child: Mutex<Option<Child>>,
}

pub struct Pend(oneshot::Receiver<Result<Value, String>>);

impl Pend {
    pub async fn response(self) -> Result<Value> {
        match self.0.await {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(m)) => Err(anyhow!(m)),
            Err(_) => Err(anyhow!("debugger đã thoát")),
        }
    }
}

fn frame(v: &Value) -> Vec<u8> {
    let body = serde_json::to_vec(v).unwrap();
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend(body);
    out
}

impl DapClient {
    /// Starts an adapter (e.g. `java -jar chodilamide-debugger.jar`).
    pub fn spawn(program: &Path, args: &[String]) -> Result<(Arc<DapClient>, mpsc::UnboundedReceiver<DapEvent>)> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!("không chạy được debugger {}", program.display()))?;
        let mut stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::unbounded::<Vec<u8>>();
        let (etx, erx) = mpsc::unbounded();
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        std::thread::spawn(move || {
            for bytes in futures::executor::block_on_stream(rx) {
                if stdin.write_all(&bytes).and_then(|_| stdin.flush()).is_err() {
                    break;
                }
            }
        });
        {
            let pending = pending.clone();
            std::thread::spawn(move || {
                let mut r = BufReader::new(stdout);
                while let Some(msg) = read(&mut r) {
                    match msg["type"].as_str() {
                        Some("response") => {
                            let id = msg["request_seq"].as_i64().unwrap_or(-1);
                            if let Some(tx) = pending.lock().unwrap().remove(&id) {
                                let res = if msg["success"].as_bool().unwrap_or(false) {
                                    Ok(msg.get("body").cloned().unwrap_or(Value::Null))
                                } else {
                                    Err(msg["message"].as_str().unwrap_or("lỗi debugger").to_string())
                                };
                                let _ = tx.send(res);
                            }
                        }
                        Some("event") => {
                            let _ = etx.unbounded_send(DapEvent::Event {
                                event: msg["event"].as_str().unwrap_or("").to_string(),
                                body: msg.get("body").cloned().unwrap_or(Value::Null),
                            });
                        }
                        _ => {}
                    }
                }
                pending.lock().unwrap().clear();
                let _ = etx.unbounded_send(DapEvent::Closed);
            });
        }
        Ok((Arc::new(DapClient { seq: AtomicI64::new(1), tx, pending, child: Mutex::new(Some(child)) }), erx))
    }

    pub fn request(&self, command: &str, arguments: Value) -> Pend {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let (otx, orx) = oneshot::channel();
        self.pending.lock().unwrap().insert(seq, otx);
        let _ = self.tx.unbounded_send(frame(&json!({ "seq": seq, "type": "request", "command": command, "arguments": arguments })));
        Pend(orx)
    }

    pub fn kill(&self) {
        if let Some(mut c) = self.child.lock().unwrap().take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Drop for DapClient {
    fn drop(&mut self) {
        self.kill();
    }
}

fn read(r: &mut impl BufRead) -> Option<Value> {
    let mut len = None;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            if len.is_some() {
                break;
            }
            continue;
        }
        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            len = v.trim().parse::<usize>().ok();
        }
    }
    let mut body = vec![0u8; len?];
    r.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}
