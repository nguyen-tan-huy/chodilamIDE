//! Git through the `git` command line (like IntelliJ's Git4Idea): status for the tree, line
//! changes against HEAD for the gutter (own Myers diff, so unsaved text is compared too),
//! branches, log, show, blame, commit, push, rollback.
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::Command;

mod work;
pub use work::*;

/// Every git call: never waits for a password on a terminal nobody sees (fails instead, and
/// the IDE asks for an account), and also reads the accounts saved by the IDE (store helper,
/// added to whatever helper the user configured).
fn git_cmd(root: &Path) -> Command {
    let mut c = Command::new("git");
    c.arg("-C").arg(root).args(["-c", "credential.helper=store"]).env("GIT_TERMINAL_PROMPT", "0").env("GCM_INTERACTIVE", "never").stdin(std::process::Stdio::null());
    // merge/rebase --continue would open an editor for the message: keep git's default message
    c.env("GIT_EDITOR", "true");
    c
}

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let out = git_cmd(root).args(args).output().map_err(|e| format!("git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if err.is_empty() { format!("git {} thất bại", args.join(" ")) } else { err })
    }
}

/// Top of the work tree containing `dir`, None outside git.
pub fn repo_root(dir: &Path) -> Option<PathBuf> {
    git(dir, &["rev-parse", "--show-toplevel"]).ok().map(|s| PathBuf::from(s.trim()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Modified,
    Added,
    Untracked,
    Deleted,
    Renamed,
    Conflict,
}

impl FileStatus {
    pub fn letter(self) -> &'static str {
        match self {
            FileStatus::Modified => "M",
            FileStatus::Added => "A",
            FileStatus::Untracked => "U",
            FileStatus::Deleted => "D",
            FileStatus::Renamed => "R",
            FileStatus::Conflict => "!",
        }
    }
}

/// `git status --porcelain -z` → absolute path → status.
pub fn parse_status(root: &Path, out: &str) -> HashMap<PathBuf, FileStatus> {
    let mut map = HashMap::new();
    let mut parts = out.split('\0').filter(|s| !s.is_empty());
    while let Some(entry) = parts.next() {
        if entry.len() < 4 {
            continue;
        }
        let (x, y) = (entry.as_bytes()[0], entry.as_bytes()[1]);
        let path = &entry[3..];
        let st = match (x, y) {
            (b'?', b'?') => FileStatus::Untracked,
            (b'U', _) | (_, b'U') | (b'A', b'A') | (b'D', b'D') => FileStatus::Conflict,
            (b'R', _) | (b'C', _) => {
                parts.next(); // the old name follows
                FileStatus::Renamed
            }
            (b'A', _) => FileStatus::Added,
            (b'D', _) | (_, b'D') => FileStatus::Deleted,
            _ => FileStatus::Modified,
        };
        map.insert(root.join(path), st);
    }
    map
}

pub fn status(root: &Path) -> HashMap<PathBuf, FileStatus> {
    status_map(&changes(root))
}

/// Tree colors from the change list (no second `git status`).
pub fn status_map(changes: &[Change]) -> HashMap<PathBuf, FileStatus> {
    changes.iter().map(|c| (c.path.clone(), xy_status(c.index as u8, c.worktree as u8))).collect()
}

fn xy_status(x: u8, y: u8) -> FileStatus {
    match (x, y) {
        (b'?', b'?') => FileStatus::Untracked,
        (b'U', _) | (_, b'U') | (b'A', b'A') | (b'D', b'D') => FileStatus::Conflict,
        (b'R', _) | (b'C', _) => FileStatus::Renamed,
        (b'A', _) => FileStatus::Added,
        (b'D', _) | (_, b'D') => FileStatus::Deleted,
        _ => FileStatus::Modified,
    }
}

pub fn branch(root: &Path) -> Option<String> {
    let b = git(root, &["rev-parse", "--abbrev-ref", "HEAD"]).ok()?.trim().to_string();
    if b == "HEAD" {
        // detached: show the commit
        return git(root, &["rev-parse", "--short", "HEAD"]).ok().map(|h| format!("({})", h.trim()));
    }
    Some(b)
}

/// Local branches first (current marked), then remote ones.
pub fn branches(root: &Path) -> Vec<(String, bool)> {
    let out = git(root, &["branch", "--all", "--format=%(HEAD)%(refname:short)"]).unwrap_or_default();
    out.lines()
        .filter(|l| !l.contains("HEAD ->") && !l.ends_with("/HEAD"))
        .map(|l| (l[1..].trim().to_string(), l.starts_with('*')))
        .filter(|(b, _)| !b.is_empty())
        .collect()
}

/// A local branch with its upstream and how far it is from it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BranchInfo {
    pub name: String,
    pub current: bool,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    /// the upstream branch no longer exists on the remote
    pub gone: bool,
}

/// Everything the Git window's branch list shows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RefList {
    pub locals: Vec<BranchInfo>,
    /// (remote, its branches without the "remote/" prefix)
    pub remotes: Vec<(String, Vec<String>)>,
    pub tags: Vec<String>,
}

/// Local branches (with ahead/behind), remote branches by remote, and tags, in one git call.
pub fn ref_list(root: &Path) -> RefList {
    let out = git(root, &["for-each-ref", "--format=%(refname)\t%(HEAD)\t%(upstream:short)\t%(upstream:track,nobracket)", "refs/heads", "refs/remotes", "refs/tags"]).unwrap_or_default();
    parse_ref_list(&out)
}

pub fn parse_ref_list(out: &str) -> RefList {
    let mut list = RefList::default();
    for line in out.lines() {
        let mut f = line.split('\t');
        let (Some(name), head, upstream, track) = (f.next(), f.next().unwrap_or(""), f.next().unwrap_or(""), f.next().unwrap_or("")) else { continue };
        if let Some(b) = name.strip_prefix("refs/heads/") {
            let (mut ahead, mut behind) = (0, 0);
            for part in track.split(',') {
                let part = part.trim();
                if let Some(n) = part.strip_prefix("ahead ") {
                    ahead = n.trim().parse().unwrap_or(0);
                } else if let Some(n) = part.strip_prefix("behind ") {
                    behind = n.trim().parse().unwrap_or(0);
                }
            }
            list.locals.push(BranchInfo { name: b.to_string(), current: head.trim() == "*", upstream: (!upstream.is_empty()).then(|| upstream.to_string()), ahead, behind, gone: track.trim() == "gone" });
        } else if let Some(r) = name.strip_prefix("refs/remotes/") {
            if r.ends_with("/HEAD") {
                continue;
            }
            if let Some((remote, branch)) = r.split_once('/') {
                match list.remotes.iter_mut().find(|(n, _)| n == remote) {
                    Some((_, v)) => v.push(branch.to_string()),
                    None => list.remotes.push((remote.to_string(), vec![branch.to_string()])),
                }
            }
        } else if let Some(t) = name.strip_prefix("refs/tags/") {
            list.tags.push(t.to_string());
        }
    }
    list.tags.reverse();
    list
}

pub fn checkout(root: &Path, branch: &str, create: bool) -> Result<String, String> {
    if create {
        return git(root, &["checkout", "-b", branch]);
    }
    if local_branch_exists(root, branch) {
        return git(root, &["checkout", branch]);
    }
    // a remote branch: its local branch if there is one (never reset it: it may hold
    // unpushed commits), otherwise a new local branch tracking it
    for r in remote_names(root) {
        if let Some(local) = branch.strip_prefix(&format!("{r}/")) {
            return if local_branch_exists(root, local) { git(root, &["checkout", local]) } else { git(root, &["checkout", "-b", local, "--track", branch]) };
        }
    }
    git(root, &["checkout", branch])
}

fn local_branch_exists(root: &Path, name: &str) -> bool {
    git(root, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{name}")]).is_ok()
}

pub fn remote_names(root: &Path) -> Vec<String> {
    git(root, &["remote"]).unwrap_or_default().lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()
}

/// "origin" when it exists, else the first remote.
fn default_remote(root: &Path) -> String {
    let names = remote_names(root);
    if names.iter().any(|n| n == "origin") { "origin".into() } else { names.into_iter().next().unwrap_or_else(|| "origin".into()) }
}

/// Text of `path` at HEAD (None: new file / not in git).
pub fn head_text(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    git(root, &["show", &format!("HEAD:{}", rel.to_string_lossy().replace('\\', "/"))]).ok()
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hunk {
    /// lines of the old text replaced (empty = pure insertion before old.start)
    pub old: Range<usize>,
    /// lines of the new text (empty = pure deletion, shown before new.start)
    pub new: Range<usize>,
}

/// Line diff (Myers, O((N+M)·D)); common prefix/suffix are trimmed first.
pub fn diff_lines(old: &str, new: &str) -> Vec<Hunk> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let mut pre = 0;
    while pre < a.len() && pre < b.len() && a[pre] == b[pre] {
        pre += 1;
    }
    let mut suf = 0;
    while suf < a.len() - pre && suf < b.len() - pre && a[a.len() - 1 - suf] == b[b.len() - 1 - suf] {
        suf += 1;
    }
    let (a2, b2) = (&a[pre..a.len() - suf], &b[pre..b.len() - suf]);
    let (n, m) = (a2.len() as isize, b2.len() as isize);
    if n == 0 && m == 0 {
        return Vec::new();
    }
    // Myers: trace of V arrays, then walk back to recover the edit script
    let max = (n + m) as usize;
    let off = max as isize;
    let mut v = vec![0isize; 2 * max + 2];
    let mut trace: Vec<Vec<isize>> = Vec::new();
    'outer: for d in 0..=max as isize {
        trace.push(v.clone());
        let mut k = -d;
        while k <= d {
            let idx = (k + off) as usize;
            let mut x = if k == -d || (k != d && v[idx - 1] < v[idx + 1]) { v[idx + 1] } else { v[idx - 1] + 1 };
            let mut y = x - k;
            while x < n && y < m && a2[x as usize] == b2[y as usize] {
                x += 1;
                y += 1;
            }
            v[idx] = x;
            if x >= n && y >= m {
                trace.push(v.clone());
                break 'outer;
            }
            k += 2;
        }
    }
    // walk back: collect (x,y) steps, mark deleted old lines / inserted new lines
    let mut del = vec![false; n as usize];
    let mut ins = vec![false; m as usize];
    let (mut x, mut y) = (n, m);
    for d in (1..trace.len() as isize - 1).rev() {
        let v = &trace[d as usize];
        let k = x - y;
        let idx = (k + off) as usize;
        let prev_k = if k == -d || (k != d && v[idx - 1] < v[idx + 1]) { k + 1 } else { k - 1 };
        let prev_x = v[(prev_k + off) as usize];
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            x -= 1;
            y -= 1;
        }
        if x == prev_x {
            ins[(y - 1) as usize] = true;
        } else {
            del[(x - 1) as usize] = true;
        }
        x = prev_x;
        y = prev_y;
    }
    // group runs into hunks
    let mut hunks = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < del.len() || j < ins.len() {
        if i < del.len() && j < ins.len() && !del[i] && !ins[j] {
            i += 1;
            j += 1;
            continue;
        }
        let (si, sj) = (i, j);
        while i < del.len() && del[i] {
            i += 1;
        }
        while j < ins.len() && ins[j] {
            j += 1;
        }
        if si == i && sj == j {
            // only one side left (tail): consume it
            if i < del.len() {
                i += 1;
            } else {
                j += 1;
            }
            continue;
        }
        hunks.push(Hunk { old: pre + si..pre + i, new: pre + sj..pre + j });
    }
    hunks
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LineMark {
    Added,
    Modified,
    /// lines were removed just above this line
    Deleted,
}

/// Gutter marks per line of the new text.
pub fn line_marks(hunks: &[Hunk]) -> Vec<(usize, LineMark)> {
    let mut out = Vec::new();
    for h in hunks {
        if h.new.is_empty() {
            out.push((h.new.start, LineMark::Deleted));
        } else {
            let mark = if h.old.is_empty() { LineMark::Added } else { LineMark::Modified };
            out.extend(h.new.clone().map(|l| (l, mark)));
        }
    }
    out
}

/// The hunk touching `line` of the new text, reverted: (new line range to replace, old lines).
pub fn revert_hunk(old: &str, hunks: &[Hunk], line: usize) -> Option<(Range<usize>, String)> {
    let h = hunks.iter().find(|h| h.new.contains(&line) || (h.new.is_empty() && h.new.start == line))?;
    let old_lines: Vec<&str> = old.lines().collect();
    let text = old_lines[h.old.clone()].iter().map(|l| format!("{l}\n")).collect::<String>();
    Some((h.new.clone(), text))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Commit {
    pub hash: String,
    pub short: String,
    pub author: String,
    pub date: String,
    pub subject: String,
}

pub fn log(root: &Path, path: Option<&Path>, n: usize) -> Vec<Commit> {
    let mut args = vec!["log".to_string(), format!("-n{n}"), "--date=short".into(), "--format=%H%x1f%h%x1f%an%x1f%ad%x1f%s".into()];
    if let Some(p) = path {
        args.push("--follow".into());
        args.push("--".into());
        args.push(p.to_string_lossy().to_string());
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    git(root, &refs)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\u{1f}').collect();
            (f.len() == 5).then(|| Commit { hash: f[0].into(), short: f[1].into(), author: f[2].into(), date: f[3].into(), subject: f[4].into() })
        })
        .collect()
}

/// `git show` of a commit (stat + patch), for a read-only tab.
pub fn show(root: &Path, hash: &str) -> String {
    git(root, &["show", "--stat", "--patch", "--format=commit %H%nAuthor: %an <%ae>%nDate:   %ad%n%n    %s%n%n%b", hash]).unwrap_or_else(|e| e)
}

/// Working-tree diff of one file against HEAD (or of everything with None).
pub fn diff_text(root: &Path, path: Option<&Path>) -> String {
    let mut args = vec!["diff", "HEAD", "--no-color"];
    let p;
    if let Some(path) = path {
        p = path.to_string_lossy().to_string();
        args.push("--");
        args.push(&p);
    }
    git(root, &args).unwrap_or_else(|e| e)
}

#[derive(Debug, Clone, PartialEq)]
pub struct BlameLine {
    pub short: String,
    pub author: String,
    pub date: String,
    pub summary: String,
}

/// `git blame --line-porcelain`: one entry per line of the file as on disk.
pub fn blame(root: &Path, path: &Path) -> Vec<BlameLine> {
    let Ok(out) = git(root, &["blame", "--line-porcelain", "--", &path.to_string_lossy()]) else { return Vec::new() };
    parse_blame(&out)
}

pub fn parse_blame(out: &str) -> Vec<BlameLine> {
    let mut lines = Vec::new();
    let mut cur = BlameLine { short: String::new(), author: String::new(), date: String::new(), summary: String::new() };
    for l in out.lines() {
        if l.starts_with('\t') {
            lines.push(cur.clone());
        } else if let Some(a) = l.strip_prefix("author ") {
            cur.author = a.to_string();
        } else if let Some(t) = l.strip_prefix("author-time ") {
            cur.date = t.parse::<i64>().map(ymd).unwrap_or_default();
        } else if let Some(s) = l.strip_prefix("summary ") {
            cur.summary = s.to_string();
        } else if l.len() >= 40 && l.as_bytes()[..40].iter().all(|c| c.is_ascii_hexdigit()) {
            cur.short = if l.starts_with("0000000") { "chưa commit".into() } else { l[..8].to_string() };
        }
    }
    lines
}

/// Unix seconds → YYYY-MM-DD (UTC), no date library needed.
fn ymd(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    // civil-from-days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{:02}-{:02}", if m <= 2 { y + 1 } else { y }, m, d)
}

/// Commits the index (what is staged), like SmartGit / `git commit`: `stage_first` are staged
/// before (empty: only what is already staged). No pathspec, so staged hunks stay partial,
/// renames keep their deleted source and a merge can be concluded. An empty message with
/// `amend` keeps the previous one.
pub fn commit(root: &Path, stage_first: &[PathBuf], message: &str, amend: bool) -> Result<String, String> {
    if message.trim().is_empty() && !amend {
        return Err("Chưa nhập commit message".into());
    }
    if !stage_first.is_empty() {
        stage(root, stage_first)?;
    }
    let merging = repo_state(root) == RepoState::Merging;
    if !amend && !merging && git(root, &["diff", "--cached", "--quiet"]).is_ok() && has_head(root) {
        return Err("Chưa stage file nào để commit".into());
    }
    let mut args = vec!["commit"];
    if amend {
        args.push("--amend");
    }
    if message.trim().is_empty() {
        args.push("--no-edit");
    } else {
        args.extend(["-m", message]);
    }
    git(root, &args)
}

fn has_head(root: &Path) -> bool {
    git(root, &["rev-parse", "--verify", "--quiet", "HEAD"]).is_ok()
}

/// Message of the last commit (amend fills it in).
pub fn last_message(root: &Path) -> String {
    git(root, &["log", "-1", "--format=%B"]).map(|s| s.trim_end().to_string()).unwrap_or_default()
}

/// Where a repository URL points: for choosing the account (https host) or the SSH key.
#[derive(Debug, Clone, PartialEq)]
pub struct RepoUrl {
    pub host: String,
    pub https: bool,
    pub ssh: bool,
    /// the folder `git clone` would create ("repo" of ".../repo.git")
    pub name: String,
}

/// https://host/g/repo.git, http://…, ssh://git@host/g/repo.git, git@host:g/repo.git, or a local
/// path / file:// (host empty, neither https nor ssh).
pub fn parse_repo_url(url: &str) -> Option<RepoUrl> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    let name_of = |path: &str| path.trim_end_matches('/').rsplit(['/', ':']).next().unwrap_or("").trim_end_matches(".git").to_string();
    let strip_userinfo = |rest: &str| {
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        let host = authority.rsplit('@').next().unwrap_or(authority).split(':').next().unwrap_or("").to_string();
        (host, path.to_string())
    };
    let (host, https, ssh, path) = if let Some(rest) = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")) {
        let (h, p) = strip_userinfo(rest);
        (h, true, false, p)
    } else if let Some(rest) = url.strip_prefix("ssh://") {
        let (h, p) = strip_userinfo(rest);
        (h, false, true, p)
    } else if let Some(rest) = url.strip_prefix("file://") {
        (String::new(), false, false, rest.to_string())
    } else if let Some((left, path)) = url.split_once(':').filter(|(l, _)| l.contains('@') && !l.contains('/')) {
        // scp style: git@host:group/repo.git
        (left.rsplit('@').next().unwrap_or(left).to_string(), false, true, path.to_string())
    } else {
        (String::new(), false, false, url.to_string())
    };
    let name = name_of(&path);
    if name.is_empty() {
        return None;
    }
    Some(RepoUrl { host, https, ssh, name })
}

/// `git clone` into `dest`, reporting git's progress lines ("Receiving objects:  42%…") to
/// `progress`. Never prompts (a missing login fails instead of hanging) and trusts a new SSH host
/// on first contact only (`accept-new`).
pub fn clone_repo(url: &str, dest: &Path, mut progress: impl FnMut(&str)) -> Result<(), String> {
    use std::io::Read;
    use std::process::Stdio;
    let mut child = Command::new("git")
        .args(["clone", "--progress", "--", url])
        .arg(dest)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("git: {e}"))?;
    let mut stderr = child.stderr.take().unwrap();
    let (mut seg, mut tail, mut buf) = (Vec::<u8>::new(), Vec::<String>::new(), [0u8; 1024]);
    let flush = |seg: &mut Vec<u8>, progress: &mut dyn FnMut(&str), tail: &mut Vec<String>| {
        let line = String::from_utf8_lossy(seg).trim().to_string();
        seg.clear();
        if !line.is_empty() {
            progress(&line);
            tail.push(line);
            if tail.len() > 8 {
                tail.remove(0);
            }
        }
    };
    loop {
        match stderr.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                for &b in &buf[..n] {
                    // git redraws a progress line with \r
                    if b == b'\r' || b == b'\n' {
                        flush(&mut seg, &mut progress, &mut tail);
                    } else {
                        seg.push(b);
                    }
                }
            }
        }
    }
    flush(&mut seg, &mut progress, &mut tail);
    let status = child.wait().map_err(|e| format!("git: {e}"))?;
    if status.success() { Ok(()) } else { Err(tail.join("\n")) }
}

fn home_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).unwrap_or_default())
}

/// user.name / user.email of the global git config (a new machine has none).
pub fn global_identity() -> (String, String) {
    let get = |k: &str| git(&home_dir(), &["config", "--global", k]).map(|s| s.trim().to_string()).unwrap_or_default();
    (get("user.name"), get("user.email"))
}

pub fn set_global_identity(name: &str, email: &str) -> Result<(), String> {
    set_identity(&home_dir(), name, email, true)
}

const SSH_KEYS: [&str; 3] = ["id_ed25519", "id_rsa", "id_ecdsa"];

pub fn has_ssh_key() -> bool {
    SSH_KEYS.iter().any(|k| home_dir().join(".ssh").join(k).is_file())
}

/// The public half of the first SSH key found (what is pasted into GitLab / GitHub).
pub fn ssh_public_key() -> Option<String> {
    SSH_KEYS.iter().find_map(|k| std::fs::read_to_string(home_dir().join(".ssh").join(format!("{k}.pub"))).ok()).map(|s| s.trim().to_string())
}

/// `ssh-keygen -t ed25519` without passphrase into ~/.ssh/id_ed25519; refuses to overwrite a key.
pub fn generate_ssh_key(email: &str) -> Result<String, String> {
    let dir = home_dir().join(".ssh");
    let key = dir.join("id_ed25519");
    if key.exists() {
        return Err("Đã có ~/.ssh/id_ed25519, không ghi đè".into());
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    let out = Command::new("ssh-keygen")
        .args(["-t", "ed25519", "-N", "", "-q", "-C"])
        .arg(if email.trim().is_empty() { "chodilamide" } else { email.trim() })
        .arg("-f")
        .arg(&key)
        .output()
        .map_err(|e| format!("ssh-keygen: {e} (cài openssh)"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    ssh_public_key().ok_or_else(|| "Tạo key xong nhưng không đọc được file .pub".into())
}

/// Undo the last commit like SmartGit / IntelliJ: `reset --soft HEAD~1`, so its changes stay
/// staged and nothing is lost. Not for the first commit or while a merge / rebase is running.
pub fn undo_last_commit(root: &Path) -> Result<String, String> {
    if repo_state(root) != RepoState::Clean {
        return Err("Đang merge/rebase dở: hoàn tất hoặc abort trước".into());
    }
    if git(root, &["rev-parse", "--verify", "--quiet", "HEAD~1"]).is_err() {
        return Err("Đây là commit đầu tiên của repository, không thể undo".into());
    }
    git(root, &["reset", "--soft", "HEAD~1"])
}

/// HEAD is already on the upstream branch (undoing it would diverge from the remote).
pub fn head_pushed(root: &Path) -> bool {
    matches!(ahead_behind(root), Some((0, _)))
}

pub fn push(root: &Path, force_with_lease: bool) -> Result<String, String> {
    let mut args = vec!["push"];
    if force_with_lease {
        args.push("--force-with-lease");
    }
    let out = git_cmd(root).args(&args).output().map_err(|e| e.to_string())?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if out.status.success() {
        Ok(text)
    } else if text.contains("no upstream") || text.contains("has no upstream branch") {
        let b = branch(root).unwrap_or_default();
        let remote = default_remote(root);
        args.extend(["-u", &remote, &b]);
        git(root, &args)
    } else if is_rejected(&text) {
        Err(format!("Push bị từ chối: remote có commit bạn chưa có - Pull (Ctrl+T) rồi push lại\n{}", text.trim()))
    } else {
        Err(text.trim().to_string())
    }
}

pub fn is_rejected(msg: &str) -> bool {
    msg.contains("[rejected]") || msg.contains("non-fast-forward") || msg.contains("fetch first")
}

/// `git pull` as configured (merge or pull.rebase), local edits stashed and put back around it.
pub fn pull(root: &Path) -> Result<String, String> {
    git(root, &["pull", "--autostash"])
}

/// Discards the working-tree changes of `path` (back to HEAD).
pub fn rollback_file(root: &Path, path: &Path) -> Result<String, String> {
    git(root, &["checkout", "HEAD", "--", &path.to_string_lossy()])
}

// ---- working tree state (Git tool window) ------------------------------------------------

/// One entry of `git status`: index (staged) and work-tree letters, like porcelain XY.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub path: PathBuf,
    /// a rename's / copy's source
    pub orig: Option<PathBuf>,
    /// ' ' unchanged, M modified, A added, D deleted, R renamed, U unmerged, ? untracked
    pub index: char,
    pub worktree: char,
}

impl Change {
    pub fn conflicted(&self) -> bool {
        self.index == 'U' || self.worktree == 'U' || (self.index == 'A' && self.worktree == 'A') || (self.index == 'D' && self.worktree == 'D')
    }
    pub fn untracked(&self) -> bool {
        self.index == '?'
    }
    pub fn staged(&self) -> bool {
        !self.untracked() && !self.conflicted() && self.index != ' '
    }
    pub fn unstaged(&self) -> bool {
        !self.untracked() && !self.conflicted() && self.worktree != ' '
    }
    /// The paths git must be told about (a rename: both).
    pub fn paths(&self) -> Vec<PathBuf> {
        std::iter::once(self.path.clone()).chain(self.orig.clone()).collect()
    }
}

pub fn parse_changes(root: &Path, out: &str) -> Vec<Change> {
    let mut v = Vec::new();
    let mut parts = out.split('\0').filter(|s| !s.is_empty());
    while let Some(e) = parts.next() {
        if e.len() < 4 {
            continue;
        }
        let b = e.as_bytes();
        let (x, y) = (b[0] as char, b[1] as char);
        let orig = if x == 'R' || x == 'C' { parts.next().map(|o| root.join(o)) } else { None };
        v.push(Change { path: root.join(&e[3..]), orig, index: x, worktree: y });
    }
    v
}

/// Work-tree changes. Files that match .gitignore are left out even when they are tracked
/// (committed `target/classes/*.class`, `.classpath`…): the IDE neither shows nor manages them.
/// A staged or conflicted file is always kept, so a commit never contains anything invisible.
pub fn changes(root: &Path) -> Vec<Change> {
    let all = git(root, &["status", "--porcelain", "-z", "--untracked-files=all"]).map(|o| parse_changes(root, &o)).unwrap_or_default();
    let candidates: Vec<PathBuf> = all.iter().filter(|c| !c.untracked() && !c.conflicted() && !c.staged()).map(|c| c.path.clone()).collect();
    if candidates.is_empty() {
        return all;
    }
    let ignored = ignored_among(root, &candidates);
    if ignored.is_empty() {
        return all;
    }
    all.into_iter().filter(|c| !ignored.contains(&c.path)).collect()
}

/// The `candidates` that .gitignore patterns match, whether or not they are tracked
/// (`git check-ignore --no-index`, one call for the whole list).
pub fn ignored_among(root: &Path, candidates: &[PathBuf]) -> std::collections::HashSet<PathBuf> {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = match git_cmd(root).args(["check-ignore", "--no-index", "-z", "--stdin"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn() {
        Ok(c) => c,
        Err(_) => return Default::default(),
    };
    let input: Vec<u8> = candidates.iter().filter_map(|p| p.strip_prefix(root).ok()).flat_map(|p| p.to_string_lossy().replace('\\', "/").into_bytes().into_iter().chain(std::iter::once(0))).collect();
    // written from a thread: a long list must not block on a full pipe while git writes back
    let mut stdin = child.stdin.take().unwrap();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let out = child.wait_with_output();
    let _ = writer.join();
    let Ok(out) = out else { return Default::default() };
    String::from_utf8_lossy(&out.stdout).split('\0').filter(|s| !s.is_empty()).map(|rel| root.join(rel)).collect()
}

/// (ahead, behind) of the upstream branch; None without upstream.
pub fn ahead_behind(root: &Path) -> Option<(usize, usize)> {
    let out = git(root, &["rev-list", "--left-right", "--count", "HEAD...@{u}"]).ok()?;
    let mut it = out.split_whitespace().filter_map(|n| n.parse().ok());
    Some((it.next()?, it.next()?))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RepoState {
    Clean,
    Merging,
    Rebasing,
    CherryPicking,
    Reverting,
    /// no merge / rebase in progress but files are unmerged: the stash a pull (--autostash)
    /// or a stash pop could not put back cleanly. The stash is still there.
    Conflicted,
}

impl RepoState {
    pub fn label(self) -> &'static str {
        match self {
            RepoState::Clean => "",
            RepoState::Merging => "ĐANG MERGE",
            RepoState::Rebasing => "ĐANG REBASE",
            RepoState::CherryPicking => "ĐANG CHERRY-PICK",
            RepoState::Reverting => "ĐANG REVERT",
            RepoState::Conflicted => "XUNG ĐỘT SAU PULL/STASH",
        }
    }
    fn cmd(self) -> &'static str {
        match self {
            RepoState::Merging | RepoState::Clean | RepoState::Conflicted => "merge",
            RepoState::Rebasing => "rebase",
            RepoState::CherryPicking => "cherry-pick",
            RepoState::Reverting => "revert",
        }
    }
}

pub fn repo_state(root: &Path) -> RepoState {
    let Ok(dir) = git(root, &["rev-parse", "--git-dir"]) else { return RepoState::Clean };
    let d = root.join(dir.trim());
    if d.join("rebase-merge").exists() || d.join("rebase-apply").exists() {
        RepoState::Rebasing
    } else if d.join("MERGE_HEAD").exists() {
        RepoState::Merging
    } else if d.join("CHERRY_PICK_HEAD").exists() {
        RepoState::CherryPicking
    } else if d.join("REVERT_HEAD").exists() {
        RepoState::Reverting
    } else {
        RepoState::Clean
    }
}

/// merge / rebase / cherry-pick: "continue", "abort" or "skip" (rebase).
pub fn state_action(root: &Path, state: RepoState, action: &str) -> Result<String, String> {
    if state == RepoState::Clean {
        return Err("Không có merge/rebase nào đang dở".into());
    }
    if state == RepoState::Conflicted {
        return conflicted_action(root, action);
    }
    git(root, &[state.cmd(), &format!("--{action}")])
}

/// The state to show: git's own, or `Conflicted` when it says clean but files are unmerged
/// (a pull's autostash that did not apply cleanly).
pub fn effective_state(root: &Path, changes: &[Change]) -> RepoState {
    let s = repo_state(root);
    if s == RepoState::Clean && changes.iter().any(|c| c.conflicted()) { RepoState::Conflicted } else { s }
}

/// "continue": every file resolved → drop the pull's leftover autostash. "abort": throw the
/// conflicting work tree away (`reset --hard`), only while a stash still holds the user's changes.
fn conflicted_action(root: &Path, action: &str) -> Result<String, String> {
    let stashes = stashes(root);
    match action {
        "continue" => {
            if !git(root, &["ls-files", "-u"]).unwrap_or_default().trim().is_empty() {
                return Err("Còn file xung đột chưa giải quyết (chọn file → giải quyết → đánh dấu đã xong)".into());
            }
            match stashes.iter().find(|s| s.message.contains("autostash")) {
                Some(s) => git(root, &["stash", "drop", &format!("stash@{{{}}}", s.index)]),
                None => Ok("Đã giải quyết xong".into()),
            }
        }
        "abort" => {
            if stashes.is_empty() {
                return Err("Không có stash giữ thay đổi của bạn: reset --hard sẽ làm mất chúng, nên không làm".into());
            }
            git(root, &["reset", "--hard"]).map(|_| "Đã bỏ phần xung đột; thay đổi của bạn vẫn nằm trong stash (Stashes → Pop khi cần)".into())
        }
        other => Err(format!("{other}: không áp dụng cho xung đột sau pull/stash")),
    }
}

fn rels(root: &Path, paths: &[PathBuf]) -> Vec<String> {
    paths.iter().map(|p| p.strip_prefix(root).unwrap_or(p).to_string_lossy().to_string()).collect()
}

pub fn stage(root: &Path, paths: &[PathBuf]) -> Result<String, String> {
    let r = rels(root, paths);
    let mut args = vec!["add", "-A", "--"];
    args.extend(r.iter().map(String::as_str));
    git(root, &args)
}

pub fn unstage(root: &Path, paths: &[PathBuf]) -> Result<String, String> {
    let r = rels(root, paths);
    // before the first commit there is no HEAD to restore from
    let mut args = if has_head(root) { vec!["restore", "--staged", "--"] } else { vec!["rm", "-q", "--cached", "-r", "--"] };
    args.extend(r.iter().map(String::as_str));
    git(root, &args)
}

/// Throws away the work-tree changes (untracked files are deleted).
pub fn discard(root: &Path, c: &Change) -> Result<String, String> {
    if c.untracked() {
        return std::fs::remove_file(&c.path).map(|_| String::new()).map_err(|e| e.to_string());
    }
    let r = rels(root, std::slice::from_ref(&c.path));
    if c.index == 'A' || c.orig.is_some() {
        // newly added / a rename's new name: unstage and delete
        let _ = git(root, &["rm", "-q", "-f", "--cached", "--", &r[0]]);
        let _ = std::fs::remove_file(&c.path);
        // a rename: bring the old name back
        if let Some(o) = &c.orig {
            let ro = rels(root, std::slice::from_ref(o));
            return git(root, &["restore", "--staged", "--worktree", "--source=HEAD", "--", &ro[0]]);
        }
        return Ok(String::new());
    }
    git(root, &["restore", "--staged", "--worktree", "--source=HEAD", "--", &r[0]])
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stash {
    pub index: usize,
    pub message: String,
}

pub fn stashes(root: &Path) -> Vec<Stash> {
    git(root, &["stash", "list", "--format=%gs"])
        .unwrap_or_default()
        .lines()
        .enumerate()
        .map(|(i, l)| Stash { index: i, message: l.to_string() })
        .collect()
}

pub fn stash_push(root: &Path, message: &str, untracked: bool) -> Result<String, String> {
    let mut args = vec!["stash", "push"];
    if untracked {
        args.push("--include-untracked");
    }
    if !message.trim().is_empty() {
        args.push("-m");
        args.push(message);
    }
    git(root, &args)
}

/// "apply", "pop" or "drop" stash@{index}.
pub fn stash_op(root: &Path, op: &str, index: usize) -> Result<String, String> {
    git(root, &["stash", op, &format!("stash@{{{index}}}")])
}

pub fn merge(root: &Path, branch: &str) -> Result<String, String> {
    git(root, &["merge", "--no-edit", branch])
}

pub fn rebase(root: &Path, branch: &str) -> Result<String, String> {
    git(root, &["rebase", branch])
}

/// `path` as it is at `rev` (branch, commit, HEAD…); None when it doesn't exist there.
pub fn file_at(root: &Path, rev: &str, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    git(root, &["show", &format!("{rev}:{}", rel.to_string_lossy().replace('\\', "/"))]).ok()
}

// ---- side-by-side diff ------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RowKind {
    Same,
    Changed,
    Added,
    Removed,
}

/// One row of a two-column diff: left (old) and right (new) line with its 0-based number.
#[derive(Debug, Clone, PartialEq)]
pub struct DiffRow {
    pub left: Option<(usize, String)>,
    pub right: Option<(usize, String)>,
    pub kind: RowKind,
}

/// Aligns `old` and `new` for a side-by-side view: equal lines paired, a hunk's lines paired
/// as "changed" as far as both sides go, the rest added / removed.
pub fn side_by_side(old: &str, new: &str) -> Vec<DiffRow> {
    side_by_side_hunks(old, new).0
}

/// [`side_by_side`] plus each hunk with the row it starts at (stage / revert one block).
pub fn side_by_side_hunks(old: &str, new: &str) -> (Vec<DiffRow>, Vec<(usize, Hunk)>) {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let mut rows = Vec::new();
    let mut starts = Vec::new();
    let (mut i, mut j) = (0, 0);
    for h in diff_lines(old, new) {
        while i < h.old.start && j < h.new.start {
            rows.push(DiffRow { left: Some((i, a[i].into())), right: Some((j, b[j].into())), kind: RowKind::Same });
            i += 1;
            j += 1;
        }
        starts.push((rows.len(), h.clone()));
        let (n_old, n_new) = (h.old.len(), h.new.len());
        for k in 0..n_old.max(n_new) {
            let l = (k < n_old).then(|| (h.old.start + k, a[h.old.start + k].to_string()));
            let r = (k < n_new).then(|| (h.new.start + k, b[h.new.start + k].to_string()));
            let kind = match (&l, &r) {
                (Some(_), Some(_)) => RowKind::Changed,
                (Some(_), None) => RowKind::Removed,
                _ => RowKind::Added,
            };
            rows.push(DiffRow { left: l, right: r, kind });
        }
        i = h.old.end;
        j = h.new.end;
    }
    while i < a.len() && j < b.len() {
        rows.push(DiffRow { left: Some((i, a[i].into())), right: Some((j, b[j].into())), kind: RowKind::Same });
        i += 1;
        j += 1;
    }
    (rows, starts)
}

// ---- conflicts --------------------------------------------------------------------------------

/// One `<<<<<<< … ======= … >>>>>>>` block (diff3's `|||||||` base kept apart).
#[derive(Debug, Clone, PartialEq)]
pub struct Conflict {
    /// 0-based line of `<<<<<<<` and of `>>>>>>>`
    pub start: usize,
    pub end: usize,
    pub ours_label: String,
    pub theirs_label: String,
    pub ours: Vec<String>,
    pub base: Vec<String>,
    pub theirs: Vec<String>,
}

/// A conflict marker line: exactly 7 marker characters, then nothing or " label".
fn marker<'a>(line: &'a str, m: &str) -> Option<&'a str> {
    let rest = line.trim_end_matches('\r').strip_prefix(m)?;
    (rest.is_empty() || rest.starts_with(' ')).then(|| rest.trim())
}

pub fn parse_conflicts(text: &str) -> Vec<Conflict> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let Some(label) = marker(lines[i], "<<<<<<<") else {
            i += 1;
            continue;
        };
        let start = i;
        let mut c = Conflict { start, end: start, ours_label: label.trim().into(), theirs_label: String::new(), ours: vec![], base: vec![], theirs: vec![] };
        let mut part = 0; // 0 ours, 1 base, 2 theirs
        i += 1;
        let mut closed = false;
        while i < lines.len() {
            let l = lines[i];
            if part == 0 && marker(l, "|||||||").is_some() {
                part = 1;
            } else if part < 2 && marker(l, "=======") == Some("") {
                part = 2;
            } else if let Some(t) = marker(l, ">>>>>>>").filter(|_| part == 2) {
                c.theirs_label = t.trim().into();
                c.end = i;
                closed = true;
                break;
            } else {
                match part {
                    0 => c.ours.push(l.into()),
                    1 => c.base.push(l.into()),
                    _ => c.theirs.push(l.into()),
                }
            }
            i += 1;
        }
        if closed {
            out.push(c);
        }
        i += 1;
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Pick {
    Ours,
    Theirs,
    /// ours then theirs
    Both,
    /// the common ancestor (diff3 / zdiff3 markers or the base from the index)
    Base,
}

/// `text` with conflict `index` replaced by the chosen side(s).
pub fn resolve_conflict(text: &str, index: usize, pick: Pick) -> String {
    let Some(c) = parse_conflicts(text).into_iter().nth(index) else { return text.to_string() };
    let chosen: Vec<String> = match pick {
        Pick::Ours => c.ours.clone(),
        Pick::Theirs => c.theirs.clone(),
        Pick::Both => c.ours.iter().chain(&c.theirs).cloned().collect(),
        Pick::Base => c.base.clone(),
    };
    replace_conflict(text, index, &chosen)
}

/// `text` with conflict `index` (markers included) replaced by `chosen` lines.
pub fn replace_conflict(text: &str, index: usize, chosen: &[String]) -> String {
    let Some(c) = parse_conflicts(text).into_iter().nth(index) else { return text.to_string() };
    let lines: Vec<&str> = text.lines().collect();
    let chosen = chosen.to_vec();
    let mut out: Vec<String> = lines[..c.start].iter().map(|s| s.to_string()).collect();
    out.extend(chosen);
    out.extend(lines[c.end + 1..].iter().map(|s| s.to_string()));
    let mut s = out.join("\n");
    if text.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// Whole file from one side (`git checkout --ours/--theirs`) and marked resolved. A side
/// that deleted the file (DU / UD / DD) resolves by deleting it.
pub fn take_side(root: &Path, c: &Change, ours: bool) -> Result<String, String> {
    let r = rels(root, std::slice::from_ref(&c.path));
    let deleted = if ours { c.index == 'D' } else { c.worktree == 'D' };
    if deleted {
        return git(root, &["rm", "-q", "-f", "--", &r[0]]);
    }
    git(root, &["checkout", if ours { "--ours" } else { "--theirs" }, "--", &r[0]])?;
    stage(root, &[c.path.clone()])
}

/// The file's conflicts are settled: `git add`.
pub fn mark_resolved(root: &Path, path: &Path) -> Result<String, String> {
    stage(root, &[path.to_path_buf()])
}

// ---- accounts ------------------------------------------------------------------------

/// Is this git error a missing / wrong login?
pub fn is_auth_error(msg: &str) -> bool {
    let m = msg.to_lowercase();
    ["authentication failed", "could not read username", "could not read password", "terminal prompts disabled", "invalid username or password", "403", "401", "permission denied"].iter().any(|w| m.contains(w))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Remote {
    pub name: String,
    pub url: String,
    /// host of an https remote (None for ssh: keys, not accounts)
    pub https_host: Option<String>,
}

pub fn remotes(root: &Path) -> Vec<Remote> {
    let out = git(root, &["remote", "-v"]).unwrap_or_default();
    let mut v: Vec<Remote> = Vec::new();
    for l in out.lines() {
        let mut p = l.split_whitespace();
        let (Some(name), Some(url)) = (p.next(), p.next()) else { continue };
        if v.iter().any(|r| r.name == name) {
            continue;
        }
        let https_host = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")).map(|r| {
            let r = r.split('/').next().unwrap_or(r);
            r.rsplit('@').next().unwrap_or(r).to_string()
        });
        v.push(Remote { name: name.into(), url: url.into(), https_host });
    }
    v
}

#[derive(Debug, Clone, PartialEq)]
pub struct Account {
    pub protocol: String,
    pub host: String,
    pub username: String,
}

pub fn credentials_file() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).unwrap_or_default()).join(".git-credentials")
}

/// Accounts in ~/.git-credentials (git's store helper): `https://user:token@host` per line.
pub fn parse_credentials(text: &str) -> Vec<Account> {
    text.lines()
        .filter_map(|l| {
            let (protocol, rest) = l.trim().split_once("://")?;
            let (userinfo, host) = rest.rsplit_once('@')?;
            let user = userinfo.split(':').next()?;
            Some(Account { protocol: protocol.into(), host: host.trim_end_matches('/').into(), username: percent_decode(user) })
        })
        .collect()
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn accounts() -> Vec<Account> {
    std::fs::read_to_string(credentials_file()).map(|t| parse_credentials(&t)).unwrap_or_default()
}

fn credential_store(action: &str, input: &str) -> Result<(), String> {
    use std::io::Write;
    let file = credentials_file();
    let mut child = Command::new("git")
        .args(["credential-store", "--file"])
        .arg(&file)
        .arg(action)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("git: {e}"))?;
    child.stdin.take().ok_or("stdin")?.write_all(input.as_bytes()).map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    // the token file must stay private
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600));
    }
    if out.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&out.stderr).into_owned()) }
}

/// Saves (or replaces) the login for `host` - a personal access token as the password
/// (GitHub/GitLab no longer accept account passwords over https).
pub fn save_account(host: &str, username: &str, token: &str) -> Result<(), String> {
    if host.trim().is_empty() || username.trim().is_empty() || token.trim().is_empty() {
        return Err("Cần host, username và token".into());
    }
    // one login per host: drop the old one first
    let _ = credential_store("erase", &format!("protocol=https\nhost={}\n\n", host.trim()));
    credential_store("store", &format!("protocol=https\nhost={}\nusername={}\npassword={}\n\n", host.trim(), username.trim(), token.trim()))?;
    ensure_store_helper();
    Ok(())
}

/// Plain `git` (terminal, lazygit, the IDE's terminal) reads ~/.git-credentials only with a
/// store helper configured: set it globally when the user has no credential helper at all
/// (an existing libsecret / manager setup is left alone).
pub fn ensure_store_helper() {
    let out = Command::new("git").args(["config", "--global", "--get-all", "credential.helper"]).output();
    let none = out.map(|o| String::from_utf8_lossy(&o.stdout).trim().is_empty()).unwrap_or(false);
    if none {
        let _ = Command::new("git").args(["config", "--global", "credential.helper", "store"]).status();
    }
}

pub fn remove_account(a: &Account) -> Result<(), String> {
    credential_store("erase", &format!("protocol={}\nhost={}\nusername={}\n\n", a.protocol, a.host, a.username))
}

/// user.name / user.email: (value for this repository, global value)
pub fn identity(root: &Path) -> ((String, String), (String, String)) {
    let get = |args: &[&str]| git(root, args).map(|s| s.trim().to_string()).unwrap_or_default();
    (
        (get(&["config", "--local", "user.name"]), get(&["config", "--local", "user.email"])),
        (get(&["config", "--global", "user.name"]), get(&["config", "--global", "user.email"])),
    )
}

/// Sets user.name/email for this repository (`global` = for every repository). Empty = unset.
pub fn set_identity(root: &Path, name: &str, email: &str, global: bool) -> Result<(), String> {
    let scope = if global { "--global" } else { "--local" };
    for (k, v) in [("user.name", name), ("user.email", email)] {
        if v.trim().is_empty() {
            let _ = git(root, &["config", scope, "--unset", k]);
        } else {
            git(root, &["config", scope, k, v.trim()])?;
        }
    }
    Ok(())
}

pub fn fetch(root: &Path) -> Result<String, String> {
    git(root, &["fetch", "--all", "--prune"])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn myers_hunks_and_marks() {
        let old = "a\nb\nc\nd\ne\n";
        let new = "a\nB\nc\nd\nx\ny\ne\n";
        let h = diff_lines(old, new);
        assert_eq!(h, [Hunk { old: 1..2, new: 1..2 }, Hunk { old: 4..4, new: 4..6 }]);
        assert_eq!(line_marks(&h), [(1, LineMark::Modified), (4, LineMark::Added), (5, LineMark::Added)]);
        let h = diff_lines("a\nb\nc\n", "a\nc\n");
        assert_eq!(h, [Hunk { old: 1..2, new: 1..1 }]);
        assert_eq!(line_marks(&h), [(1, LineMark::Deleted)]);
        assert!(diff_lines("x\n", "x\n").is_empty());
        // revert the modified line back to "b"
        let h = diff_lines(old, new);
        assert_eq!(revert_hunk(old, &h, 1), Some((1..2, "b\n".to_string())));
        // bigger reorder still consistent: applying hunks old→new reproduces new
        let (o, n) = ("1\n2\n3\n4\n5\n6\n7\n8\n", "0\n1\n3\n4\n4b\n6\n8\n9\n");
        let h = diff_lines(o, n);
        let (ol, nl): (Vec<&str>, Vec<&str>) = (o.lines().collect(), n.lines().collect());
        let mut rebuilt: Vec<&str> = Vec::new();
        let mut i = 0;
        for x in &h {
            rebuilt.extend(&ol[i..x.old.start]);
            rebuilt.extend(&nl[x.new.clone()]);
            i = x.old.end;
        }
        rebuilt.extend(&ol[i..]);
        assert_eq!(rebuilt, nl);
    }

    #[test]
    fn side_by_side_rows() {
        let rows = side_by_side("a\nb\nc\nd\n", "a\nB\nc\nx\nd\n");
        let k: Vec<RowKind> = rows.iter().map(|r| r.kind).collect();
        assert_eq!(k, [RowKind::Same, RowKind::Changed, RowKind::Same, RowKind::Added, RowKind::Same]);
        assert_eq!(rows[3].left, None);
        assert_eq!(rows[3].right, Some((3, "x".into())));
        assert_eq!(rows[4].left, Some((3, "d".into())));
    }

    #[test]
    fn conflict_blocks() {
        let t = "a\n<<<<<<< HEAD\nmine\n=======\ntheirs1\ntheirs2\n>>>>>>> feature\nb\n<<<<<<< HEAD\nx\n||||||| base\no\n=======\ny\n>>>>>>> feature\n";
        let c = parse_conflicts(t);
        assert_eq!(c.len(), 2);
        assert_eq!((c[0].ours.clone(), c[0].theirs.clone(), c[0].theirs_label.as_str()), (vec!["mine".to_string()], vec!["theirs1".to_string(), "theirs2".to_string()], "feature"));
        assert_eq!(c[1].base, ["o"]);
        let r = resolve_conflict(t, 0, Pick::Theirs);
        assert!(r.starts_with("a\ntheirs1\ntheirs2\nb\n<<<<<<<"));
        let r = resolve_conflict(&r, 0, Pick::Both);
        assert_eq!(r, "a\ntheirs1\ntheirs2\nb\nx\ny\n");
        let ch = parse_changes(Path::new("/r"), "UU c.txt\0M  s.txt\0 M w.txt\0?? n.txt\0");
        assert!(ch[0].conflicted() && ch[1].staged() && !ch[1].unstaged() && ch[2].unstaged() && ch[3].untracked());
    }

    #[test]
    fn accounts_and_remotes() {
        let a = parse_credentials("https://huy%40lv:ghp_x@github.com\nhttps://oauth2:tok@gitlab.longvan.net\n");
        assert_eq!(a[0], Account { protocol: "https".into(), host: "github.com".into(), username: "huy@lv".into() });
        assert_eq!(a[1].host, "gitlab.longvan.net");
        assert!(is_auth_error("fatal: could not read Username for 'https://github.com': terminal prompts disabled"));
        assert!(!is_auth_error("Everything up-to-date"));
    }

    #[test]
    fn status_and_blame_parsing() {
        let root = Path::new("/r");
        let s = parse_status(root, " M src/A.java\0?? new.txt\0R  b.txt\0a.txt\0UU c.txt\0A  d.txt\0");
        assert_eq!(s[&root.join("src/A.java")], FileStatus::Modified);
        assert_eq!(s[&root.join("new.txt")], FileStatus::Untracked);
        assert_eq!(s[&root.join("b.txt")], FileStatus::Renamed);
        assert_eq!(s[&root.join("c.txt")], FileStatus::Conflict);
        assert_eq!(s[&root.join("d.txt")], FileStatus::Added);
        assert!(!s.contains_key(&root.join("a.txt")));
        let blame = "0123456789abcdef0123456789abcdef01234567 1 1 1\nauthor Huy\nauthor-time 1790000000\nsummary init\n\tclass A {}\n";
        let b = parse_blame(blame);
        assert_eq!(b.len(), 1);
        assert_eq!((b[0].short.as_str(), b[0].author.as_str(), b[0].summary.as_str()), ("01234567", "Huy", "init"));
        assert_eq!(ymd(0), "1970-01-01");
        assert_eq!(ymd(1_790_000_000), "2026-09-21"); // date -u -d @1790000000
    }

    /// Against a real temporary repository (skipped when git is missing).
    #[test]
    fn real_repo_roundtrip() {
        let dir = std::env::temp_dir().join(format!("vcs-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        if git(&dir, &["init", "-q", "-b", "main"]).is_err() {
            return;
        }
        let _ = git(&dir, &["config", "user.email", "t@t"]);
        let _ = git(&dir, &["config", "user.name", "T"]);
        let f = dir.join("A.txt");
        std::fs::write(&f, "one\ntwo\n").unwrap();
        commit(&dir, &[f.clone()], "first", false).unwrap();
        std::fs::write(&f, "one\n2\n").unwrap();
        let root = repo_root(&dir).unwrap();
        assert_eq!(status(&root).get(&root.join("A.txt")), Some(&FileStatus::Modified));
        assert_eq!(head_text(&root, &root.join("A.txt")).as_deref(), Some("one\ntwo\n"));
        assert_eq!(branch(&root).as_deref(), Some("main"));
        assert_eq!(log(&root, None, 5)[0].subject, "first");
        assert_eq!(blame(&root, &root.join("A.txt")).len(), 2);
        rollback_file(&root, &root.join("A.txt")).unwrap();
        // a real merge conflict: state, conflict file, resolve, continue
        git(&root, &["checkout", "-q", "-b", "feat"]).unwrap();
        std::fs::write(&f, "one\nFEAT\n").unwrap();
        commit(&root, &[f.clone()], "feat", false).unwrap();
        git(&root, &["checkout", "-q", "main"]).unwrap();
        std::fs::write(&f, "one\nMAIN\n").unwrap();
        commit(&root, &[f.clone()], "main", false).unwrap();
        assert!(merge(&root, "feat").is_err());
        assert_eq!(repo_state(&root), RepoState::Merging);
        assert!(changes(&root).iter().any(|c| c.conflicted()));
        let text = std::fs::read_to_string(&f).unwrap();
        std::fs::write(&f, resolve_conflict(&text, 0, Pick::Both)).unwrap();
        mark_resolved(&root, &f).unwrap();
        state_action(&root, RepoState::Merging, "continue").unwrap();
        assert_eq!(repo_state(&root), RepoState::Clean);
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "one\nMAIN\nFEAT\n");
        // stash round trip
        std::fs::write(&f, "dirty\n").unwrap();
        stash_push(&root, "wip", false).unwrap();
        assert_eq!(stashes(&root).len(), 1);
        stash_op(&root, "pop", 0).unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "dirty\n");
        rollback_file(&root, &f).unwrap();
        // pull with pull.rebase=true and uncommitted edits (product-service's situation)
        let clone = dir.with_extension("clone");
        let _ = std::fs::remove_dir_all(&clone);
        git(&dir, &["clone", "-q", &dir.to_string_lossy(), &clone.to_string_lossy()]).unwrap();
        let _ = git(&clone, &["config", "pull.rebase", "true"]);
        let _ = git(&clone, &["config", "user.email", "t@t"]);
        let _ = git(&clone, &["config", "user.name", "T"]);
        std::fs::write(dir.join("B.txt"), "new upstream\n").unwrap();
        commit(&dir, &[dir.join("B.txt")], "second", false).unwrap();
        std::fs::write(clone.join("A.txt"), "one\nlocal edit\n").unwrap();
        pull(&clone).unwrap();
        assert!(clone.join("B.txt").exists(), "pulled");
        assert_eq!(std::fs::read_to_string(clone.join("A.txt")).unwrap(), "one\nlocal edit\n", "edit kept");
        std::fs::remove_dir_all(&clone).unwrap();
        let _ = git(&root, &["remote", "add", "origin", "https://tok@gitlab.example.com/g/p.git"]);
        assert_eq!(remotes(&root)[0].https_host.as_deref(), Some("gitlab.example.com"));
        set_identity(&root, "Huy", "h@x.vn", false).unwrap();
        assert_eq!(identity(&root).0, ("Huy".to_string(), "h@x.vn".to_string()));
        // the merge above is the file's latest content
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "one\nMAIN\nFEAT\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ref_list_parses_tracking_remotes_and_tags() {
        let out = "refs/heads/dev\t \torigin/dev\tbehind 2\nrefs/heads/prod\t*\torigin/prod\tahead 2, behind 5\nrefs/heads/old\t \torigin/old\tgone\nrefs/heads/local\t \t\t\nrefs/remotes/origin/HEAD\t \t\t\nrefs/remotes/origin/dev\t \t\t\nrefs/remotes/up/main\t \t\t\nrefs/tags/v1\t \t\t\nrefs/tags/v2\t \t\t\n";
        let l = parse_ref_list(out);
        assert_eq!(l.locals.len(), 4);
        assert_eq!((l.locals[0].ahead, l.locals[0].behind), (0, 2));
        assert!(l.locals[1].current && l.locals[1].ahead == 2 && l.locals[1].behind == 5);
        assert!(l.locals[2].gone);
        assert!(l.locals[3].upstream.is_none());
        assert_eq!(l.remotes, vec![("origin".to_string(), vec!["dev".to_string()]), ("up".to_string(), vec!["main".to_string()])]);
        assert_eq!(l.tags, vec!["v2".to_string(), "v1".to_string()]);
    }

    #[test]
    fn tracked_but_ignored_changes_are_hidden() {
        let dir = std::env::temp_dir().join(format!("vcs-ign-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("target")).unwrap();
        let g = |a: &[&str]| git(&dir, a).is_ok();
        if !g(&["init", "-q", "-b", "main"]) {
            return;
        }
        let _ = git(&dir, &["config", "user.email", "t@t"]);
        let _ = git(&dir, &["config", "user.name", "T"]);
        std::fs::write(dir.join("keep.txt"), "1\n").unwrap();
        std::fs::write(dir.join("target/A.class"), "1\n").unwrap();
        std::fs::write(dir.join("staged.class"), "1\n").unwrap();
        g(&["add", "-A"]);
        g(&["commit", "-qm", "init"]);
        // the ignore rule arrives after the files were committed
        std::fs::write(dir.join(".gitignore"), "target/\n*.class\n").unwrap();
        std::fs::write(dir.join("keep.txt"), "2\n").unwrap();
        std::fs::write(dir.join("target/A.class"), "2\n").unwrap();
        std::fs::write(dir.join("staged.class"), "2\n").unwrap();
        g(&["add", "staged.class"]);
        std::fs::write(dir.join("new.txt"), "x\n").unwrap();
        let names: Vec<String> = changes(&dir).iter().map(|c| c.path.strip_prefix(&dir).unwrap().to_string_lossy().to_string()).collect();
        assert!(names.contains(&"keep.txt".to_string()) && names.contains(&"new.txt".to_string()) && names.contains(&".gitignore".to_string()), "{names:?}");
        assert!(!names.contains(&"target/A.class".to_string()), "tracked + ignored is hidden: {names:?}");
        assert!(names.contains(&"staged.class".to_string()), "staged stays visible: {names:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn autostash_conflict_is_a_state_with_continue_and_abort() {
        let base = std::env::temp_dir().join(format!("vcs-autostash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let (remote, a, b) = (base.join("r.git"), base.join("a"), base.join("b"));
        let run = |dir: &Path, args: &[&str]| git(dir, args).is_ok();
        if !run(&base, &["init", "-q", "--bare", "-b", "main", remote.to_str().unwrap()]) {
            return;
        }
        for d in [&a, &b] {
            run(&base, &["clone", "-q", remote.to_str().unwrap(), d.to_str().unwrap()]);
            let _ = git(d, &["config", "user.email", "t@t"]);
            let _ = git(d, &["config", "user.name", "T"]);
        }
        std::fs::write(a.join("f.txt"), "1\n").unwrap();
        run(&a, &["add", "."]);
        run(&a, &["commit", "-qm", "init"]);
        run(&a, &["push", "-q", "origin", "main"]);
        run(&b, &["pull", "-q"]);
        std::fs::write(b.join("f.txt"), "remote\n").unwrap();
        run(&b, &["commit", "-qam", "remote"]);
        run(&b, &["push", "-q"]);
        // a has an uncommitted edit of the same line, then pulls
        std::fs::write(a.join("f.txt"), "dirty\n").unwrap();
        let _ = pull(&a);
        let ch = changes(&a);
        assert!(ch.iter().any(|c| c.conflicted()), "the autostash conflicts");
        assert_eq!(repo_state(&a), RepoState::Clean, "git itself reports nothing in progress");
        assert_eq!(effective_state(&a, &ch), RepoState::Conflicted);
        // continue before resolving is refused
        assert!(state_action(&a, RepoState::Conflicted, "continue").is_err());
        // abort: work tree back to the pulled commit, the stash keeps the user's edit
        state_action(&a, RepoState::Conflicted, "abort").unwrap();
        assert_eq!(std::fs::read_to_string(a.join("f.txt")).unwrap(), "remote\n");
        assert_eq!(stashes(&a).len(), 1);
        assert_eq!(effective_state(&a, &changes(&a)), RepoState::Clean);
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn undo_last_commit_keeps_changes_staged() {
        let dir = std::env::temp_dir().join(format!("vcs-undo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        if git(&dir, &["init", "-q", "-b", "main"]).is_err() {
            return;
        }
        let _ = git(&dir, &["config", "user.email", "t@t"]);
        let _ = git(&dir, &["config", "user.name", "T"]);
        std::fs::write(dir.join("A.txt"), "1\n").unwrap();
        commit(&dir, &[dir.join("A.txt")], "first", false).unwrap();
        assert!(undo_last_commit(&dir).is_err(), "the first commit cannot be undone");
        std::fs::write(dir.join("A.txt"), "2\n").unwrap();
        commit(&dir, &[dir.join("A.txt")], "second", false).unwrap();
        assert!(!head_pushed(&dir), "no upstream: nothing pushed");
        assert_eq!(last_message(&dir), "second");
        undo_last_commit(&dir).unwrap();
        assert_eq!(last_message(&dir), "first", "HEAD is back");
        assert_eq!(std::fs::read_to_string(dir.join("A.txt")).unwrap(), "2\n", "the work tree is untouched");
        assert!(changes(&dir).iter().any(|c| c.staged()), "the undone changes are staged");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn repo_urls_are_understood() {
        let u = |s: &str| parse_repo_url(s).unwrap();
        assert_eq!(u("https://gitlab.longvan.vn/team/computing-management.git"), RepoUrl { host: "gitlab.longvan.vn".into(), https: true, ssh: false, name: "computing-management".into() });
        assert_eq!(u("https://user:tok@github.com:443/a/b/").host, "github.com");
        assert_eq!(u("https://github.com/a/b/").name, "b");
        let s = u("git@gitlab.com:grp/sub/repo.git");
        assert!(s.ssh && !s.https && s.host == "gitlab.com" && s.name == "repo");
        assert!(u("ssh://git@host.vn:2222/g/r.git").ssh);
        let local = u("/home/me/proj.git");
        assert!(!local.https && !local.ssh && local.host.is_empty() && local.name == "proj");
        assert_eq!(u("file:///srv/x/y.git").name, "y");
        assert!(parse_repo_url("   ").is_none() && parse_repo_url("https://host/").is_none());
    }

    #[test]
    fn clone_reports_progress_and_fails_cleanly() {
        let base = std::env::temp_dir().join(format!("vcs-clone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let src = base.join("src");
        std::fs::create_dir_all(&src).unwrap();
        if git(&src, &["init", "-q", "-b", "main"]).is_err() {
            return;
        }
        let _ = git(&src, &["config", "user.email", "t@t"]);
        let _ = git(&src, &["config", "user.name", "T"]);
        std::fs::write(src.join("A.txt"), "1\n").unwrap();
        commit(&src, &[src.join("A.txt")], "init", false).unwrap();
        let dest = base.join("copy");
        let mut lines = Vec::new();
        clone_repo(src.to_str().unwrap(), &dest, |l| lines.push(l.to_string())).unwrap();
        assert!(dest.join("A.txt").is_file(), "cloned");
        assert!(lines.iter().any(|l| l.contains("Cloning")), "progress lines: {lines:?}");
        // a repository that does not exist: an error, no hang
        let err = clone_repo(base.join("nope").to_str().unwrap(), &base.join("x"), |_| {}).unwrap_err();
        assert!(!err.is_empty());
        std::fs::remove_dir_all(&base).unwrap();
    }
}
