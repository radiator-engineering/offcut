//! Caches that build tools leave behind, such as Bazel output bases.
//!
//! A cache folder is removable when no process has a file open inside it and
//! nothing in it has changed for `max_age_days`. A running Bazel server holds
//! files open, so the open-files rule protects a cache that is in use.

use crate::decide::{DAY, Verdict};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Idle days before a built-in cache goes.
const BUILTIN_MAX_AGE_DAYS: u64 = 7;

#[derive(Clone, Debug)]
pub struct Cache {
    pub path: PathBuf,
    pub max_age_days: u64,
}

#[derive(Deserialize, Default)]
struct Config {
    #[serde(default, rename = "cache")]
    caches: Vec<CacheCfg>,
}

#[derive(Deserialize)]
struct CacheCfg {
    /// A folder, or `<folder>/*` for each folder inside it. `~` is the home folder.
    path: String,
    max_age_days: Option<u64>,
}

fn expand(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => crate::home().join(rest),
        None => PathBuf::from(p),
    }
}

fn children(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut v: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    v.sort();
    v
}

/// Bazel's output bases: one folder per workspace, named by a 32-character
/// hex hash, under `/private/var/tmp/_bazel_<user>` (macOS) or `/tmp/_bazel_<user>`.
fn bazel_output_bases() -> Vec<Cache> {
    let user = std::env::var("USER").unwrap_or_default();
    if user.is_empty() {
        return Vec::new();
    }
    ["/private/var/tmp", "/var/tmp", "/tmp"]
        .iter()
        .flat_map(|root| children(&Path::new(root).join(format!("_bazel_{user}"))))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.len() == 32 && n.bytes().all(|b| b.is_ascii_hexdigit()))
        })
        .map(|path| Cache {
            path,
            max_age_days: BUILTIN_MAX_AGE_DAYS,
        })
        .collect()
}

/// Drop repeats: on macOS `/var/tmp` is a link to `/private/var/tmp`, so one
/// folder can be found twice under two names.
fn dedup(caches: Vec<Cache>) -> Vec<Cache> {
    let mut seen = std::collections::HashSet::new();
    caches
        .into_iter()
        .filter_map(|c| {
            let real = std::fs::canonicalize(&c.path).ok()?;
            seen.insert(real.clone())
                .then_some(Cache { path: real, ..c })
        })
        .collect()
}

/// Caches from `~/.config/offcut/config.toml`, else the built-in Bazel rule.
pub fn discover() -> Vec<Cache> {
    let file = crate::home().join(".config/offcut/config.toml");
    if let Ok(text) = std::fs::read_to_string(&file)
        && let Ok(cfg) = toml::from_str::<Config>(&text)
        && !cfg.caches.is_empty()
    {
        return dedup(
            cfg.caches
                .into_iter()
                .flat_map(|c| {
                    let age = c.max_age_days.unwrap_or(BUILTIN_MAX_AGE_DAYS);
                    let dirs = match c.path.strip_suffix("/*") {
                        Some(base) => children(&expand(base)),
                        None => vec![expand(&c.path)],
                    };
                    dirs.into_iter()
                        .filter(|d| d.is_dir())
                        .map(move |path| Cache {
                            path,
                            max_age_days: age,
                        })
                })
                .collect(),
        );
    }
    dedup(bazel_output_bases())
}

fn mtime(p: &Path) -> u64 {
    std::fs::metadata(p)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs())
}

/// Seconds since the newest change to the folder or anything directly in it.
/// Bazel rewrites `command.log` on every command, so this tracks its use.
pub fn idle_secs(dir: &Path) -> u64 {
    let newest = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| mtime(&e.path()))
        .chain([mtime(dir)])
        .max()
        .unwrap_or(0);
    crate::gather::now().saturating_sub(newest)
}

/// The signature every `CACHEDIR.TAG` starts with
/// (<https://bford.info/cachedir/>). Cargo writes one in `target/`.
const CACHEDIR_SIGNATURE: &str = "Signature: 8a477f597d28d172789f06886806bc55";

/// Build output folders inside a worktree: folders holding a valid
/// `CACHEDIR.TAG`, at most three levels down. Does not look inside `.git`,
/// `node_modules`,
/// inside a tagged folder, or through symbolic links.
pub fn tagged_in(worktree: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        let tagged = std::fs::read_to_string(dir.join("CACHEDIR.TAG"))
            .is_ok_and(|t| t.starts_with(CACHEDIR_SIGNATURE));
        if tagged {
            out.push(dir.to_path_buf());
            return;
        }
        if depth == 0 {
            return;
        }
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() && e.file_name() != ".git" && e.file_name() != "node_modules" {
                walk(&e.path(), depth - 1, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(worktree, 3, &mut out);
    out.retain(|p| p != worktree);
    out
}

/// Like [`idle_secs`], one level deeper: a build touches `target/debug/*`
/// without always touching `target/` or `target/debug`.
pub fn idle_secs_two_deep(dir: &Path) -> u64 {
    let children: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .collect();
    let newest = children
        .iter()
        .map(|c| crate::gather::now().saturating_sub(idle_secs(c)))
        .chain([mtime(dir)])
        .max()
        .unwrap_or(0);
    crate::gather::now().saturating_sub(newest)
}

/// A Bazel output base whose workspace folder no longer exists: its
/// worktree was removed, so nothing can build into it again. Bazel names the
/// workspace in `DO_NOT_BUILD_HERE`. No such file, or an unreadable one,
/// means not orphaned.
pub fn orphaned(path: &Path) -> bool {
    std::fs::read_to_string(path.join("DO_NOT_BUILD_HERE"))
        .ok()
        .map(|s| s.trim().to_string())
        .is_some_and(|w| w.starts_with('/') && !Path::new(&w).exists())
}

pub fn decide(in_use: bool, orphaned: bool, idle_secs: u64, max_age_days: u64) -> Verdict {
    if in_use {
        return Verdict::Keep("a process has files open in it".into());
    }
    if orphaned {
        return Verdict::Remove("its workspace no longer exists".into());
    }
    let days = idle_secs / DAY;
    if days >= max_age_days {
        Verdict::Remove(format!("cache unused for {days} days"))
    } else {
        Verdict::Keep(format!(
            "used {days} days ago, under the {max_age_days}-day limit"
        ))
    }
}

/// Remove a cache folder. Build tools leave read-only files, so make the tree
/// writable first. Refuses anything too close to the filesystem root.
pub fn remove(path: &Path) -> std::io::Result<()> {
    if path.components().count() < 4 || path == crate::home() {
        return Err(std::io::Error::other(
            "refusing to remove a path this short",
        ));
    }
    let _ = std::process::Command::new("chmod")
        .args(["-R", "u+w"])
        .arg(path)
        .status();
    std::fs::remove_dir_all(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_old_unused_cache_goes() {
        assert!(decide(false, false, 10 * DAY, 7).removable());
    }

    #[test]
    fn a_recent_cache_stays() {
        assert!(!decide(false, false, 2 * DAY, 7).removable());
    }

    #[test]
    fn a_cache_in_use_stays_however_old() {
        assert!(!decide(true, true, 400 * DAY, 7).removable());
    }

    #[test]
    fn a_cache_whose_workspace_is_gone_goes_at_once() {
        assert!(decide(false, true, 0, 7).removable());
    }

    #[test]
    fn orphaned_reads_the_workspace_bazel_recorded() {
        let d = tempfile::tempdir().unwrap();
        let base = d.path().join("base");
        std::fs::create_dir(&base).unwrap();
        assert!(!orphaned(&base), "no marker file");
        std::fs::write(
            base.join("DO_NOT_BUILD_HERE"),
            d.path().display().to_string(),
        )
        .unwrap();
        assert!(!orphaned(&base), "workspace exists");
        std::fs::write(
            base.join("DO_NOT_BUILD_HERE"),
            d.path().join("gone").display().to_string(),
        )
        .unwrap();
        assert!(orphaned(&base));
    }

    #[test]
    fn build_output_is_found_by_its_cachedir_tag() {
        let d = tempfile::tempdir().unwrap();
        let w = d.path();
        let tag = |p: &Path, text: &str| {
            std::fs::create_dir_all(p).unwrap();
            std::fs::write(p.join("CACHEDIR.TAG"), text).unwrap();
        };
        tag(
            &w.join("target"),
            "Signature: 8a477f597d28d172789f06886806bc55\n# cargo",
        );
        tag(
            &w.join("crates/x/target"),
            "Signature: 8a477f597d28d172789f06886806bc55",
        );
        tag(
            &w.join("target/inner"),
            "Signature: 8a477f597d28d172789f06886806bc55",
        );
        tag(&w.join("fake"), "not a signature");
        tag(
            &w.join(".git/t"),
            "Signature: 8a477f597d28d172789f06886806bc55",
        );
        let mut found = tagged_in(w);
        found.sort();
        assert_eq!(found, vec![w.join("crates/x/target"), w.join("target")]);
    }

    #[test]
    fn a_folder_found_under_two_names_is_listed_once() {
        let d = tempfile::tempdir().unwrap();
        let real = d.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = d.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let c = |p: &Path| Cache {
            path: p.to_path_buf(),
            max_age_days: 7,
        };
        assert_eq!(dedup(vec![c(&real), c(&link)]).len(), 1);
    }

    #[test]
    fn remove_refuses_short_paths() {
        assert!(remove(Path::new("/tmp")).is_err());
        assert!(remove(&crate::home()).is_err());
    }
}
