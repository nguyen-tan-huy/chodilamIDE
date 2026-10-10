//! SmartGit-style operations on top of the basics: staging single hunks (the index written
//! directly, no patch files), the three versions of a conflicted file, the commit graph and
//! what can be done on a commit (checkout, branch, tag, cherry-pick, revert, reset).
use super::{Hunk, git, git_cmd, has_head, rels};
use std::io::Write;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::Stdio;

fn git_stdin(root: &Path, args: &[&str], input: &str) -> Result<String, String> {
    let mut child = git_cmd(root).args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("git: {e}"))?;
    if let Some(mut i) = child.stdin.take() {
        i.write_all(input.as_bytes()).map_err(|e| e.to_string())?;
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() { Ok(String::from_utf8_lossy(&out.stdout).into_owned()) } else { Err(String::from_utf8_lossy(&out.stderr).trim().to_string()) }
}

fn show_spec(root: &Path, spec: &str) -> Option<String> {
    git(root, &["show", spec]).ok()
}

fn rel(root: &Path, path: &Path) -> String {
    rels(root, &[path.to_path_buf()]).remove(0).replace('\\', "/")
}

// ---- index ---------------------------------------------------------------------------------

/// `path` as staged (stage 0 of the index); None when it is not in the index.
pub fn index_text(root: &Path, path: &Path) -> Option<String> {
    show_spec(root, &format!(":{}", rel(root, path)))
}

/// Stage 1 (base), 2 (ours) or 3 (theirs) of a conflicted file.
pub fn stage_text(root: &Path, stage: u8, path: &Path) -> Option<String> {
    show_spec(root, &format!(":{stage}:{}", rel(root, path)))
}

/// Writes `text` as the staged content of `path` (keeps the file mode).
pub fn write_index(root: &Path, path: &Path, text: &str) -> Result<String, String> {
    let r = rel(root, path);
    let mode = git(root, &["ls-files", "-s", "--", &r]).ok().and_then(|o| o.split_whitespace().next().map(str::to_string)).unwrap_or_else(|| "100644".into());
    let sha = git_stdin(root, &["hash-object", "-w", "--stdin"], text)?.trim().to_string();
    git(root, &["update-index", "--add", "--cacheinfo", &format!("{mode},{sha},{r}")])
}

/// The lines of `s` with their "\n" (so a missing last newline survives).
fn pieces(s: &str) -> Vec<&str> {
    s.split_inclusive('\n').collect()
}

/// `base` with hunk `h` (of `diff_lines(base, target)`) taken from `target`.
pub fn apply_hunk(base: &str, target: &str, h: &Hunk) -> String {
    let (a, b) = (pieces(base), pieces(target));
    let parts: Vec<&str> = a[..h.old.start.min(a.len())].iter().chain(&b[h.new.start.min(b.len())..h.new.end.min(b.len())]).chain(&a[h.old.end.min(a.len())..]).copied().collect();
    let mut out = String::new();
    for (i, p) in parts.iter().enumerate() {
        out.push_str(p);
        if i + 1 < parts.len() && !p.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

/// The hunk seen from the other side (undo it in the new text).
pub fn reverse(h: &Hunk) -> Hunk {
    Hunk { old: h.new.clone(), new: h.old.clone() }
}

/// Stages one hunk of the work tree (`hunk` of `diff_lines(index, worktree)`).
pub fn stage_hunk(root: &Path, path: &Path, worktree: &str, hunk: &Hunk) -> Result<String, String> {
    let index = index_text(root, path).unwrap_or_default();
    write_index(root, path, &apply_hunk(&index, worktree, hunk))
}

/// Unstages one hunk (`hunk` of `diff_lines(HEAD, index)`): that part of the index back to HEAD.
pub fn unstage_hunk(root: &Path, path: &Path, hunk: &Hunk) -> Result<String, String> {
    let head = super::head_text(root, path);
    let index = index_text(root, path).unwrap_or_default();
    let new = apply_hunk(&index, head.as_deref().unwrap_or(""), &reverse(hunk));
    if head.is_none() && new.is_empty() {
        // a new file with nothing staged left: out of the index again
        return git(root, &["rm", "-q", "--cached", "--", &rel(root, path)]);
    }
    write_index(root, path, &new)
}

/// Character ranges that differ inside a changed line pair (the middle after the common
/// prefix and suffix), as byte ranges of `a` and `b`.
pub fn inline_diff(a: &str, b: &str) -> (Range<usize>, Range<usize>) {
    let pre: usize = a.chars().zip(b.chars()).take_while(|(x, y)| x == y).map(|(x, _)| x.len_utf8()).sum();
    let (ra, rb) = (&a[pre..], &b[pre..]);
    let suf: usize = ra.chars().rev().zip(rb.chars().rev()).take_while(|(x, y)| x == y).map(|(x, _)| x.len_utf8()).sum();
    (pre..a.len() - suf, pre..b.len() - suf)
}

// ---- conflicts -------------------------------------------------------------------------------

/// The file re-merged from the index stages with diff3 markers (every conflict with its base),
/// to show the base even when merge.conflictStyle is "merge". None without the stages.
pub fn diff3_text(root: &Path, path: &Path) -> Option<String> {
    let ours = stage_text(root, 2, path)?;
    let theirs = stage_text(root, 3, path)?;
    let base = stage_text(root, 1, path).unwrap_or_default();
    let dir = std::env::temp_dir().join(format!("chodilamide-merge-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    let (o, b, t) = (dir.join("ours"), dir.join("base"), dir.join("theirs"));
    std::fs::write(&o, ours).ok()?;
    std::fs::write(&b, base).ok()?;
    std::fs::write(&t, theirs).ok()?;
    // exit code = number of conflicts, not an error
    let out = git_cmd(root).args(["merge-file", "-p", "--diff3", "-L", "ours", "-L", "base", "-L", "theirs"]).arg(&o).arg(&b).arg(&t).output().ok()?;
    let _ = std::fs::remove_dir_all(&dir);
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

// ---- log graph -------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct GraphCommit {
    pub hash: String,
    pub short: String,
    pub parents: Vec<String>,
    /// "HEAD -> main", "origin/main", "tag: v1"
    pub refs: Vec<String>,
    pub author: String,
    pub date: String,
    pub subject: String,
}

/// One segment of the graph inside a row: lane columns, y 0 = top, 1 = middle (the dot), 2 = bottom.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Edge {
    pub from: (usize, u8),
    pub to: (usize, u8),
    /// lane color index
    pub lane: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GraphRow {
    pub col: usize,
    pub lanes: usize,
    pub edges: Vec<Edge>,
}

/// `git log` in topological order with parents and refs; `all`: every branch and tag.
/// `all`: every ref; else the `branches` given plus HEAD (none = HEAD only).
pub fn graph_log(root: &Path, n: usize, all: bool, branches: &[String], path: Option<&Path>) -> Vec<GraphCommit> {
    let mut args = vec!["log".to_string(), "--topo-order".into(), format!("-n{n}"), "--date=format:%Y-%m-%d %H:%M".into(), "--format=%H%x1f%h%x1f%P%x1f%D%x1f%an%x1f%ad%x1f%s".into()];
    if all {
        args.push("--all".into());
    } else {
        // "--end-of-options" keeps a branch named like an option from becoming one
        args.push("--end-of-options".into());
        args.extend(branches.iter().cloned());
        // branches ticked in the Git window are shown together with the current one
        if !branches.is_empty() && !branches.iter().any(|b| b == "HEAD") {
            args.push("HEAD".into());
        }
    }
    if let Some(p) = path {
        // parents rewritten to the commits that touched the file: the graph stays connected
        args.extend(["--parents".into(), "--".into(), p.to_string_lossy().to_string()]);
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    parse_graph_log(&git(root, &refs).unwrap_or_default())
}

pub fn parse_graph_log(out: &str) -> Vec<GraphCommit> {
    out.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\u{1f}').collect();
            (f.len() == 7).then(|| GraphCommit {
                hash: f[0].into(),
                short: f[1].into(),
                parents: f[2].split_whitespace().map(str::to_string).collect(),
                refs: f[3].split(", ").filter(|r| !r.is_empty()).map(str::to_string).collect(),
                author: f[4].into(),
                date: f[5].into(),
                subject: f[6].into(),
            })
        })
        .collect()
}

/// Lanes for the graph column: every commit gets the lane that waits for it (or a free one),
/// its first parent continues that lane, other parents get / join lanes.
pub fn layout_graph(commits: &[GraphCommit]) -> Vec<GraphRow> {
    let mut lanes: Vec<Option<String>> = Vec::new();
    let mut colors: Vec<usize> = Vec::new();
    let mut next_color = 0;
    let mut rows = Vec::with_capacity(commits.len());
    let free_slot = |lanes: &mut Vec<Option<String>>, colors: &mut Vec<usize>, skip: Option<usize>, next_color: &mut usize| -> usize {
        let i = lanes.iter().enumerate().position(|(i, l)| l.is_none() && Some(i) != skip).unwrap_or_else(|| {
            lanes.push(None);
            colors.push(0);
            lanes.len() - 1
        });
        colors[i] = *next_color;
        *next_color += 1;
        i
    };
    for c in commits {
        let mut edges = Vec::new();
        let waiting: Vec<usize> = lanes.iter().enumerate().filter(|(_, l)| l.as_deref() == Some(c.hash.as_str())).map(|(i, _)| i).collect();
        let col = match waiting.first() {
            Some(&i) => i,
            None => free_slot(&mut lanes, &mut colors, None, &mut next_color),
        };
        let before = lanes.len();
        for (j, l) in lanes.iter().enumerate() {
            match l {
                Some(h) if *h == c.hash => edges.push(Edge { from: (j, 0), to: (col, 1), lane: colors[j] }),
                Some(_) => edges.push(Edge { from: (j, 0), to: (j, 2), lane: colors[j] }),
                None => {}
            }
        }
        for &j in &waiting {
            lanes[j] = None;
        }
        for (k, p) in c.parents.iter().enumerate() {
            if k == 0 {
                // the first parent keeps this lane (lanes waiting for the same commit meet there)
                lanes[col] = Some(p.clone());
                edges.push(Edge { from: (col, 1), to: (col, 2), lane: colors[col] });
                continue;
            }
            let j = match lanes.iter().position(|l| l.as_deref() == Some(p.as_str())) {
                Some(j) => j,
                None => {
                    let j = free_slot(&mut lanes, &mut colors, Some(col), &mut next_color);
                    lanes[j] = Some(p.clone());
                    j
                }
            };
            edges.push(Edge { from: (col, 1), to: (j, 2), lane: colors[j] });
        }
        while lanes.last().is_some_and(|l| l.is_none()) {
            lanes.pop();
            colors.pop();
        }
        rows.push(GraphRow { col, lanes: before.max(lanes.len()).max(col + 1), edges });
    }
    rows
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommitFile {
    /// M A D R C T
    pub status: char,
    pub path: PathBuf,
    pub orig: Option<PathBuf>,
}

/// Files changed by `hash` (against its first parent; the root commit: everything).
pub fn commit_files(root: &Path, hash: &str, parent: Option<&str>) -> Vec<CommitFile> {
    let out = match parent {
        Some(p) => git(root, &["diff", "--name-status", "-z", "-M", p, hash]),
        None => git(root, &["diff-tree", "--root", "--no-commit-id", "-r", "-z", "-M", "--name-status", hash]),
    };
    parse_name_status(root, &out.unwrap_or_default())
}

pub fn parse_name_status(root: &Path, out: &str) -> Vec<CommitFile> {
    let mut v = Vec::new();
    let mut it = out.split('\0').filter(|s| !s.is_empty());
    while let Some(st) = it.next() {
        let status = st.chars().next().unwrap_or('M');
        let Some(first) = it.next() else { break };
        if status == 'R' || status == 'C' {
            let Some(second) = it.next() else { break };
            v.push(CommitFile { status, path: root.join(second), orig: Some(root.join(first)) });
        } else {
            v.push(CommitFile { status, path: root.join(first), orig: None });
        }
    }
    v
}

/// Full message, author and committer of a commit (the details pane).
pub fn commit_details(root: &Path, hash: &str) -> String {
    git(root, &["show", "-s", "--format=%H%nTác giả: %an <%ae>  %ad%nCommitter: %cn  %cd%n%n%B", "--date=format:%Y-%m-%d %H:%M", hash]).unwrap_or_default()
}

// ---- operations on a commit / branch ---------------------------------------------------------

pub fn checkout_detached(root: &Path, hash: &str) -> Result<String, String> {
    git(root, &["checkout", "--detach", hash])
}

pub fn create_branch_at(root: &Path, name: &str, at: &str, switch: bool) -> Result<String, String> {
    if switch { git(root, &["checkout", "-b", name, at]) } else { git(root, &["branch", name, at]) }
}

pub fn create_tag(root: &Path, name: &str, at: &str) -> Result<String, String> {
    git(root, &["tag", name, at])
}

pub fn delete_branch(root: &Path, name: &str, force: bool) -> Result<String, String> {
    git(root, &["branch", if force { "-D" } else { "-d" }, name])
}

pub fn rename_branch(root: &Path, old: &str, new: &str) -> Result<String, String> {
    git(root, &["branch", "-m", old, new])
}

pub fn cherry_pick(root: &Path, hash: &str) -> Result<String, String> {
    git(root, &["cherry-pick", hash])
}

pub fn revert_commit(root: &Path, hash: &str) -> Result<String, String> {
    git(root, &["revert", "--no-edit", hash])
}

/// "soft", "mixed" or "hard".
pub fn reset(root: &Path, mode: &str, hash: &str) -> Result<String, String> {
    git(root, &["reset", &format!("--{mode}"), hash])
}

pub fn head_hash(root: &Path) -> Option<String> {
    has_head(root).then(|| git(root, &["rev-parse", "HEAD"]).ok().map(|s| s.trim().to_string())).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gc(h: &str, p: &[&str]) -> GraphCommit {
        GraphCommit { hash: h.into(), short: h.into(), parents: p.iter().map(|s| s.to_string()).collect(), refs: vec![], author: String::new(), date: String::new(), subject: String::new() }
    }

    #[test]
    fn graph_lanes() {
        // m merges b into a:  m(a,b) / b(r) / a(r) / r
        let rows = layout_graph(&[gc("m", &["a", "b"]), gc("b", &["r"]), gc("a", &["r"]), gc("r", &[])]);
        assert_eq!(rows.iter().map(|r| r.col).collect::<Vec<_>>(), [0, 1, 0, 0]);
        assert!(rows[0].edges.contains(&Edge { from: (0, 1), to: (1, 2), lane: 1 }));
        // b continues lane 1 to r, which waits in lane 0 (a) after a: b's lane joins
        assert!(rows[1].edges.iter().any(|e| e.from == (0, 0) && e.to == (0, 2)));
        assert_eq!(rows[3].lanes, 2);
        assert_eq!(rows[3].edges.iter().filter(|e| e.to == (0, 1)).count(), 2, "both lanes end in r");
    }

    #[test]
    fn hunks_apply_and_inline() {
        let base = "a\nb\nc\nd\n";
        let target = "a\nB\nc\nd\ne";
        let h = super::super::diff_lines(base, target);
        assert_eq!(apply_hunk(base, target, &h[0]), "a\nB\nc\nd\n");
        assert_eq!(apply_hunk(base, target, &h[1]), "a\nb\nc\nd\ne");
        // undo the first hunk inside the target
        assert_eq!(apply_hunk(target, base, &reverse(&h[0])), "a\nb\nc\nd\ne");
        assert_eq!(inline_diff("int x = 1;", "int x = 42;"), (8..9, 8..10));
        assert_eq!(inline_diff("Tiếng Việt", "Tiếng Viet"), (10..13, 10..11));
    }

    #[test]
    fn name_status() {
        let f = parse_name_status(Path::new("/r"), "M\0a.txt\0R100\0old.txt\0new.txt\0D\0x\0");
        assert_eq!(f.len(), 3);
        assert_eq!(f[1], CommitFile { status: 'R', path: PathBuf::from("/r/new.txt"), orig: Some(PathBuf::from("/r/old.txt")) });
    }

    /// The bugs of the review, against a real repository (skipped without git).
    #[test]
    fn real_repo_fixes() {
        use crate::*;
        let dir = std::env::temp_dir().join(format!("vcs-work-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        if git(&dir, &["init", "-q", "-b", "main"]).is_err() {
            return;
        }
        let _ = git(&dir, &["config", "user.email", "t@t"]);
        let _ = git(&dir, &["config", "user.name", "T"]);
        let root = repo_root(&dir).unwrap();
        let f = root.join("A.txt");
        std::fs::write(&f, "1\n2\n3\n4\n5\n6\n7\n8\n").unwrap();
        commit(&root, &[f.clone()], "init", false).unwrap();
        assert_eq!(commit(&root, &[], "nothing", false).unwrap_err(), "Chưa stage file nào để commit");

        // a rename commits the deletion of the old name too
        git(&root, &["mv", "A.txt", "B.txt"]).unwrap();
        let b = root.join("B.txt");
        let ch = changes(&root);
        assert_eq!(ch[0].orig.as_deref(), Some(f.as_path()));
        commit(&root, &[], "rename", false).unwrap();
        assert!(changes(&root).is_empty(), "nothing left staged: {:?}", changes(&root));

        // stage one hunk of two, commit: the other stays unstaged
        std::fs::write(&b, "ONE\n2\n3\n4\n5\n6\n7\nEIGHT\n").unwrap();
        let wt = std::fs::read_to_string(&b).unwrap();
        let hunks = diff_lines(&index_text(&root, &b).unwrap(), &wt);
        assert_eq!(hunks.len(), 2);
        stage_hunk(&root, &b, &wt, &hunks[0]).unwrap();
        assert_eq!(index_text(&root, &b).unwrap(), "ONE\n2\n3\n4\n5\n6\n7\n8\n");
        // unstage it again, stage again, commit
        let staged = diff_lines(&head_text(&root, &b).unwrap(), &index_text(&root, &b).unwrap());
        unstage_hunk(&root, &b, &staged[0]).unwrap();
        assert_eq!(index_text(&root, &b), head_text(&root, &b));
        stage_hunk(&root, &b, &wt, &hunks[0]).unwrap();
        commit(&root, &[], "first hunk", false).unwrap();
        assert_eq!(head_text(&root, &b).unwrap(), "ONE\n2\n3\n4\n5\n6\n7\n8\n");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), wt, "work tree untouched");
        // amend with an empty message keeps it
        commit(&root, &[b.clone()], "", true).unwrap();
        assert_eq!(last_message(&root), "first hunk");

        // commit after resolving a merge (was: "cannot do a partial commit during a merge")
        let g = root.join("G.txt");
        std::fs::write(&g, "g\n").unwrap();
        commit(&root, &[g.clone()], "add g", false).unwrap();
        git(&root, &["checkout", "-q", "-b", "feat"]).unwrap();
        std::fs::write(&b, "feat\n").unwrap();
        std::fs::write(&g, "feat g\n").unwrap();
        commit(&root, &[b.clone(), g.clone()], "feat", false).unwrap();
        git(&root, &["checkout", "-q", "main"]).unwrap();
        std::fs::write(&b, "main\n").unwrap();
        commit(&root, &[b.clone()], "main", false).unwrap();
        git(&root, &["rm", "-q", "G.txt"]).unwrap();
        commit(&root, &[], "drop g", false).unwrap();
        assert!(merge(&root, "feat").is_err());
        // G.txt: deleted by us, modified by them → take ours = delete
        let cg = changes(&root).into_iter().find(|c| c.path == g).unwrap();
        assert!(cg.conflicted());
        take_side(&root, &cg, true).unwrap();
        let d3 = diff3_text(&root, &b).unwrap();
        assert!(d3.contains("|||||||"), "{d3}");
        mark_resolved(&root, &b).unwrap();
        commit(&root, &[], "merge feat", false).unwrap();
        assert_eq!(repo_state(&root), RepoState::Clean);
        assert!(!g.exists());

        // graph: the merge has two parents, its row draws to two lanes
        let log = graph_log(&root, 50, true, &[], None);
        assert_eq!(log[0].parents.len(), 2);
        assert!(log[0].refs.iter().any(|r| r == "HEAD -> main"));
        let rows = layout_graph(&log);
        assert!(rows[0].edges.iter().filter(|e| e.from.1 == 1).count() == 2);
        let files = commit_files(&root, &log[0].hash, Some(&log[0].parents[0]));
        assert_eq!(files.len(), 1, "{files:?}");

        // checkout of a remote branch never resets the local one
        let clone = dir.with_extension("clone");
        let _ = std::fs::remove_dir_all(&clone);
        git(&dir, &["clone", "-q", &dir.to_string_lossy(), &clone.to_string_lossy()]).unwrap();
        let _ = git(&clone, &["config", "user.email", "t@t"]);
        let _ = git(&clone, &["config", "user.name", "T"]);
        checkout(&clone, "origin/feat", false).unwrap();
        assert_eq!(branch(&clone).as_deref(), Some("feat"));
        std::fs::write(clone.join("L.txt"), "local\n").unwrap();
        commit(&clone, &[clone.join("L.txt")], "local only", false).unwrap();
        checkout(&clone, "main", false).unwrap();
        checkout(&clone, "origin/feat", false).unwrap();
        assert_eq!(last_message(&clone), "local only", "local commit kept");
        std::fs::remove_dir_all(&clone).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
