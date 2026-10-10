//! Project tree, IntelliJ style: the project folder at the top with its real folder structure;
//! a folder that is a Maven module is marked as one (module icon, source roots first, packages
//! compacted) right where it sits, and "Dependencies" lists the jars of ALL modules once, each jar a
//! single time. Pure data; the workspace renders `rows()`.
use project_model::Project;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NodeId {
    Module(PathBuf),
    /// A directory shown under a source root (packages compacted: `com.example.web`).
    Dir(PathBuf),
    File(PathBuf),
    /// "Dependencies (N)" of the project (keyed by the project root): every jar of every module, once
    Deps(PathBuf),
    /// a jar in ~/.m2 (right-click: delete from the local repository)
    Jar(PathBuf),
    /// the project folder at the top of the tree
    RootDir,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Module,
    ModuleIndependent,
    SourceMain,
    SourceTest,
    Resources,
    Package,
    Folder,
    Java,
    Xml,
    File,
    Library,
    Jar,
}

#[derive(Debug, Clone)]
pub struct Row {
    pub id: NodeId,
    pub depth: usize,
    pub label: String,
    pub icon: Icon,
    pub expandable: bool,
    pub expanded: bool,
}

#[derive(Clone)]
struct DirEntry {
    path: PathBuf,
    is_dir: bool,
}

pub struct ProjectTree {
    pub root: PathBuf,
    /// show dotfiles/dot-dirs and build output (target, .git, .mvn, .gitignore…)
    pub show_hidden: bool,
    expanded: HashSet<NodeId>,
    children: HashMap<PathBuf, Vec<DirEntry>>,
    /// the Maven modules of the current model by folder (set at the start of `rows`)
    modules: HashMap<PathBuf, project_model::Module>,
    /// the module the project root is (single-module projects) opens by itself, once
    seeded: bool,
}

/// Hidden unless "show hidden" is on (besides every name starting with '.').
const HIDDEN_DIRS: &[&str] = &["target", "node_modules", "bin", "out"];

impl ProjectTree {
    pub fn new(root: PathBuf) -> Self {
        // the project folder starts open
        Self { root, show_hidden: false, expanded: HashSet::from([NodeId::RootDir]), children: HashMap::new(), modules: HashMap::new(), seeded: false }
    }

    pub fn toggle(&mut self, id: &NodeId) {
        if !self.expanded.remove(id) {
            self.expanded.insert(id.clone());
        }
    }

    pub fn expand(&mut self, id: NodeId) {
        self.expanded.insert(id);
    }

    /// Drops cached directory listings (after a save/create/delete or model reload).
    pub fn refresh(&mut self) {
        self.children.clear();
    }

    fn list(&mut self, dir: &Path) -> Vec<DirEntry> {
        if let Some(v) = self.children.get(dir) {
            return v.clone();
        }
        let mut v: Vec<DirEntry> = std::fs::read_dir(dir)
            .map(|rd| {
                rd.flatten()
                    .filter_map(|e| {
                        let name = e.file_name().to_string_lossy().to_string();
                        let is_dir = e.file_type().ok()?.is_dir();
                        let hidden = name.starts_with('.') || (is_dir && HIDDEN_DIRS.contains(&name.as_str()));
                        if hidden && !self.show_hidden {
                            return None;
                        }
                        Some(DirEntry { path: e.path(), is_dir })
                    })
                    .collect()
            })
            .unwrap_or_default();
        // folders first, then by name (case-insensitive)
        v.sort_by_key(|e| (!e.is_dir, file_name(&e.path).to_lowercase()));
        self.children.insert(dir.to_path_buf(), v.clone());
        v
    }

    /// Follows single-subdirectory chains: `com/example/web` → ("com.example.web", deepest dir).
    fn compact(&mut self, dir: &Path) -> (String, PathBuf) {
        let mut label = file_name(dir);
        let mut cur = dir.to_path_buf();
        loop {
            let kids = self.list(&cur);
            if kids.len() == 1 && kids[0].is_dir {
                label = format!("{label}.{}", file_name(&kids[0].path));
                cur = kids[0].path.clone();
            } else {
                return (label, cur);
            }
        }
    }

    /// Expands every node leading to `file` (project folder, folders, modules, source roots,
    /// packages) so the next `rows()` contains it — "Select Opened File" in IntelliJ.
    pub fn reveal(&mut self, project: Option<&Project>, file: &Path) {
        let module_dirs: HashSet<PathBuf> = project.map(|p| p.modules.iter().map(|m| PathBuf::from(&m.path)).collect()).unwrap_or_default();
        self.expanded.insert(NodeId::RootDir);
        // every folder from the project root down to the file: a module's id is its own kind;
        // source roots, compacted packages (the deepest dir of the chain) and plain folders are dirs
        let mut dirs = Vec::new();
        let mut dir = file.parent();
        while let Some(d) = dir {
            if !d.starts_with(&self.root) {
                break;
            }
            dirs.push(d.to_path_buf());
            dir = d.parent();
        }
        for d in dirs {
            self.expanded.insert(if module_dirs.contains(&d) { NodeId::Module(d) } else { NodeId::Dir(d) });
        }
    }

    pub fn rows(&mut self, project: Option<&Project>) -> Vec<Row> {
        let mut rows = Vec::new();
        self.modules = project.map(|p| p.modules.iter().map(|m| (PathBuf::from(&m.path), m.clone())).collect()).unwrap_or_default();
        let root = self.root.clone();
        if !self.seeded && project.is_some() {
            self.seeded = true;
            if self.modules.contains_key(&root) {
                self.expanded.insert(NodeId::Module(root.clone()));
            }
        }
        if self.modules.contains_key(&root) {
            // a single-module project: the project folder is the module
            self.push_module(&mut rows, &root, 0);
        } else {
            let expanded = self.expanded.contains(&NodeId::RootDir);
            rows.push(Row { id: NodeId::RootDir, depth: 0, label: file_name(&root), icon: Icon::Folder, expandable: true, expanded });
            if expanded {
                self.push_dir_children(&mut rows, &root, 1, false);
            }
        }
        self.push_dependencies(&mut rows, &root);
        rows
    }

    /// "Dependencies (N)": the jars of every module, each one once (two modules on the same jar
    /// list it a single time; two versions of an artifact are two jars).
    fn push_dependencies(&mut self, rows: &mut Vec<Row>, root: &Path) {
        let mut seen: HashSet<&str> = HashSet::new();
        let mut jars: Vec<&str> = Vec::new();
        for m in self.modules.values() {
            for j in m.classpath_jars.iter().flatten() {
                if j.ends_with(".jar") && seen.insert(j.as_str()) {
                    jars.push(j.as_str());
                }
            }
        }
        if jars.is_empty() {
            return;
        }
        jars.sort_by_key(|j| (file_name(Path::new(j)).to_lowercase(), j.to_string()));
        let id = NodeId::Deps(root.to_path_buf());
        let expanded = self.expanded.contains(&id);
        rows.push(Row { id, depth: 0, label: format!("Dependencies ({})", jars.len()), icon: Icon::Library, expandable: true, expanded });
        if expanded {
            for j in jars {
                let p = PathBuf::from(j);
                // the model still lists a jar deleted from ~/.m2 until the next reload
                let label = if p.exists() { file_name(&p) } else { format!("{}  (đã xóa)", file_name(&p)) };
                rows.push(Row { id: NodeId::Jar(p), depth: 1, label, icon: Icon::Jar, expandable: false, expanded: false });
            }
        }
    }

    /// A module folder: the module row, its source roots (and resources), then everything else
    /// in the folder (pom.xml, README, other folders - which may be modules too).
    fn push_module(&mut self, rows: &mut Vec<Row>, module_dir: &Path, depth: usize) {
        let Some(m) = self.modules.get(module_dir).cloned() else { return };
        let id = NodeId::Module(module_dir.to_path_buf());
        let expanded = self.expanded.contains(&id);
        let name = file_name(module_dir);
        rows.push(Row {
            id,
            depth,
            label: if m.in_reactor { name } else { format!("{name}  [independent]") },
            icon: if m.in_reactor { Icon::Module } else { Icon::ModuleIndependent },
            expandable: true,
            expanded,
        });
        if !expanded {
            return;
        }
        let mut roots: Vec<(PathBuf, Icon)> =
            m.source_roots.iter().map(|sr| (PathBuf::from(&sr.path), if sr.kind == "test" { Icon::SourceTest } else { Icon::SourceMain })).collect();
        // resources + JSF webapp: not source roots in the model, but where .xml/.xhtml live
        for extra in ["src/main/resources", "src/main/webapp", "src/test/resources"] {
            let p = module_dir.join(extra);
            if p.is_dir() && !roots.iter().any(|(r, _)| *r == p) {
                roots.push((p, Icon::Resources));
            }
        }
        for (path, icon) in roots {
            let id = NodeId::Dir(path.clone());
            let expanded = self.expanded.contains(&id);
            let label = path.strip_prefix(module_dir).map(|p| p.display().to_string()).unwrap_or_else(|_| path.display().to_string());
            rows.push(Row { id, depth: depth + 1, label, icon, expandable: true, expanded });
            if expanded {
                self.push_dir_children(rows, &path, depth + 2, icon != Icon::Resources);
            }
        }
        // src/ is already shown through its source roots above
        for e in self.list(module_dir) {
            if e.is_dir && file_name(&e.path) == "src" {
                continue;
            }
            if e.is_dir {
                self.push_folder(rows, &e.path, depth + 1);
            } else {
                rows.push(file_row(&e.path, depth + 1));
            }
        }
    }

    /// A plain folder row (or a module, when the folder is one) with its children when open.
    fn push_folder(&mut self, rows: &mut Vec<Row>, dir: &Path, depth: usize) {
        if self.modules.contains_key(dir) {
            return self.push_module(rows, dir, depth);
        }
        let id = NodeId::Dir(dir.to_path_buf());
        let expanded = self.expanded.contains(&id);
        rows.push(Row { id, depth, label: file_name(dir), icon: Icon::Folder, expandable: true, expanded });
        if expanded {
            self.push_dir_children(rows, dir, depth + 1, false);
        }
    }

    fn push_dir_children(&mut self, rows: &mut Vec<Row>, dir: &Path, depth: usize, packages: bool) {
        for e in self.list(dir) {
            if e.is_dir {
                if !packages {
                    self.push_folder(rows, &e.path, depth);
                    continue;
                }
                let (label, target) = self.compact(&e.path);
                let id = NodeId::Dir(target.clone());
                let expanded = self.expanded.contains(&id);
                rows.push(Row { id, depth, label, icon: Icon::Package, expandable: true, expanded });
                if expanded {
                    self.push_dir_children(rows, &target, depth + 1, true);
                }
            } else {
                rows.push(file_row(&e.path, depth));
            }
        }
    }
}

fn file_row(path: &Path, depth: usize) -> Row {
    let icon = match path.extension().and_then(|e| e.to_str()) {
        Some("java") => Icon::Java,
        Some("xml" | "xhtml") => Icon::Xml,
        _ => Icon::File,
    };
    Row { id: NodeId::File(path.to_path_buf()), depth, label: file_name(path), icon, expandable: false, expanded: false }
}

pub fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| p.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use project_model::{Module, SourceRoot};

    fn module(root: &Path, dir: &str, jars: &[&str], in_reactor: bool) -> Module {
        let path = root.join(dir);
        let src = path.join("src/main/java");
        Module {
            path: path.display().to_string(),
            group_id: Some("g".into()),
            artifact_id: Some(dir.into()),
            version: Some("1".into()),
            packaging: "jar".into(),
            content_root: path.display().to_string(),
            source_roots: if src.is_dir() { vec![SourceRoot { path: src.display().to_string(), kind: "main".into(), lang: "java".into() }] } else { vec![] },
            dependencies: vec![],
            profiles: vec![],
            in_reactor,
            classpath_jars: Some(jars.iter().map(|j| j.to_string()).collect()),
        }
    }

    #[test]
    fn project_folder_with_modules_inside_and_one_dependency_list() {
        let root = std::env::temp_dir().join(format!("tree-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let src = root.join("core/src/main/java");
        std::fs::create_dir_all(src.join("com/example/web")).unwrap();
        std::fs::write(src.join("com/example/web/App.java"), "class App {}").unwrap();
        std::fs::create_dir_all(root.join("web/src/main/java")).unwrap();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(root.join("docs/notes.md"), "x").unwrap();
        std::fs::write(root.join("pom.xml"), "<project/>").unwrap();
        std::fs::write(root.join("core/pom.xml"), "<project/>").unwrap();
        // two modules on the same jar (and one on a different version of another)
        let project = Project {
            root: root.display().to_string(),
            modules: vec![
                module(&root, "core", &["/m2/a/a-1.0.jar", "/m2/b/b-1.0.jar", "/m2/target/classes"], true),
                module(&root, "web", &["/m2/a/a-1.0.jar", "/m2/b/b-2.0.jar"], false),
            ],
        };
        let mut t = ProjectTree::new(root.clone());
        let rows = t.rows(Some(&project));
        let labels: Vec<_> = rows.iter().map(|r| (r.depth, r.label.as_str())).collect();
        // the project folder first, its folders and modules in place (folders before files)
        assert_eq!(labels[0], (0, file_name(&root).as_str()));
        assert_eq!(&labels[1..5], &[(1, "core"), (1, "docs"), (1, "web  [independent]"), (1, "pom.xml")]);
        assert!(rows[1].icon == Icon::Module && rows[3].icon == Icon::ModuleIndependent && rows[2].icon == Icon::Folder);
        // one Dependencies node at the end: a.jar once although two modules use it, b in two versions
        let deps = rows.last().unwrap();
        assert_eq!((deps.depth, deps.label.as_str()), (0, "Dependencies (3)"));
        t.expand(NodeId::Deps(root.clone()));
        let jars: Vec<String> = t.rows(Some(&project)).into_iter().filter(|r| matches!(r.id, NodeId::Jar(_))).map(|r| r.label.replace("  (đã xóa)", "")).collect();
        assert_eq!(jars, ["a-1.0.jar", "b-1.0.jar", "b-2.0.jar"]);

        // a module opens with its source roots and packages compacted
        t.toggle(&rows[1].id);
        let rows = t.rows(Some(&project));
        let core: Vec<_> = rows.iter().skip(1).take(3).map(|r| (r.depth, r.label.as_str())).collect();
        assert_eq!(core, [(1, "core"), (2, "src/main/java"), (2, "pom.xml")]);
        let java = rows.iter().find(|r| r.label == "src/main/java" && r.depth == 2).unwrap().id.clone();
        t.toggle(&java);
        let rows = t.rows(Some(&project));
        let pkg = rows.iter().find(|r| r.label == "com.example.web").expect("compacted package");
        assert_eq!(pkg.depth, 3);
        t.toggle(&pkg.id.clone());
        let rows = t.rows(Some(&project));
        assert_eq!(rows.iter().find(|r| r.label == "App.java").unwrap().depth, 4);

        // a plain folder opens like any folder
        let docs = rows.iter().find(|r| r.label == "docs").unwrap().id.clone();
        t.toggle(&docs);
        assert!(t.rows(Some(&project)).iter().any(|r| r.label == "notes.md" && r.depth == 2));

        // dotfiles only with "show hidden"
        std::fs::write(root.join("core/.gitignore"), "target/").unwrap();
        t.refresh();
        assert!(!t.rows(Some(&project)).iter().any(|r| r.label == ".gitignore"));
        t.show_hidden = true;
        t.refresh();
        assert!(t.rows(Some(&project)).iter().any(|r| r.label == ".gitignore"));

        // reveal from a collapsed tree: project folder, module, source root, package
        let mut t = ProjectTree::new(root.clone());
        let app = src.join("com/example/web/App.java");
        t.reveal(Some(&project), &app);
        assert!(t.rows(Some(&project)).iter().any(|r| r.id == NodeId::File(app.clone())));
        let notes = root.join("docs/notes.md");
        t.reveal(Some(&project), &notes);
        assert!(t.rows(Some(&project)).iter().any(|r| r.id == NodeId::File(notes.clone())));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_single_module_project_is_its_own_folder_node() {
        let root = std::env::temp_dir().join(format!("tree-single-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src/main/java")).unwrap();
        std::fs::write(root.join("pom.xml"), "<project/>").unwrap();
        let project = Project { root: root.display().to_string(), modules: vec![module(&root, "", &[], true)] };
        let mut t = ProjectTree::new(root.clone());
        let rows = t.rows(Some(&project));
        // the folder is the module (opened by itself the first time), no extra "root" node
        assert_eq!((rows[0].depth, rows[0].icon, rows[0].expanded), (0, Icon::Module, true));
        assert_eq!(rows[1].label, "src/main/java");
        assert!(!rows.iter().any(|r| r.id == NodeId::RootDir));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn without_a_model_the_folder_is_listed_plainly() {
        let root = std::env::temp_dir().join(format!("tree-plain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("a")).unwrap();
        std::fs::write(root.join("b.txt"), "x").unwrap();
        let mut t = ProjectTree::new(root.clone());
        let rows = t.rows(None);
        let labels: Vec<_> = rows.iter().map(|r| (r.depth, r.label.as_str())).collect();
        assert_eq!(labels, [(0, file_name(&root).as_str()), (1, "a"), (1, "b.txt")]);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
