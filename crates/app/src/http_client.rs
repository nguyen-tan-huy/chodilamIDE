//! IntelliJ HTTP Client: `.http` / `.rest` files - requests separated by `###`, `METHOD url`,
//! headers, blank line, body; `{{var}}` from http-client.env.json (+ .private) next to the
//! file or at the project root (first environment, or the one named "dev"/"local").
//! Run with the gutter ▶ or Ctrl+Enter; executed through `curl`, response in the Run panel.
use crate::workspace::Workspace;
use gpui::Context;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const METHODS: &[&str] = &["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS"];

#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
    /// 0-based line of the request line
    pub line: usize,
}

pub fn is_http_file(p: &Path) -> bool {
    p.extension().is_some_and(|e| e == "http" || e == "rest")
}

/// `GET http://…` (also `http://…` alone = GET).
pub fn is_request_line(l: &str) -> bool {
    let t = l.trim_start();
    METHODS.iter().any(|m| t.strip_prefix(m).is_some_and(|r| r.starts_with(' '))) || t.starts_with("http://") || t.starts_with("https://")
}

/// The request whose block contains `line`.
pub fn request_at(text: &str, line: usize) -> Option<Request> {
    let lines: Vec<&str> = text.lines().collect();
    // block bounds: ### separators
    let start = (0..=line.min(lines.len().saturating_sub(1))).rev().find(|&i| lines[i].trim_start().starts_with("###")).map_or(0, |i| i + 1);
    let end = (line + 1..lines.len()).find(|&i| lines[i].trim_start().starts_with("###")).unwrap_or(lines.len());
    let req_line = (start..end).find(|&i| is_request_line(lines[i]))?;
    let first = lines[req_line].trim();
    let (method, rest) = match METHODS.iter().find(|m| first.starts_with(&format!("{m} "))) {
        Some(m) => (m.to_string(), first[m.len()..].trim()),
        None => ("GET".to_string(), first),
    };
    // "GET url HTTP/1.1": drop the version
    let url = rest.split_whitespace().next().unwrap_or("").to_string();
    let mut headers = Vec::new();
    let mut i = req_line + 1;
    while i < end && !lines[i].trim().is_empty() {
        let l = lines[i].trim();
        if !l.starts_with('#') && !l.starts_with("//") {
            if let Some((k, v)) = l.split_once(':') {
                headers.push((k.trim().to_string(), v.trim().to_string()));
            }
        }
        i += 1;
    }
    // body: after the blank line, until the block ends; response handlers (> {% %}) dropped
    let body_lines: Vec<&str> = lines[(i + 1).min(end)..end].iter().copied().take_while(|l| !l.trim_start().starts_with("> ")).collect();
    let body = body_lines.join("\n").trim().to_string();
    Some(Request { method, url, headers, body, line: req_line })
}

/// `{{name}}` replaced from `vars` (unknown names stay as written).
pub fn substitute(s: &str, vars: &BTreeMap<String, String>) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("{{") {
        out.push_str(&rest[..i]);
        let Some(j) = rest[i..].find("}}") else {
            out.push_str(&rest[i..]);
            return out;
        };
        let name = rest[i + 2..i + j].trim();
        match vars.get(name) {
            Some(v) => out.push_str(v),
            None => out.push_str(&rest[i..i + j + 2]),
        }
        rest = &rest[i + j + 2..];
    }
    out.push_str(rest);
    out
}

/// Variables of the environment: http-client.env.json (+ http-client.private.env.json)
/// next to the file, else at the project root. Environment: "dev", else "local", else the first.
pub fn env_vars(file: &Path, root: &Path) -> (Option<String>, BTreeMap<String, String>) {
    let dirs: Vec<PathBuf> = [file.parent().map(Path::to_path_buf), Some(root.to_path_buf())].into_iter().flatten().collect();
    for d in dirs {
        let Ok(s) = std::fs::read_to_string(d.join("http-client.env.json")) else { continue };
        let Ok(v) = serde_json::from_str::<Value>(&s) else { continue };
        let Some(obj) = v.as_object() else { continue };
        let name = ["dev", "local"].iter().find(|n| obj.contains_key(**n)).map(|n| n.to_string()).or_else(|| obj.keys().next().cloned());
        let mut vars = BTreeMap::new();
        let mut add = |v: &Value| {
            for (k, x) in v.as_object().into_iter().flatten() {
                vars.insert(k.clone(), x.as_str().map(String::from).unwrap_or_else(|| x.to_string()));
            }
        };
        if let Some(n) = &name {
            add(&v[n]);
            if let Ok(p) = std::fs::read_to_string(d.join("http-client.private.env.json")) {
                if let Ok(pv) = serde_json::from_str::<Value>(&p) {
                    add(&pv[n]);
                }
            }
        }
        return (name, vars);
    }
    (None, BTreeMap::new())
}

/// curl arguments for a request (-i: status line + headers in the output).
pub fn curl_args(r: &Request) -> Vec<String> {
    let mut a = vec!["-sS".into(), "-i".into(), "--max-time".into(), "60".into(), "-X".into(), r.method.clone(), r.url.clone()];
    for (k, v) in &r.headers {
        a.push("-H".into());
        a.push(format!("{k}: {v}"));
    }
    if !r.body.is_empty() {
        a.push("--data-binary".into());
        a.push("@-".into());
    }
    a
}

impl Workspace {
    /// Runs the request at `line` of the active .http file; the response goes to a Run session.
    pub(crate) fn run_http_request(&mut self, line: usize, cx: &mut Context<Self>) {
        let Some(t) = self.active.and_then(|i| self.tabs.get(i)).cloned() else { return };
        let Some(path) = t.read(cx).path().cloned() else { return };
        let text = t.read(cx).editor.buffer.text();
        let Some(mut req) = request_at(&text, line) else {
            self.message = Some("Không có request ở đây (dòng dạng: GET http://…)".into());
            return cx.notify();
        };
        let (env, vars) = env_vars(&path, &self.root);
        req.url = substitute(&req.url, &vars);
        req.body = substitute(&req.body, &vars);
        for h in req.headers.iter_mut() {
            h.1 = substitute(&h.1, &vars);
        }
        let title = format!("HTTP {} {}", req.method, req.url.split('?').next().unwrap_or(&req.url));
        let id = self.new_http_session(title, cx);
        let header = format!("{} {}{}\n", req.method, req.url, env.map(|e| format!("   (env: {e})")).unwrap_or_default());
        self.session_output(id, &header, false, cx);
        let started = std::time::Instant::now();
        cx.spawn(async move |this, cx| {
            let out = cx
                .background_executor()
                .spawn(async move {
                    use std::io::Write;
                    let mut child = std::process::Command::new("curl")
                        .args(curl_args(&req))
                        .stdin(std::process::Stdio::piped())
                        .stdout(std::process::Stdio::piped())
                        .stderr(std::process::Stdio::piped())
                        .spawn()
                        .map_err(|e| format!("cần 'curl': {e}"))?;
                    if let Some(mut stdin) = child.stdin.take() {
                        let _ = stdin.write_all(req.body.as_bytes());
                    }
                    let o = child.wait_with_output().map_err(|e| e.to_string())?;
                    Ok::<_, String>((String::from_utf8_lossy(&o.stdout).into_owned(), String::from_utf8_lossy(&o.stderr).into_owned(), o.status.code()))
                })
                .await;
            let _ = this.update(cx, |ws, cx| {
                match out {
                    Ok((stdout, stderr, code)) => {
                        ws.session_output(id, &pretty_response(&stdout), false, cx);
                        if !stderr.trim().is_empty() {
                            ws.session_output(id, &stderr, true, cx);
                        }
                        ws.session_output(id, &format!("\n({} ms)\n", started.elapsed().as_millis()), false, cx);
                        ws.session_finished(id, code.map(i64::from), cx);
                    }
                    Err(e) => {
                        ws.session_output(id, &e, true, cx);
                        ws.session_finished(id, None, cx);
                    }
                }
            });
        })
        .detach();
    }
}

/// Status + headers as curl printed them, a JSON body pretty-printed.
pub fn pretty_response(raw: &str) -> String {
    let raw = raw.replace("\r\n", "\n");
    // the last header block (after redirects / 100-continue)
    let (head, body) = match raw.rfind("\n\n") {
        Some(i) if raw[..i].lines().next().is_some_and(|l| l.starts_with("HTTP/")) => {
            let mut split = i;
            // body may itself contain blank lines: split at the first blank line after the last status line
            if let Some(last_status) = raw.rfind("\nHTTP/").map(|p| p + 1).or(Some(0)) {
                if let Some(j) = raw[last_status..].find("\n\n") {
                    split = last_status + j;
                }
            }
            (&raw[..split], raw[split + 2..].to_string())
        }
        _ => ("", raw.clone()),
    };
    let body = match serde_json::from_str::<Value>(body.trim()) {
        Ok(v) if !body.trim().is_empty() => serde_json::to_string_pretty(&v).unwrap_or(body),
        _ => body,
    };
    if head.is_empty() {
        body
    } else {
        format!("{head}\n\n{body}\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_blocks_and_vars() {
        let text = "### list\nGET {{host}}/api/items?page=1\nAccept: application/json\n\n###\n# create\nPOST {{host}}/api/items HTTP/1.1\nContent-Type: application/json\nAuthorization: Bearer {{token}}\n\n{\n  \"name\": \"x\"\n}\n\n> {% client.test(); %}\n";
        let r = request_at(text, 1).unwrap();
        assert_eq!((r.method.as_str(), r.url.as_str(), r.headers.len(), r.body.as_str()), ("GET", "{{host}}/api/items?page=1", 1, ""));
        let r = request_at(text, 9).unwrap();
        assert_eq!((r.method.as_str(), r.line), ("POST", 6));
        assert_eq!(r.body, "{\n  \"name\": \"x\"\n}");
        let vars: BTreeMap<String, String> = [("host".to_string(), "http://localhost:8080".to_string())].into();
        assert_eq!(substitute("{{host}}/a/{{nope}}", &vars), "http://localhost:8080/a/{{nope}}");
        assert!(is_request_line("  DELETE http://x") && !is_request_line("GETTER x"));
        let args = curl_args(&r);
        assert!(args.contains(&"--data-binary".to_string()) && args.contains(&"POST".to_string()));
        let resp = pretty_response("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"a\":1}");
        assert_eq!(resp, "HTTP/1.1 200 OK\nContent-Type: application/json\n\n{\n  \"a\": 1\n}\n");
    }
}
