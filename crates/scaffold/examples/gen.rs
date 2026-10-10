//! `cargo run -p scaffold --example gen -- <dir> <single|multi|child> [boot] [java]` - writes a sample project.
use scaffold::*;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let dir = std::path::PathBuf::from(&a[1]);
    let kind = match a[2].as_str() {
        "multi" => Kind::Multi { module: "demo-app".into() },
        "child" => Kind::Child(Coords { group: "vn.demo".into(), artifact: "demo".into(), version: "0.0.1-SNAPSHOT".into() }),
        _ => Kind::Single,
    };
    let spec = Spec { group: "vn.demo".into(), artifact: if matches!(kind, Kind::Child(_)) { "demo-extra".into() } else { "demo".into() }, description: "Demo".into(), package: default_package("vn.demo", "demo"), java: a.get(4).and_then(|j| j.parse().ok()).unwrap_or(11), boot: a.get(3).cloned().unwrap_or("2.7.12".into()), war: false, deps: vec!["web".into()], kind };
    for p in write(&dir, &spec).unwrap() {
        println!("{}", p.display());
    }
}
