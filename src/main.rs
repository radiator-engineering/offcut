mod cache;
mod decide;
mod gather;
mod manifest;
mod provider;
mod schedule;

use clap::{Parser, Subcommand};
use decide::{Verdict, decide};
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

#[derive(Parser)]
#[command(
    version,
    about = "Find and safely remove the worktrees and caches that agents and builds leave behind"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
    #[command(flatten)]
    scan: Scan,
}

#[derive(clap::Args, Clone)]
struct Scan {
    /// Search this folder for git repos (repeatable). Default: only the repo you are in.
    #[arg(long = "root", global = true)]
    roots: Vec<PathBuf>,
    /// Search all of ~/Development instead of one repo.
    #[arg(long, global = true, conflicts_with = "roots")]
    all: bool,
    /// Remove a clean, pushed, unmerged worktree once it has been idle this many days.
    #[arg(long, default_value_t = 30, global = true)]
    max_age: u64,
    /// Also look at build caches (Bazel output bases). Implied by --all.
    #[arg(long, global = true)]
    caches: bool,
    /// Measure every worktree, not only the removable ones. Slower: `du` on big trees.
    #[arg(long, global = true)]
    sizes: bool,
    /// Print JSON instead of a table.
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Subcommand)]
enum Cmd {
    /// List worktrees and what offcut would do with each. Deletes nothing. The default.
    Report,
    /// Remove the worktrees the report marks `remove`.
    Apply {
        /// Do not ask for confirmation.
        #[arg(long)]
        yes: bool,
        /// Remove only this kind of item.
        #[arg(long, value_enum)]
        only: Option<Only>,
    },
    /// Keep the machine clean with a scheduled job (macOS launchd).
    Schedule {
        #[command(subcommand)]
        action: ScheduleCmd,
    },
    /// Put back worktrees that `apply` removed. With no path, list what can be restored.
    Restore {
        /// Worktree paths to restore.
        paths: Vec<PathBuf>,
    },
}

#[derive(Subcommand)]
enum ScheduleCmd {
    /// Install the job: every 6 hours, remove unused build caches and save a report.
    Install,
    /// Remove the job.
    Remove,
    /// Show whether the job is installed and when it last reported.
    Status,
}

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum Only {
    Worktrees,
    Caches,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Worktree,
    Cache,
}

#[derive(Serialize)]
struct Row {
    kind: Kind,
    path: String,
    repo: String,
    branch: Option<String>,
    #[serde(skip)]
    head: Option<String>,
    size_bytes: u64,
    #[serde(flatten)]
    verdict: Verdict,
}

fn gib(b: u64) -> String {
    if b == 0 {
        return "-".into();
    }
    format!("{:.1} GB", b as f64 / 1e9)
}

/// The repos to scan: the one containing the current folder (found through
/// its git dir, so it works from inside a linked worktree too), or whatever
/// `--root` / `--all` names. `git worktree list` then finds that repo's
/// worktrees wherever they live.
fn repos(scan: &Scan) -> Result<Vec<PathBuf>, String> {
    let roots = if scan.all {
        vec![home().join("Development")]
    } else {
        scan.roots.clone()
    };
    if !roots.is_empty() {
        return Ok(roots
            .iter()
            .flat_map(|r| gather::find_repos(r, 2))
            .collect());
    }
    gather::current_repo().map(|r| vec![r]).ok_or_else(|| {
        "not inside a git repo: run it in one, or pass --root <dir> or --all".to_string()
    })
}

fn rows(scan: &Scan, repos: &[PathBuf]) -> Vec<Row> {
    let open = gather::open_paths();
    let mut found = gather::gather(repos, &open);
    for repo in repos {
        let paths: Vec<PathBuf> = found
            .iter()
            .filter(|w| w.repo == *repo)
            .map(|w| w.path.clone())
            .collect();
        if !paths.is_empty() {
            let hints = provider::hints(repo, &paths);
            gather::apply_hints(&mut found, &hints);
        }
    }
    let verdicts: Vec<Verdict> = found
        .iter()
        .map(|w| decide(&w.facts, scan.max_age))
        .collect();
    // `du` is the slow part, so measure only what the caller needs.
    let measure: Vec<usize> = (0..found.len())
        .filter(|&i| scan.sizes || verdicts[i].removable())
        .collect();
    let paths: Vec<PathBuf> = measure.iter().map(|&i| found[i].path.clone()).collect();
    let sizes = gather::du_many(&paths);
    let mut rows: Vec<Row> = found
        .into_iter()
        .zip(verdicts)
        .map(|(w, verdict)| Row {
            kind: Kind::Worktree,
            verdict,
            path: w.path.display().to_string(),
            repo: w.repo.display().to_string(),
            branch: w.branch,
            head: w.head,
            size_bytes: 0,
        })
        .collect();
    for (&i, size) in measure.iter().zip(sizes) {
        rows[i].size_bytes = size;
    }
    if scan.caches || scan.all {
        rows.extend(cache_rows(scan, &open));
    }
    rows.sort_by(|a, b| {
        b.verdict
            .removable()
            .cmp(&a.verdict.removable())
            .then(b.size_bytes.cmp(&a.size_bytes))
    });
    rows
}

fn cache_rows(scan: &Scan, open: &[String]) -> Vec<Row> {
    let mut rows = Vec::new();
    for c in cache::discover() {
        let verdict = cache::decide(
            gather::in_use(&c.path, open),
            cache::idle_secs(&c.path),
            c.max_age_days,
        );
        // `du` on a build cache can take minutes, so measure only what goes.
        let size_bytes = if scan.sizes || verdict.removable() {
            gather::du_bytes(&c.path)
        } else {
            0
        };
        rows.push(Row {
            kind: Kind::Cache,
            path: c.path.display().to_string(),
            repo: String::new(),
            branch: None,
            head: None,
            size_bytes,
            verdict,
        });
    }
    rows
}

pub(crate) fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

fn reclaimable(rows: &[Row]) -> u64 {
    rows.iter()
        .filter(|r| r.verdict.removable())
        .map(|r| r.size_bytes)
        .sum()
}

fn print_report(rows: &[Row], json: bool) {
    if json {
        let total = reclaimable(rows);
        let v = serde_json::json!({ "reclaimable_bytes": total, "items": rows });
        println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
        return;
    }
    for r in rows {
        let (tag, why) = match &r.verdict {
            Verdict::Remove(w) => ("remove", w),
            Verdict::Keep(w) => ("keep  ", w),
        };
        println!("{tag}  {:>9}  {}  ({why})", gib(r.size_bytes), r.path);
    }
    let n = rows.iter().filter(|r| r.verdict.removable()).count();
    println!(
        "\n{n} of {} items removable, {} reclaimable",
        rows.len(),
        gib(reclaimable(rows))
    );
}

fn apply(rows: &[Row], yes: bool, only: Option<Only>) -> ExitCode {
    let wanted = |r: &Row| match only {
        None => true,
        Some(Only::Worktrees) => r.kind == Kind::Worktree,
        Some(Only::Caches) => r.kind == Kind::Cache,
    };
    let todo: Vec<&Row> = rows
        .iter()
        .filter(|r| r.verdict.removable() && wanted(r))
        .collect();
    if todo.is_empty() {
        println!("nothing to remove");
        return ExitCode::SUCCESS;
    }
    for r in &todo {
        println!("remove  {:>9}  {}", gib(r.size_bytes), r.path);
    }
    let total: u64 = todo.iter().map(|r| r.size_bytes).sum();
    println!("\n{} items, {}", todo.len(), gib(total));
    if !yes {
        print!("Remove these? [y/N] ");
        let _ = std::io::stdout().flush();
        let mut a = String::new();
        if std::io::stdin().read_line(&mut a).is_err() || !a.trim().eq_ignore_ascii_case("y") {
            println!("cancelled");
            return ExitCode::FAILURE;
        }
    }
    let mut failed = 0;
    for r in todo {
        if r.kind == Kind::Cache {
            match cache::remove(Path::new(&r.path)) {
                Ok(()) => println!("removed {}", r.path),
                Err(e) => {
                    failed += 1;
                    eprintln!("failed  {}: {e}", r.path);
                }
            }
            continue;
        }
        // Record first, so every removal can be undone with `offcut restore`.
        let Some(head) = r.head.clone() else {
            failed += 1;
            eprintln!(
                "skipped {}: HEAD unknown, so it could not be restored",
                r.path
            );
            continue;
        };
        let entry = manifest::Entry {
            ts: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
            repo: r.repo.clone(),
            path: r.path.clone(),
            branch: r.branch.clone(),
            head,
            size_bytes: r.size_bytes,
        };
        if let Err(e) = manifest::append(&entry) {
            eprintln!("stopped: cannot write {}: {e}", manifest::file().display());
            return ExitCode::FAILURE;
        }
        // Never --force: git refuses if the worktree changed since the report.
        let out = Command::new("git")
            .arg("-C")
            .arg(&r.repo)
            .args(["worktree", "remove", &r.path])
            .output();
        match out {
            Ok(o) if o.status.success() => println!("removed {}", r.path),
            Ok(o) => {
                failed += 1;
                eprintln!(
                    "failed  {}: {}",
                    r.path,
                    String::from_utf8_lossy(&o.stderr).trim()
                );
            }
            Err(e) => {
                failed += 1;
                eprintln!("failed  {}: {e}", r.path);
            }
        }
    }
    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn branch_free(repo: &Path, branch: &str) -> bool {
    let exists = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .output()
        .is_ok_and(|o| o.status.success());
    let used = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .is_ok_and(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .any(|l| l == format!("branch refs/heads/{branch}"))
        });
    exists && !used
}

fn restore(paths: &[PathBuf]) -> ExitCode {
    let entries = manifest::read();
    if paths.is_empty() {
        let mut seen = Vec::new();
        for e in entries.iter().rev() {
            if seen.contains(&e.path) {
                continue;
            }
            seen.push(e.path.clone());
            let state = if Path::new(&e.path).exists() {
                "present "
            } else {
                "removed "
            };
            let at = e
                .branch
                .as_deref()
                .unwrap_or(&e.head[..e.head.len().min(10)]);
            println!("{state} {}  [{at}]", e.path);
        }
        if seen.is_empty() {
            println!("nothing recorded in {}", manifest::file().display());
        }
        return ExitCode::SUCCESS;
    }
    let mut failed = 0;
    for p in paths {
        let want = std::path::absolute(p).unwrap_or_else(|_| p.clone());
        let Some(e) = entries.iter().rev().find(|e| Path::new(&e.path) == want) else {
            failed += 1;
            eprintln!("no record of {}", want.display());
            continue;
        };
        if want.exists() {
            println!("present {}", e.path);
            continue;
        }
        let repo = Path::new(&e.repo);
        let mut cmd = Command::new("git");
        cmd.arg("-C").arg(repo).args(["worktree", "add"]);
        // Back on its branch when the branch is still there and free;
        // otherwise detached at the commit it had.
        match e.branch.as_deref().filter(|b| branch_free(repo, b)) {
            Some(b) => cmd.arg(&e.path).arg(b),
            None => cmd.arg("--detach").arg(&e.path).arg(&e.head),
        };
        match cmd.output() {
            Ok(o) if o.status.success() => println!("restored {}", e.path),
            Ok(o) => {
                failed += 1;
                eprintln!(
                    "failed  {}: {}",
                    e.path,
                    String::from_utf8_lossy(&o.stderr).trim()
                );
            }
            Err(err) => {
                failed += 1;
                eprintln!("failed  {}: {err}", e.path);
            }
        }
    }
    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let cmd = cli.cmd.unwrap_or(Cmd::Report);
    match &cmd {
        Cmd::Restore { paths } => return restore(paths),
        Cmd::Schedule { action } => {
            return match action {
                ScheduleCmd::Install => schedule::install(),
                ScheduleCmd::Remove => schedule::remove(),
                ScheduleCmd::Status => schedule::status(),
            };
        }
        _ => {}
    }
    let repos = match repos(&cli.scan) {
        Ok(r) => r,
        // Caches are machine-wide, so `--caches` works outside a repo.
        Err(_) if cli.scan.caches => Vec::new(),
        Err(e) => {
            eprintln!("offcut: {e}");
            return ExitCode::FAILURE;
        }
    };
    let rows = rows(&cli.scan, &repos);
    match cmd {
        Cmd::Apply { yes, only } => apply(&rows, yes, only),
        _ => {
            print_report(&rows, cli.scan.json);
            ExitCode::SUCCESS
        }
    }
}
