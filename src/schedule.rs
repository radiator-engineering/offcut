//! `offcut schedule`: a macOS launchd job that keeps the machine clean.
//!
//! Every six hours it removes unused build caches (they rebuild) and saves a
//! report of everything else to `~/.local/state/offcut/last-report.json`. It
//! never removes a worktree: those are for a person to review.

use std::path::PathBuf;
use std::process::{Command, ExitCode};

const LABEL: &str = "live.radiator.offcut";
const EVERY_SECS: u32 = 6 * 60 * 60;

fn plist_path() -> PathBuf {
    crate::home().join(format!("Library/LaunchAgents/{LABEL}.plist"))
}

pub fn state_dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::home().join(".local/state"))
        .join("offcut")
}

fn uid() -> String {
    Command::new("id")
        .arg("-u")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// The shell line the job runs. Absolute paths, because launchd has a bare PATH.
fn job_script(bin: &str, dir: &str) -> String {
    format!(
        "export PATH=\"$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin\"; \
         mkdir -p '{dir}'; \
         '{bin}' --caches apply --only caches --yes >> '{dir}/job.log' 2>&1; \
         '{bin}' --all report --json > '{dir}/last-report.json.tmp' 2>>'{dir}/job.log' \
         && mv '{dir}/last-report.json.tmp' '{dir}/last-report.json'"
    )
}

fn plist(bin: &str, dir: &str) -> String {
    let script = job_script(bin, dir)
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array><string>/bin/sh</string><string>-c</string><string>{script}</string></array>
  <key>StartInterval</key><integer>{EVERY_SECS}</integer>
  <key>RunAtLoad</key><true/>
</dict>
</plist>
"#
    )
}

pub fn install() -> ExitCode {
    let Ok(exe) = std::env::current_exe() else {
        eprintln!("offcut: cannot find my own path");
        return ExitCode::FAILURE;
    };
    let dir = state_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = plist_path();
    if let Some(p) = path.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    let body = plist(&exe.display().to_string(), &dir.display().to_string());
    if let Err(e) = std::fs::write(&path, body) {
        eprintln!("offcut: cannot write {}: {e}", path.display());
        return ExitCode::FAILURE;
    }
    let domain = format!("gui/{}", uid());
    // Replace any earlier copy, then load this one.
    let _ = Command::new("launchctl")
        .args(["bootout", &format!("{domain}/{LABEL}")])
        .output();
    match Command::new("launchctl")
        .args(["bootstrap", &domain])
        .arg(&path)
        .output()
    {
        Ok(o) if o.status.success() => {
            println!("installed {}", path.display());
            println!("runs every 6 hours; removes unused build caches, saves a report");
            println!("report: {}/last-report.json", dir.display());
            println!("log:    {}/job.log", dir.display());
            ExitCode::SUCCESS
        }
        Ok(o) => {
            eprintln!(
                "offcut: launchctl failed: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            );
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("offcut: cannot run launchctl: {e}");
            ExitCode::FAILURE
        }
    }
}

pub fn remove() -> ExitCode {
    let domain = format!("gui/{}", uid());
    let _ = Command::new("launchctl")
        .args(["bootout", &format!("{domain}/{LABEL}")])
        .output();
    match std::fs::remove_file(plist_path()) {
        Ok(()) => println!("removed the scheduled job"),
        Err(_) => println!("no scheduled job was installed"),
    }
    ExitCode::SUCCESS
}

pub fn status() -> ExitCode {
    let installed = plist_path().is_file();
    let loaded = Command::new("launchctl")
        .args(["print", &format!("gui/{}/{LABEL}", uid())])
        .output()
        .is_ok_and(|o| o.status.success());
    println!("installed: {installed}\nloaded:    {loaded}");
    let report = state_dir().join("last-report.json");
    match std::fs::metadata(&report).and_then(|m| m.modified()) {
        Ok(t) => {
            let age = std::time::SystemTime::now()
                .duration_since(t)
                .map_or(0, |d| d.as_secs());
            println!(
                "last report: {} minutes ago ({})",
                age / 60,
                report.display()
            );
        }
        Err(_) => println!("last report: none yet"),
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_job_never_applies_to_worktrees() {
        let s = job_script("/bin/offcut", "/state");
        assert!(s.contains("apply --only caches --yes"));
        assert_eq!(s.matches("apply").count(), 1);
    }

    #[test]
    fn the_job_is_not_throttled() {
        // Background priority made one scan take over ten minutes instead of one.
        let p = plist("/bin/offcut", "/state");
        assert!(!p.contains("Nice") && !p.contains("LowPriorityIO") && !p.contains("Background"));
    }

    #[test]
    fn the_plist_escapes_the_shell_line() {
        let p = plist("/bin/offcut", "/state");
        assert!(p.contains("&amp;&amp;"));
        assert!(!p.contains(" && "));
        assert!(p.contains(LABEL));
    }
}
