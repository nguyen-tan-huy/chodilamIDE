//! Maven project model shared by the ChoDiLamIDE Rust IDE and java-debug-model (nvim).
pub mod cache;
pub mod maven;
pub mod model;
pub mod pom;

pub use maven::BuildOpts;
pub use model::{Dependency, Library, Module, Project, SourceRoot};

use std::path::Path;

/// Loads (from cache) or resolves the project at `root`, applying `manifest.json`
/// the same way java-debug-model's init.lua does.
pub fn open_project(root: &Path, mut opts: BuildOpts) -> anyhow::Result<Project> {
    let root_s = maven::normalize_root(root);
    let manifest = cache::load_manifest(&root_s);
    opts.manually_added.extend(manifest.added);
    let mut project = maven::build(root, &opts)?;
    if !manifest.excluded.is_empty() {
        project.modules.retain(|m| !manifest.excluded.contains(&m.path));
    }
    Ok(project)
}
