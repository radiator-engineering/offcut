//! `apply --detach`: start the same `apply` in its own process group and
//! return at once.
//!
//! The Claude Code mod calls this at the end of a turn or session. Claude
//! Code kills its children's process group when it exits, which would end
//! a cleanup still waiting out `--delay`. A process group of its own keeps
//! it alive. Output goes to `~/.local/state/offcut/apply.log`.

use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};

/// Set on the background copy, so it starts its log entry with [`header`].
pub const CHILD: &str = "OFFCUT_DETACHED";

/// The log is cut back to empty past this size, keeping one `.old` copy.
const LOG_MAX: u64 = 1_000_000;

pub fn log() -> PathBuf {
    crate::manifest::file().with_file_name("apply.log")
}

fn open_log() -> std::io::Result<std::fs::File> {
    let f = log();
    if let Some(dir) = f.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if std::fs::metadata(&f).is_ok_and(|m| m.len() > LOG_MAX) {
        let _ = std::fs::rename(&f, f.with_extension("log.old"));
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&f)
}

/// Re-run this command without `--detach`, in a new process group.
pub fn spawn() -> ExitCode {
    let out = match open_log() {
        Ok(f) => f,
        Err(e) => {
            eprintln!("offcut: cannot open {}: {e}", log().display());
            return ExitCode::FAILURE;
        }
    };
    let err = match out.try_clone() {
        Ok(f) => f,
        Err(e) => {
            eprintln!("offcut: {e}");
            return ExitCode::FAILURE;
        }
    };
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("offcut: cannot find its own binary: {e}");
            return ExitCode::FAILURE;
        }
    };
    let args = std::env::args_os().skip(1).filter(|a| a != "--detach");
    match Command::new(exe)
        .args(args)
        .env(CHILD, "1")
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err)
        .process_group(0)
        .spawn()
    {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("offcut: cannot start apply: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The first line of a run in the log: when, and where.
pub fn header() {
    let when = Command::new("date")
        .arg("+%F %T")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let cwd = std::env::current_dir().unwrap_or_default();
    println!("== {when} {}", cwd.display());
}
