//! Read facts about worktrees from git, `lsof` and `du`. Nothing here writes.

use crate::decide::{Facts, Hint};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug)]
pub struct Worktree {
    pub repo: PathBuf,
    pub path: PathBuf,
    pub branch: Option<String>,
    /// Commit checked out, so a removal can be undone.
    pub head: Option<String>,
    pub facts: Facts,
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The main repo of the current folder, even when run from a linked worktree.
pub fn current_repo() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let common = git(
        &cwd,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let common = PathBuf::from(common.trim());
    // `<repo>/.git` for a normal repo. A bare repo has no working folder to scan.
    (common.file_name()? == ".git").then(|| common.parent().map(Path::to_path_buf))?
}

/// Repositories under `root`: directories holding a `.git` directory, at most
/// `depth` levels down. Linked worktrees have a `.git` file, so they are not
/// listed as repos; they are found through their main repo.
pub fn find_repos(root: &Path, depth: usize) -> Vec<PathBuf> {
    let mut found = Vec::new();
    walk(root, depth, &mut found);
    found.sort();
    found
}

fn walk(dir: &Path, depth: usize, found: &mut Vec<PathBuf>) {
    if dir.join(".git").is_dir() {
        found.push(dir.to_path_buf());
        return;
    }
    if depth == 0 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        if p.is_dir() && !p.is_symlink() {
            walk(&p, depth - 1, found);
        }
    }
}

struct Listed {
    path: PathBuf,
    branch: Option<String>,
    locked: bool,
    bare: bool,
}

fn list_worktrees(repo: &Path) -> Vec<Listed> {
    let Some(text) = git(repo, &["worktree", "list", "--porcelain"]) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for block in text.split("\n\n") {
        let mut l = Listed {
            path: PathBuf::new(),
            branch: None,
            locked: false,
            bare: false,
        };
        for line in block.lines() {
            if let Some(p) = line.strip_prefix("worktree ") {
                l.path = PathBuf::from(p);
            } else if let Some(b) = line.strip_prefix("branch ") {
                l.branch = Some(b.trim_start_matches("refs/heads/").to_string());
            } else if line == "locked" || line.starts_with("locked ") {
                l.locked = true;
            } else if line == "bare" {
                l.bare = true;
            }
        }
        if !l.path.as_os_str().is_empty() {
            out.push(l);
        }
    }
    out
}

fn default_branch_ref(repo: &Path) -> Option<String> {
    if let Some(s) = git(
        repo,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    ) {
        return Some(s.trim().to_string());
    }
    ["origin/main", "origin/master"]
        .into_iter()
        .find(|r| git(repo, &["rev-parse", "--verify", "--quiet", r]).is_some())
        .map(String::from)
}

/// Every path a process has open or uses as its working directory, from one
/// `lsof` call. An empty set if `lsof` is missing.
pub fn open_paths() -> Vec<String> {
    let Ok(out) = Command::new("lsof").args(["-w", "-F", "n"]).output() else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.strip_prefix('n'))
        .filter(|p| p.starts_with('/'))
        .map(String::from)
        .collect()
}

fn in_use(path: &Path, open: &[String]) -> bool {
    let p = path.to_string_lossy();
    open.iter().any(|o| {
        o == p.as_ref()
            || o.strip_prefix(p.as_ref())
                .is_some_and(|r| r.starts_with('/'))
    })
}

/// Disk use of one folder, from `du`. Slow on big trees, so callers ask only
/// for the folders they need.
pub fn du_bytes(path: &Path) -> u64 {
    Command::new("du")
        .arg("-sk")
        .arg(path)
        .output()
        .ok()
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .split_whitespace()
                .next()
                .and_then(|n| n.parse::<u64>().ok())
        })
        .map_or(0, |k| k * 1024)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn mtime(p: &Path) -> Option<u64> {
    std::fs::metadata(p)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

/// Last activity: the newest of the last commit, the index file, and the
/// worktree folder. An approximation; it does not walk the tree.
fn idle_secs(path: &Path) -> u64 {
    let commit = git(path, &["log", "-1", "--format=%ct"])
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0);
    let gitdir = git(path, &["rev-parse", "--absolute-git-dir"]).map(|s| PathBuf::from(s.trim()));
    let index = gitdir
        .as_ref()
        .and_then(|g| mtime(&g.join("index")))
        .unwrap_or(0);
    let newest = commit.max(index).max(mtime(path).unwrap_or(0));
    now().saturating_sub(newest)
}

fn facts_for(l: &Listed, repo: &Path, default_ref: Option<&str>, open: &[String]) -> Facts {
    let is_main = l.path == repo || l.bare;
    let dirty = git(&l.path, &["status", "--porcelain"]).is_none_or(|s| !s.trim().is_empty());
    // If git cannot answer, assume the worst: dirty and unpushed.
    let unpushed = git(&l.path, &["log", "--oneline", "HEAD", "--not", "--remotes"])
        .map_or(usize::MAX, |s| s.lines().count());
    let merged = default_ref.is_some_and(|d| {
        Command::new("git")
            .arg("-C")
            .arg(&l.path)
            .args(["merge-base", "--is-ancestor", "HEAD", d])
            .status()
            .is_ok_and(|s| s.success())
    });
    Facts {
        is_main,
        locked: l.locked,
        dirty,
        unpushed,
        merged,
        in_use: in_use(&l.path, open),
        idle_secs: idle_secs(&l.path),
        hint: None,
    }
}

/// Gather every linked worktree under the given repos. Worktrees whose folder
/// is gone are skipped.
pub fn gather(repos: &[PathBuf], open: &[String]) -> Vec<Worktree> {
    let mut jobs: Vec<(PathBuf, Listed, Option<String>)> = Vec::new();
    for repo in repos {
        let default_ref = default_branch_ref(repo);
        for l in list_worktrees(repo) {
            if l.path != *repo && l.path.is_dir() {
                jobs.push((repo.clone(), l, default_ref.clone()));
            }
        }
    }
    // A worktree can be listed by more than one repo path (shared git dir).
    let mut seen: HashMap<PathBuf, ()> = HashMap::new();
    jobs.retain(|(_, l, _)| seen.insert(l.path.clone(), ()).is_none());

    let mut out: Vec<Worktree> = Vec::with_capacity(jobs.len());
    for chunk in jobs.chunks(8) {
        let done: Vec<Worktree> = std::thread::scope(|s| {
            let handles: Vec<_> = chunk
                .iter()
                .map(|(repo, l, d)| {
                    s.spawn(move || Worktree {
                        repo: repo.clone(),
                        path: l.path.clone(),
                        branch: l.branch.clone(),
                        head: git(&l.path, &["rev-parse", "HEAD"]).map(|h| h.trim().to_string()),
                        facts: facts_for(l, repo, d.as_deref(), open),
                    })
                })
                .collect();
            handles.into_iter().filter_map(|h| h.join().ok()).collect()
        });
        out.extend(done);
    }
    out
}

/// Apply provider hints by path.
#[allow(dead_code)] // used once providers land
pub fn apply_hints(wts: &mut [Worktree], hints: &HashMap<PathBuf, Hint>) {
    for w in wts {
        if let Some(h) = hints.get(&w.path) {
            w.facts.hint = Some(*h);
        }
    }
}

/// Measure many folders at once, eight at a time.
pub fn du_many(paths: &[PathBuf]) -> Vec<u64> {
    let mut out = Vec::with_capacity(paths.len());
    for chunk in paths.chunks(8) {
        let done: Vec<u64> = std::thread::scope(|s| {
            let hs: Vec<_> = chunk.iter().map(|p| s.spawn(move || du_bytes(p))).collect();
            hs.into_iter().map(|h| h.join().unwrap_or(0)).collect()
        });
        out.extend(done);
    }
    out
}
