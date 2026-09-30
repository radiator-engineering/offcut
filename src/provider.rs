//! Providers: commands that say what they know about worktree paths.
//!
//! A provider reads absolute paths on stdin, one per line, and prints one
//! JSON object per line: `{"path": "...", "verdict": "hold"|"done", "reason": "..."}`.
//! `hold` protects a path. `done` means its owner has finished. No line for a
//! path means no opinion. A provider that fails, times out or prints garbage
//! holds every path: when in doubt, keep it.

use crate::decide::Hint;
use serde::Deserialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(15);

/// The command `offcut` runs for a repo that keeps an event log and has
/// `eventlog` installed, when no provider is configured.
const EVENTLOG_COMMAND: &str = "eventlog worktree-facts --json";

#[derive(Deserialize, Default)]
struct Config {
    #[serde(default, rename = "provider")]
    providers: Vec<ProviderCfg>,
}

#[derive(Deserialize)]
struct ProviderCfg {
    command: String,
}

#[derive(Deserialize)]
struct Line {
    path: PathBuf,
    verdict: String,
}

/// Provider commands for a repo: `.offcut.toml` in the repo, else
/// `~/.config/offcut/config.toml`, else the eventlog provider when the repo
/// has a log and `eventlog` is on PATH.
pub fn commands_for(repo: &Path) -> Vec<String> {
    let files = [
        repo.join(".offcut.toml"),
        crate::home().join(".config/offcut/config.toml"),
    ];
    for f in files {
        if let Ok(text) = std::fs::read_to_string(&f)
            && let Ok(cfg) = toml::from_str::<Config>(&text)
            && !cfg.providers.is_empty()
        {
            return cfg.providers.into_iter().map(|p| p.command).collect();
        }
    }
    if repo.join(".context/events.jsonl").is_file() && on_path("eventlog") {
        return vec![EVENTLOG_COMMAND.to_string()];
    }
    Vec::new()
}

fn on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
}

/// Run one provider. `None` means it failed, and the caller holds everything.
fn run(command: &str, repo: &Path, paths: &[PathBuf]) -> Option<Vec<Line>> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(repo)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdin = child.stdin.take()?;
    let input: String = paths.iter().map(|p| format!("{}\n", p.display())).collect();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(input.as_bytes());
    });
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    let start = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().ok()? {
            break s;
        }
        if start.elapsed() > TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let _ = writer.join();
    let text = reader.join().ok()?;
    if !status.success() {
        return None;
    }
    let mut out = Vec::new();
    for l in text.lines().filter(|l| !l.trim().is_empty()) {
        out.push(serde_json::from_str::<Line>(l).ok()?);
    }
    Some(out)
}

/// Combine the providers' answers for `paths`. `hold` beats `done`. A provider
/// that fails holds every path.
pub fn hints(repo: &Path, paths: &[PathBuf]) -> HashMap<PathBuf, Hint> {
    let mut out: HashMap<PathBuf, Hint> = HashMap::new();
    for command in commands_for(repo) {
        match run(&command, repo, paths) {
            None => {
                eprintln!(
                    "offcut: provider `{command}` failed; keeping every worktree of {}",
                    repo.display()
                );
                return paths.iter().map(|p| (p.clone(), Hint::Hold)).collect();
            }
            Some(lines) => {
                for l in lines {
                    let h = match l.verdict.as_str() {
                        "hold" => Hint::Hold,
                        "done" => Hint::Done,
                        _ => continue,
                    };
                    let e = out.entry(l.path).or_insert(h);
                    if h == Hint::Hold {
                        *e = Hint::Hold;
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn with(cmd: &str) -> (tempfile::TempDir, PathBuf) {
        let d = tmp();
        std::fs::write(
            d.path().join(".offcut.toml"),
            format!("[[provider]]\ncommand = {cmd:?}\n"),
        )
        .unwrap();
        let repo = d.path().to_path_buf();
        (d, repo)
    }

    #[test]
    fn a_provider_can_hold_and_finish() {
        let (_d, repo) =
            with(r#"printf '{"path":"/a","verdict":"hold"}\n{"path":"/b","verdict":"done"}\n'"#);
        let h = hints(&repo, &[p("/a"), p("/b"), p("/c")]);
        assert_eq!(h.get(&p("/a")), Some(&Hint::Hold));
        assert_eq!(h.get(&p("/b")), Some(&Hint::Done));
        assert_eq!(h.get(&p("/c")), None);
    }

    #[test]
    fn the_provider_reads_paths_on_stdin() {
        let (_d, repo) =
            with(r#"while read p; do printf '{"path":"%s","verdict":"done"}\n' "$p"; done"#);
        let h = hints(&repo, &[p("/x"), p("/y")]);
        assert_eq!(h.len(), 2);
    }

    #[test]
    fn a_failing_provider_holds_everything() {
        for cmd in ["exit 3", "echo not json", "nonexistent-command-xyz"] {
            let (_d, repo) = with(cmd);
            let h = hints(&repo, &[p("/a"), p("/b")]);
            assert_eq!(h.get(&p("/a")), Some(&Hint::Hold), "{cmd}");
            assert_eq!(h.get(&p("/b")), Some(&Hint::Hold), "{cmd}");
        }
    }

    #[test]
    fn hold_beats_done_across_providers() {
        let d = tmp();
        std::fs::write(
            d.path().join(".offcut.toml"),
            "[[provider]]\ncommand = \"echo '{\\\"path\\\":\\\"/a\\\",\\\"verdict\\\":\\\"done\\\"}'\"\n\
             [[provider]]\ncommand = \"echo '{\\\"path\\\":\\\"/a\\\",\\\"verdict\\\":\\\"hold\\\"}'\"\n",
        )
        .unwrap();
        let h = hints(d.path(), &[p("/a")]);
        assert_eq!(h.get(&p("/a")), Some(&Hint::Hold));
    }
}
