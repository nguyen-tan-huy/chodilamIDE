//! Drives chodilamide-debugger the way the IDE does (initialize → launch → on `initialized`
//! setBreakpoints + configurationDone → stopped → stackTrace/scopes/variables → continue →
//! exited → terminated). Skipped when the debugger jar or JDKs are missing.
use dap_client::{DapClient, DapEvent};
use futures::StreamExt;
use serde_json::json;
use std::path::PathBuf;

#[test]
fn debug_sample_across_modules() {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../debugger");
    let jar = base.join("build/chodilamide-debugger.jar");
    let s = base.join("test/sample");
    let java21 = PathBuf::from("/usr/lib/jvm/java-21-openjdk/bin/java");
    if !jar.exists() || !java21.exists() || !s.join("app/out").exists() {
        eprintln!("skipped: debugger jar / JDK / compiled sample missing");
        return;
    }
    let greeter = s.join("core/src/vn/demo/core/Greeter.java");
    let (c, mut ev) = DapClient::spawn(&java21, &["-jar".into(), jar.display().to_string()]).unwrap();
    futures::executor::block_on(async {
        c.request("initialize", json!({ "adapterID": "java" })).response().await.unwrap();
        let launch = c.request("launch", json!({
            "mainClass": "vn.demo.app.Main",
            "classPaths": [s.join("app/out"), s.join("core/out")],
            "sourcePaths": [s.join("app/src"), s.join("core/src")],
            "env": {}, "vmArgs": "", "args": "", "noDebug": false
        }));
        launch.response().await.unwrap();
        let mut out = String::new();
        let mut stopped_at = None;
        let mut order = Vec::new();
        while let Some(e) = ev.next().await {
            let DapEvent::Event { event, body } = e else { break };
            order.push(event.clone());
            match event.as_str() {
                "initialized" => {
                    c.request("setBreakpoints", json!({ "source": { "path": greeter }, "breakpoints": [{ "line": 13 }] }));
                    c.request("configurationDone", json!({}));
                }
                "stopped" if stopped_at.is_none() => {
                    let t = body["threadId"].clone();
                    let st = c.request("stackTrace", json!({ "threadId": t })).response().await.unwrap();
                    let f = &st["stackFrames"][0];
                    stopped_at = Some((f["source"]["path"].as_str().unwrap().to_string(), f["line"].as_u64().unwrap()));
                    let sc = c.request("scopes", json!({ "frameId": f["id"] })).response().await.unwrap();
                    let vars = c.request("variables", json!({ "variablesReference": sc["scopes"][0]["variablesReference"] })).response().await.unwrap();
                    assert!(vars["variables"].as_array().unwrap().iter().any(|v| v["name"] == "name" && v["value"] == "\"Huy\""), "{vars}");
                    // remove the breakpoint and let it finish
                    c.request("setBreakpoints", json!({ "source": { "path": greeter }, "breakpoints": [] })).response().await.unwrap();
                    c.request("continue", json!({ "threadId": t })).response().await.unwrap();
                }
                "output" => out.push_str(body["output"].as_str().unwrap_or("")),
                "terminated" => break,
                _ => {}
            }
        }
        let (path, line) = stopped_at.expect("never stopped");
        assert!(path.ends_with("Greeter.java") && line == 13, "{path}:{line}");
        assert!(out.contains("Xin chào Minh #3 (2)"), "{out}");
        let exited = order.iter().position(|e| e == "exited").expect("no exited");
        let terminated = order.iter().position(|e| e == "terminated").unwrap();
        assert!(exited < terminated, "{order:?}");
    });
}
