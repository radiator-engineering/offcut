//! One `offcut apply` at a time on this machine.
//!
//! The Claude Code mod starts an `apply` after every turn, so runs from
//! several sessions can overlap. A second run waits for the first, up to
//! [`WAIT`], then skips: the next turn starts another one.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

/// How long a second `apply` waits for the first to finish.
const WAIT: Duration = Duration::from_secs(60);

/// A lock dir with no readable pid this old is left over from a crash.
const UNREADABLE_STALE: Duration = Duration::from_secs(600);

pub struct Lock(PathBuf);

fn dir() -> PathBuf {
    crate::manifest::file().with_file_name("apply.lock")
}

fn alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// The lock dir is stale when its pid is dead, or when it has no readable
/// pid and is older than [`UNREADABLE_STALE`].
fn stale(d: &std::path::Path) -> bool {
    match std::fs::read_to_string(d.join("pid"))
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
    {
        Some(pid) => !alive(pid),
        None => std::fs::metadata(d)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > UNREADABLE_STALE),
    }
}

impl Lock {
    /// Take the lock, waiting up to [`WAIT`]. `Ok(None)` means another live
    /// `apply` still holds it.
    pub fn take() -> std::io::Result<Option<Lock>> {
        let start = std::time::Instant::now();
        loop {
            if let Some(l) = Self::take_at(dir())? {
                return Ok(Some(l));
            }
            if start.elapsed() > WAIT {
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    fn take_at(d: PathBuf) -> std::io::Result<Option<Lock>> {
        if let Some(parent) = d.parent() {
            std::fs::create_dir_all(parent)?;
        }
        for _ in 0..2 {
            match std::fs::create_dir(&d) {
                Ok(()) => {
                    std::fs::write(d.join("pid"), std::process::id().to_string())?;
                    return Ok(Some(Lock(d)));
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if !stale(&d) {
                        return Ok(None);
                    }
                    let _ = std::fs::remove_dir_all(&d);
                }
                Err(e) => return Err(e),
            }
        }
        Ok(None)
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_taker_is_refused_until_the_first_lets_go() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("apply.lock");
        let first = Lock::take_at(d.clone()).unwrap();
        assert!(first.is_some());
        assert!(Lock::take_at(d.clone()).unwrap().is_none());
        drop(first);
        assert!(Lock::take_at(d).unwrap().is_some());
    }

    #[test]
    fn a_dead_holder_does_not_block() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("apply.lock");
        std::fs::create_dir(&d).unwrap();
        // Far above any real pid on macOS or Linux.
        std::fs::write(d.join("pid"), "999999999").unwrap();
        assert!(Lock::take_at(d).unwrap().is_some());
    }

    #[test]
    fn a_fresh_lock_with_no_pid_yet_still_blocks() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("apply.lock");
        std::fs::create_dir(&d).unwrap();
        assert!(Lock::take_at(d).unwrap().is_none());
    }
}
