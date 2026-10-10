//! `chodilamide jsf check <dir> <file.xhtml>` - headless JSF smoke test against a real jdtls:
//! resolves every EL reference of the page (Ctrl+B target) and lists member completions after
//! each `bean.`.
use anyhow::{Result, bail};
use futures::StreamExt;
use lsp_client::{Client, ServerMessage, jdtls};
use serde_json::json;
use std::path::{Path, PathBuf};

pub fn check(dir: &Path, file: &Path) -> Result<()> {
    let root = std::path::absolute(dir)?;
    let file = std::path::absolute(root.join(file))?;
    let project = project_model::open_project(&root, project_model::BuildOpts { quiet: true, ..Default::default() })?;
    let t0 = std::time::Instant::now();
    let index = jsf::beans::BeanIndex::build(&project);
    println!("bean index: {} bean ({} ms)", index.candidates.len(), t0.elapsed().as_millis());
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
    let page = std::fs::read_to_string(&file)?;
    let mut ctx = jsf::resolve::Ctx::new(&client, &index, &page, &file);
    for sp in jsf::el::el_spans(&page) {
        let expr = &page[sp.start..=sp.end];
        let line = page[..sp.start].matches('\n').count() + 1;
        let t = std::time::Instant::now();
        let r = jsf::el::parse_el(&page, sp.end - 1);
        let res = match &r {
            Some(r) => match futures::executor::block_on(ctx.resolve(r, sp.end - 1)) {
                Ok(jsf::resolve::Target::Java(loc)) => {
                    let uri = loc["uri"].as_str().unwrap_or("");
                    let short = uri.rsplit('/').next().unwrap_or(uri);
                    format!("→ {}:{}", short.split('?').next().unwrap_or(short), loc["range"]["start"]["line"].as_u64().unwrap_or(0) + 1)
                }
                Ok(jsf::resolve::Target::PageVar(off)) => format!("→ biến trang (offset {off})"),
                Err(e) => format!("✖ {e}"),
            },
            None => "(không phải tham chiếu)".into(),
        };
        println!("L{line:<3} {:<42} {res}  [{} ms]", expr.replace(char::is_whitespace, ""), t.elapsed().as_millis());
    }
    // completion after "helloBean." style prefixes of the first bean used on the page
    if let Some(bean) = jsf::el::el_spans(&page).iter().find_map(|sp| jsf::el::parse_el(&page, sp.end - 1)).map(|r| r.bean) {
        let text = format!("{page}#{{{bean}.");
        let pos = text.len();
        let file2 = file.clone();
        let mut ctx = jsf::resolve::Ctx::new(&client, &index, &text, &file2);
        if let Some(jsf::el::CompletionContext { kind: jsf::el::CompletionKind::Member(r), .. }) = jsf::el::completion_context(&text, pos) {
            let t = futures::executor::block_on(ctx.type_of(&r, pos))?;
            let members = futures::executor::block_on(ctx.members_of(&t))?;
            let items = jsf::resolve::member_items(&members);
            let labels: Vec<String> = items.iter().map(|i| format!("{} ({})", i["label"].as_str().unwrap_or(""), i["labelDetails"]["description"].as_str().unwrap_or(""))).collect();
            println!("gợi ý sau '#{{{bean}.': {}", labels.join(", "));
        }
    }
    // inspections (what the editor underlines), on the page plus a broken copy
    for (label, text) in [("trang", page.clone()), ("bản cố tình sai", page.replacen("helloBean.name}", "helloBean.khongCo}", 1).replacen("addr.country", "addr.quocGia", 1))] {
        let mut d = jsf::diagnostics::static_checks(&text);
        let mut ctx = jsf::resolve::Ctx::new(&client, &index, &text, &file);
        let t = std::time::Instant::now();
        d.extend(futures::executor::block_on(jsf::diagnostics::el_checks(&mut ctx, &index, &text, &file)));
        println!("Kiểm tra {label}: {} vấn đề ({} ms)", d.len(), t.elapsed().as_millis());
        for x in d {
            println!("  L{} [{}] {} - {}", text[..x.start].matches('\n').count() + 1, x.code, &text[x.start..x.end], x.message);
        }
    }
    client.shutdown_blocking(std::time::Duration::from_secs(5));
    Ok(())
}
