//! `chodilamide lsp check <dir> <file> [line col]` - headless jdtls smoke test:
//! start, wait for ServiceReady, open a file, print diagnostics, go-to-definition.
use anyhow::{Context, Result, bail};
use futures::StreamExt;
use lsp_client::{Client, ServerMessage, jdtls, path_to_uri};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn rss_mb(pid: u32) -> Option<u64> {
    // jdtls' launcher execs java in place, so the pid is the JVM's
    let s = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let kb: u64 = s.lines().find(|l| l.starts_with("VmRSS:"))?.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb / 1024)
}

pub fn run(dir: &Path, file: &Path, pos: Option<(u32, u32)>) -> Result<()> {
    let root = std::path::absolute(dir)?;
    let file = std::path::absolute(root.join(file))?;
    let project = project_model::open_project(&root, project_model::BuildOpts { quiet: true, ..Default::default() })?;
    let modules: Vec<PathBuf> = project.modules.iter().map(|m| PathBuf::from(&m.path)).collect();
    let tools = toolchain::find_tools();
    let jdks = toolchain::find_jdks();
    for n in &tools.notes {
        eprintln!("note: {n}");
    }
    let cfg = jdtls::config(&root, &modules, &tools, &jdks, &jdtls::Memory::default())?;
    println!("jdtls: {}\ndata:  {}\nbundles: {}  lombok: {}  patch-agent: {}  modules: {}",
        cfg.program.display(), cfg.data_dir.display(), tools.bundles.len(),
        tools.lombok.is_some(), tools.patch_agent.is_some(), modules.len());

    let t0 = Instant::now();
    let (client, mut events) = Client::spawn(&cfg.program, &cfg.args, &root, &cfg.env, cfg.settings.clone())?;
    let pid = client.pid().unwrap_or(0);
    let init = futures::executor::block_on(client.request("initialize", jdtls::initialize_params(&root, cfg.init_options.clone())).response())
        .context("initialize")?;
    *client.capabilities.lock().unwrap() = init["capabilities"].clone();
    client.notify("initialized", json!({}));
    println!("initialize: {:.1}s", t0.elapsed().as_secs_f32());

    // wait for ServiceReady
    let deadline = Instant::now() + Duration::from_secs(300);
    let mut ready = false;
    while Instant::now() < deadline {
        match futures::executor::block_on(events.next()) {
            Some(ServerMessage::Notification { method, params }) if method == "language/status" => {
                let ty = params["type"].as_str().unwrap_or("");
                if ty == "ServiceReady" {
                    ready = true;
                    break;
                }
                if ty == "Error" {
                    eprintln!("jdtls: {}", params["message"]);
                }
            }
            Some(ServerMessage::Exited) | None => bail!("jdtls thoát trước khi sẵn sàng"),
            _ => {}
        }
    }
    if !ready {
        bail!("quá 300s chưa ServiceReady");
    }
    println!("ServiceReady: {:.1}s  RSS {} MB", t0.elapsed().as_secs_f32(), rss_mb(pid).unwrap_or(0));

    let uri = path_to_uri(&file);
    let text = std::fs::read_to_string(&file)?;
    client.notify("textDocument/didOpen", json!({ "textDocument": { "uri": uri, "languageId": "java", "version": 1, "text": text } }));

    // diagnostics for our file (jdtls publishes once it has compiled it)
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        let next = futures::executor::block_on(futures::future::select(
            events.next(),
            Box::pin(async { std::thread::sleep(Duration::from_millis(200)); }),
        ));
        if let futures::future::Either::Left((Some(ServerMessage::Notification { method, params }), _)) = next {
            if method == "textDocument/publishDiagnostics" && params["uri"] == uri {
                let diags = params["diagnostics"].as_array().cloned().unwrap_or_default();
                let errors = diags.iter().filter(|d| d["severity"] == 1).count();
                println!("diagnostics: {} ({} error)", diags.len(), errors);
                for d in diags.iter().take(8) {
                    println!("  L{} {}", d["range"]["start"]["line"].as_u64().unwrap_or(0) + 1, d["message"].as_str().unwrap_or(""));
                }
                break;
            }
        }
    }

    if let Some((line, col)) = pos {
        let t = Instant::now();
        let res = futures::executor::block_on(
            client
                .request("textDocument/definition", json!({ "textDocument": { "uri": uri }, "position": { "line": line, "character": col } }))
                .response(),
        )?;
        let loc = if res.is_array() { res[0].clone() } else { res };
        println!("definition ({} ms): {} L{}", t.elapsed().as_millis(), loc["uri"].as_str().unwrap_or("-"), loc["range"]["start"]["line"].as_u64().map(|l| l + 1).unwrap_or(0));
    }
    // Ctrl+N: workspace/symbol
    let t = Instant::now();
    let syms = futures::executor::block_on(client.request("workspace/symbol", json!({ "query": "ConfigProperty" })).response())?;
    let arr = syms.as_array().cloned().unwrap_or_default();
    println!("workspace/symbol 'ConfigProperty' ({} ms): {} kết quả", t.elapsed().as_millis(), arr.len());
    let mut arr = arr;
    arr.sort_by_key(|s| !s["location"]["uri"].as_str().is_some_and(|u| u.starts_with("file:")));
    println!("  trong project: {}", arr.iter().filter(|s| s["location"]["uri"].as_str().is_some_and(|u| u.starts_with("file:"))).count());
    for s in arr.iter().take(3) {
        println!("  {} [{}] {}", s["name"], s["containerName"].as_str().unwrap_or(""), s["location"]["uri"].as_str().unwrap_or("").rsplit('/').next().unwrap_or(""));
    }
    // Shift+F6: only ask for the edit, never apply it
    if let Some((line, col)) = pos {
        let t = Instant::now();
        let edit = futures::executor::block_on(
            client.request("textDocument/rename", json!({ "textDocument": { "uri": uri }, "position": { "line": line, "character": col }, "newName": "ConfigPropertyX" })).response(),
        )?;
        let dc = edit["documentChanges"].as_array().cloned().unwrap_or_default();
        let files = dc.iter().filter(|d| d["textDocument"].is_object()).count() + edit["changes"].as_object().map_or(0, |m| m.len());
        let renames = dc.iter().filter(|d| d["kind"] == "rename").count();
        println!("rename (chỉ hỏi, KHÔNG áp dụng) ({} ms): {} file cần sửa, {} file đổi tên", t.elapsed().as_millis(), files, renames);
    }
    println!("RSS cuối: {} MB", rss_mb(pid).unwrap_or(0));
    client.shutdown_blocking(Duration::from_secs(5));
    Ok(())
}

/// `chodilamide lsp watch-check <dir>`: does jdtls follow files changed behind its back
/// when told via didChangeWatchedFiles, and survive projectConfigurationsUpdate?
pub fn watch_check(dir: &Path) -> Result<()> {
    let root = std::path::absolute(dir)?;
    let project = project_model::open_project(&root, project_model::BuildOpts { quiet: true, ..Default::default() })?;
    let module = project.modules.first().context("không có module")?;
    let src = module.source_roots.iter().find(|s| s.kind == "main").context("không có src/main/java")?;
    let modules: Vec<PathBuf> = project.modules.iter().map(|m| PathBuf::from(&m.path)).collect();
    let cfg = jdtls::config(&root, &modules, &toolchain::find_tools(), &toolchain::find_jdks(), &jdtls::Memory::default())?;
    let (client, mut events) = Client::spawn(&cfg.program, &cfg.args, &root, &cfg.env, cfg.settings.clone())?;
    futures::executor::block_on(client.request("initialize", jdtls::initialize_params(&root, cfg.init_options.clone())).response())?;
    client.notify("initialized", json!({}));
    loop {
        match futures::executor::block_on(events.next()) {
            Some(ServerMessage::Notification { method, params }) if method == "language/status" && params["type"] == "ServiceReady" => break,
            Some(ServerMessage::Exited) | None => bail!("jdtls thoát"),
            _ => {}
        }
    }
    // drain server messages in the background so the pipe never fills up
    std::thread::spawn(move || while futures::executor::block_on(events.next()).is_some() {});
    let find = |name: &str| -> usize {
        let r = futures::executor::block_on(client.request("workspace/symbol", json!({ "query": name })).response()).unwrap_or_default();
        r.as_array().map(|a| a.iter().filter(|s| s["name"] == name).count()).unwrap_or(0)
    };
    let wait_for = |name: &str, want: usize| -> bool {
        let end = Instant::now() + Duration::from_secs(20);
        while Instant::now() < end {
            if find(name) == want {
                return true;
            }
            std::thread::sleep(Duration::from_millis(300));
        }
        false
    };
    let class = "ZzWatchProbe";
    let file = Path::new(&src.path).join(format!("{class}.java"));
    std::fs::write(&file, format!("public class {class} {{ }}\n"))?;
    client.notify("workspace/didChangeWatchedFiles", json!({ "changes": [{ "uri": path_to_uri(&file), "type": 1 }] }));
    println!("{} tạo file + báo created -> Ctrl+N thấy class", if wait_for(class, 1) { "PASS" } else { "FAIL" });
    std::fs::remove_file(&file)?;
    client.notify("workspace/didChangeWatchedFiles", json!({ "changes": [{ "uri": path_to_uri(&file), "type": 3 }] }));
    println!("{} xóa file + báo deleted -> class biến mất", if wait_for(class, 0) { "PASS" } else { "FAIL" });
    let poms: Vec<_> = modules.iter().map(|m| json!({ "uri": path_to_uri(&m.join("pom.xml")) })).collect();
    client.notify("java/projectConfigurationsUpdate", json!({ "identifiers": poms }));
    let build = futures::executor::block_on(client.request("java/buildWorkspace", json!(false)).response());
    println!("{} projectConfigurationsUpdate + buildWorkspace -> jdtls vẫn trả lời ({:?})", if build.is_ok() { "PASS" } else { "FAIL" }, build.ok());
    client.shutdown_blocking(Duration::from_secs(5));
    Ok(())
}

/// `chodilamide lsp complete <dir> <file> <line> <col>`: the raw completion / signature help
/// jdtls returns at a position (1-based line, 0-based UTF-16 column).
pub fn complete(dir: &Path, file: &Path, line: u32, col: u32) -> Result<()> {
    let root = std::path::absolute(dir)?;
    let file = std::path::absolute(root.join(file))?;
    let project = project_model::open_project(&root, project_model::BuildOpts { quiet: true, ..Default::default() })?;
    let modules: Vec<PathBuf> = project.modules.iter().map(|m| PathBuf::from(&m.path)).collect();
    let cfg = jdtls::config(&root, &modules, &toolchain::find_tools(), &toolchain::find_jdks(), &jdtls::Memory::default())?;
    let (client, mut events) = Client::spawn(&cfg.program, &cfg.args, &root, &cfg.env, cfg.settings.clone())?;
    futures::executor::block_on(client.request("initialize", jdtls::initialize_params(&root, cfg.init_options.clone())).response())?;
    client.notify("initialized", json!({}));
    loop {
        match futures::executor::block_on(events.next()) {
            Some(ServerMessage::Notification { method, params }) if method == "language/status" && params["type"] == "ServiceReady" => break,
            Some(ServerMessage::Exited) | None => bail!("jdtls thoát"),
            _ => {}
        }
    }
    std::thread::spawn(move || while futures::executor::block_on(events.next()).is_some() {});
    let uri = path_to_uri(&file);
    // CHODILAMIDE_TEXT=<file>: send that content for `file` (try edits without touching it)
    let text = match std::env::var_os("CHODILAMIDE_TEXT") {
        Some(p) => std::fs::read_to_string(p)?,
        None => std::fs::read_to_string(&file)?,
    };
    client.notify("textDocument/didOpen", json!({ "textDocument": { "uri": uri, "languageId": "java", "version": 1, "text": text } }));
    let wait = std::env::var("CHODILAMIDE_WAIT").ok().and_then(|w| w.parse().ok()).unwrap_or(2);
    std::thread::sleep(Duration::from_secs(wait));
    let pos = json!({ "textDocument": { "uri": uri }, "position": { "line": line - 1, "character": col } });
    let r = futures::executor::block_on(client.request("textDocument/completion", pos.clone()).response())?;
    let items = if r.is_array() { r.as_array().cloned().unwrap_or_default() } else { r["items"].as_array().cloned().unwrap_or_default() };
    println!("{} gợi ý; 4 cái đầu:", items.len());
    for it in items.iter().take(std::env::var("CHODILAMIDE_N").ok().and_then(|n| n.parse().ok()).unwrap_or(4)) {
        println!("  label={} kind={} labelDetails={} detail={} insertText={} filterText={} textEdit={}", it["label"], it["kind"], it["labelDetails"], it["detail"], it["insertText"], it["filterText"], it["textEdit"]);
    }
    let sig = futures::executor::block_on(client.request("textDocument/signatureHelp", pos).response())?;
    println!("signatureHelp: {}", serde_json::to_string(&sig)?.chars().take(400).collect::<String>());
    client.shutdown_blocking(Duration::from_secs(5));
    Ok(())
}

/// `chodilamide lsp refs <dir> <file> <line> <col>`: go to the definition of the symbol at a
/// call site (often a jdt:// class file of a library), then ask for its references there - the
/// same two steps as Ctrl+B then Alt+F7 in the IDE.
pub fn refs(dir: &Path, file: &Path, line: u32, col: u32) -> Result<()> {
    let root = std::path::absolute(dir)?;
    let file = std::path::absolute(root.join(file))?;
    let project = project_model::open_project(&root, project_model::BuildOpts { quiet: true, ..Default::default() })?;
    let modules: Vec<PathBuf> = project.modules.iter().map(|m| PathBuf::from(&m.path)).collect();
    let cfg = jdtls::config(&root, &modules, &toolchain::find_tools(), &toolchain::find_jdks(), &jdtls::Memory::default())?;
    let (client, mut events) = Client::spawn(&cfg.program, &cfg.args, &root, &cfg.env, cfg.settings.clone())?;
    let init = futures::executor::block_on(client.request("initialize", jdtls::initialize_params(&root, cfg.init_options.clone())).response())?;
    *client.capabilities.lock().unwrap() = init["capabilities"].clone();
    client.notify("initialized", json!({}));
    loop {
        match futures::executor::block_on(events.next()) {
            Some(ServerMessage::Notification { method, params }) if method == "language/status" && params["type"] == "ServiceReady" => break,
            Some(ServerMessage::Exited) | None => bail!("jdtls thoát"),
            _ => {}
        }
    }
    std::thread::spawn(move || while futures::executor::block_on(events.next()).is_some() {});
    let uri = path_to_uri(&file);
    client.notify("textDocument/didOpen", json!({ "textDocument": { "uri": uri, "languageId": "java", "version": 1, "text": std::fs::read_to_string(&file)? } }));
    std::thread::sleep(Duration::from_secs(2));
    let pos = json!({ "textDocument": { "uri": uri }, "position": { "line": line - 1, "character": col } });
    let show = |label: &str, r: &serde_json::Value| {
        let a = r.as_array().cloned().unwrap_or_default();
        println!("{label}: {} chỗ", a.len());
        for l in a.iter().take(8) {
            let u = l["uri"].as_str().unwrap_or("");
            println!("   {}:{}", u.rsplit('/').next().unwrap_or(u), l["range"]["start"]["line"].as_u64().unwrap_or(0) + 1);
        }
    };
    let mut p = pos.clone();
    p["context"] = json!({ "includeDeclaration": false });
    show("references từ chỗ gọi", &futures::executor::block_on(client.request("textDocument/references", p).response())?);
    let def = futures::executor::block_on(client.request("textDocument/definition", pos.clone()).response())?;
    let loc = if def.is_array() { def[0].clone() } else { def };
    println!("definition: {}", loc["uri"].as_str().unwrap_or("").chars().take(160).collect::<String>());
    let p = json!({ "textDocument": { "uri": loc["uri"] }, "position": loc["range"]["start"], "context": { "includeDeclaration": false } });
    show("references từ định nghĩa", &futures::executor::block_on(client.request("textDocument/references", p).response())?);
    // what Ctrl+Alt+B / Ctrl+H / Ctrl+Alt+H / Alt+Insert ask
    let req = |m: &str, p: serde_json::Value| futures::executor::block_on(client.request(m, p).response()).unwrap_or(serde_json::Value::Null);
    let names = |v: &serde_json::Value| v.as_array().map(|a| a.iter().map(|i| i["name"].as_str().or(i["from"]["name"].as_str()).unwrap_or("?").to_string()).collect::<Vec<_>>().join(", ")).unwrap_or_default();
    show("implementation", &req("textDocument/implementation", pos.clone()));
    let th = req("textDocument/prepareTypeHierarchy", pos.clone());
    println!("prepareTypeHierarchy: {}", names(&th));
    if let Some(item) = th.as_array().and_then(|a| a.first()).cloned() {
        println!("  supertypes: {}", names(&req("typeHierarchy/supertypes", json!({ "item": item }))));
        println!("  subtypes: {}", names(&req("typeHierarchy/subtypes", json!({ "item": item }))));
    }
    let ch = req("textDocument/prepareCallHierarchy", pos.clone());
    println!("prepareCallHierarchy: {}", names(&ch));
    if let Some(item) = ch.as_array().and_then(|a| a.first()).cloned() {
        println!("  incomingCalls: {}", names(&req("callHierarchy/incomingCalls", json!({ "item": item }))));
    }
    let r = json!({ "start": pos["position"], "end": pos["position"] });
    let acts = req("textDocument/codeAction", json!({ "textDocument": pos["textDocument"], "range": r, "context": { "diagnostics": [], "triggerKind": 1 } }));
    let src: Vec<String> = acts.as_array().into_iter().flatten().filter(|a| a["kind"].as_str().is_some_and(|k| k.starts_with("source"))).map(|a| format!("{} [{}]", a["title"].as_str().unwrap_or(""), a["kind"].as_str().unwrap_or(""))).collect();
    println!("source actions (Alt+Insert): {}", src.join(" | "));
    let sem = req("textDocument/semanticTokens/full", json!({ "textDocument": pos["textDocument"] }));
    let n = sem["data"].as_array().map_or(0, |d| d.len() / 5);
    let legend = client.capabilities.lock().unwrap()["semanticTokensProvider"]["legend"]["tokenTypes"].clone();
    println!("semantic tokens: {n} token, legend: {}", legend.as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(",")).unwrap_or_default());
    // the toString prompt, answered like the IDE does (every field)
    if let Some(a) = acts.as_array().into_iter().flatten().find(|a| a["command"]["command"] == "java.action.generateToStringPrompt") {
        let params = a["command"]["arguments"][0].clone();
        let status = req("java/checkToStringStatus", params.clone());
        let edit = req("java/generateToString", json!({ "context": params, "fields": status["fields"] }));
        let text: String = edit["changes"].as_object().into_iter().flat_map(|m| m.values()).flat_map(|v| v.as_array().cloned().unwrap_or_default()).map(|e| e["newText"].as_str().unwrap_or("").to_string()).collect();
        println!("generateToString: {} field, {} ký tự: {}", status["fields"].as_array().map_or(0, |f| f.len()), text.len(), text.trim().lines().take(3).collect::<Vec<_>>().join(" ⏎ "));
    }
    client.shutdown_blocking(Duration::from_secs(5));
    Ok(())
}

/// `chodilamide lsp progress <dir>`: prints jdtls' language/progressReport jobs while it starts.
pub fn progress(dir: &Path) -> Result<()> {
    let root = std::path::absolute(dir)?;
    let project = project_model::open_project(&root, project_model::BuildOpts { quiet: true, ..Default::default() })?;
    let modules: Vec<PathBuf> = project.modules.iter().map(|m| PathBuf::from(&m.path)).collect();
    let cfg = jdtls::config(&root, &modules, &toolchain::find_tools(), &toolchain::find_jdks(), &jdtls::Memory::default())?;
    let (client, mut events) = Client::spawn(&cfg.program, &cfg.args, &root, &cfg.env, cfg.settings.clone())?;
    futures::executor::block_on(client.request("initialize", jdtls::initialize_params(&root, cfg.init_options.clone())).response())?;
    client.notify("initialized", json!({}));
    let t0 = Instant::now();
    let mut ready_at = None;
    while let Some(m) = futures::executor::block_on(events.next()) {
        match m {
            ServerMessage::Notification { method, params } if method == "language/progressReport" => {
                println!("{:>6.1}s {} {}/{} {} {}", t0.elapsed().as_secs_f32(), params["task"].as_str().unwrap_or(""), params["workDone"], params["totalWork"], params["status"].as_str().unwrap_or(""), if params["complete"].as_bool() == Some(true) { "✓" } else { "" });
            }
            ServerMessage::Notification { method, params } if method == "language/status" => {
                println!("{:>6.1}s status {} {}", t0.elapsed().as_secs_f32(), params["type"], params["message"]);
                if params["type"] == "ServiceReady" {
                    ready_at = Some(Instant::now());
                }
            }
            ServerMessage::Exited => break,
            _ => {}
        }
        if ready_at.is_some_and(|t| t.elapsed() > Duration::from_secs(15)) {
            break;
        }
    }
    client.shutdown_blocking(Duration::from_secs(5));
    Ok(())
}

/// `chodilamide lsp actions <dir> <file> <l1> <c1> <l2> <c2> [kind]`: code actions jdtls offers
/// for a range (e.g. refactor.extract.variable), with the edit of the first one.
pub fn actions(dir: &Path, file: &Path, r: [u32; 4], kind: Option<&str>) -> Result<()> {
    let root = std::path::absolute(dir)?;
    let file = std::path::absolute(root.join(file))?;
    let project = project_model::open_project(&root, project_model::BuildOpts { quiet: true, ..Default::default() })?;
    let modules: Vec<PathBuf> = project.modules.iter().map(|m| PathBuf::from(&m.path)).collect();
    let cfg = jdtls::config(&root, &modules, &toolchain::find_tools(), &toolchain::find_jdks(), &jdtls::Memory::default())?;
    let (client, mut events) = Client::spawn(&cfg.program, &cfg.args, &root, &cfg.env, cfg.settings.clone())?;
    futures::executor::block_on(client.request("initialize", jdtls::initialize_params(&root, cfg.init_options.clone())).response())?;
    client.notify("initialized", json!({}));
    loop {
        match futures::executor::block_on(events.next()) {
            Some(ServerMessage::Notification { method, params }) if method == "language/status" && params["type"] == "ServiceReady" => break,
            Some(ServerMessage::Exited) | None => bail!("jdtls thoát"),
            _ => {}
        }
    }
    std::thread::spawn(move || while futures::executor::block_on(events.next()).is_some() {});
    let uri = path_to_uri(&file);
    let text = match std::env::var_os("CHODILAMIDE_TEXT") {
        Some(p) => std::fs::read_to_string(p)?,
        None => std::fs::read_to_string(&file)?,
    };
    client.notify("textDocument/didOpen", json!({ "textDocument": { "uri": uri, "languageId": "java", "version": 1, "text": text } }));
    std::thread::sleep(Duration::from_secs(2));
    let mut ctx = json!({ "diagnostics": [], "triggerKind": 1 });
    if let Some(k) = kind {
        ctx["only"] = json!([k]);
    }
    let range = json!({ "start": { "line": r[0] - 1, "character": r[1] }, "end": { "line": r[2] - 1, "character": r[3] } });
    let res = futures::executor::block_on(client.request("textDocument/codeAction", json!({ "textDocument": { "uri": uri }, "range": range, "context": ctx })).response())?;
    for a in res.as_array().into_iter().flatten() {
        println!("- {} [{}] edit={} command={}", a["title"].as_str().unwrap_or(""), a["kind"].as_str().unwrap_or(""), a["edit"].is_object(), a["command"]["command"].as_str().or(a["command"].as_str()).unwrap_or("-"));
    }
    if let Some(a) = res.as_array().and_then(|a| a.first()) {
        let a = if a["edit"].is_object() || a["command"].is_object() { a.clone() } else { futures::executor::block_on(client.request("codeAction/resolve", a.clone()).response())? };
        println!("{}", serde_json::to_string(&a).unwrap_or_default().chars().take(1500).collect::<String>());
    }
    let fmt = futures::executor::block_on(client.request("textDocument/rangeFormatting", json!({ "textDocument": { "uri": uri }, "range": range, "options": { "tabSize": 4, "insertSpaces": true } })).response());
    println!("rangeFormatting: {}", fmt.map(|v| v.as_array().map(|a| a.len()).unwrap_or(0).to_string()).unwrap_or_else(|e| e.to_string()));
    client.shutdown_blocking(Duration::from_secs(5));
    Ok(())
}
