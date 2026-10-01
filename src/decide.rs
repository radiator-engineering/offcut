//! The safety rules. Pure: facts in, verdict out. Nothing here touches disk.

use serde::Serialize;

pub const DAY: u64 = 86_400;

/// What a provider said about a path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Hint {
    Hold,
    Done,
}

#[derive(Clone, Debug, Default)]
pub struct Facts {
    pub is_main: bool,
    pub locked: bool,
    pub dirty: bool,
    /// Commits reachable from no remote branch.
    pub unpushed: usize,
    /// Branch is an ancestor of the default branch.
    pub merged: bool,
    /// Someone committed, rebased or merged in this worktree. A new worktree
    /// has none, and its tip is already in the default branch, so `merged`
    /// alone would call it finished the moment it was made.
    pub worked: bool,
    /// A process has a file open inside the worktree.
    pub in_use: bool,
    /// When every process holding the worktree is an orphan working in it
    /// (see `gather::orphans_holding`), their pids. Empty otherwise.
    pub orphans: Vec<u32>,
    /// Seconds since the last commit or file change, whichever is newer.
    pub idle_secs: u64,
    pub hint: Option<Hint>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "verdict", content = "reason", rename_all = "lowercase")]
pub enum Verdict {
    Remove(String),
    Keep(String),
}

impl Verdict {
    pub fn removable(&self) -> bool {
        matches!(self, Verdict::Remove(_))
    }
}

/// Decide what to do with one worktree.
///
/// The keep rules come first and cannot be overridden by a hint or by age.
/// A provider `done` only skips the age wait.
pub fn decide(f: &Facts, max_age_days: u64) -> Verdict {
    use Verdict::{Keep, Remove};
    if f.is_main {
        return Keep("main worktree".into());
    }
    if f.hint == Some(Hint::Hold) {
        return Keep("provider says hold".into());
    }
    if f.locked {
        return Keep("locked".into());
    }
    // An orphaned process may be stopped only for a worktree whose owner the
    // provider says is done; anything else in use stays.
    let stops_orphans = f.in_use && !f.orphans.is_empty() && f.hint == Some(Hint::Done);
    if f.in_use && !stops_orphans {
        return Keep("a process has files open in it".into());
    }
    if f.dirty {
        return Keep("uncommitted or untracked changes".into());
    }
    if f.unpushed > 0 {
        return Keep(format!("{} commit(s) on no remote branch", f.unpushed));
    }
    let days = f.idle_secs / DAY;
    if stops_orphans {
        return Remove(format!(
            "clean, pushed, provider says done; stops {} orphaned process(es)",
            f.orphans.len()
        ));
    }
    if f.hint == Some(Hint::Done) {
        return Remove("clean, pushed, provider says done".into());
    }
    if f.merged && f.worked {
        return Remove("clean, pushed, merged".into());
    }
    if days >= max_age_days {
        return Remove(format!("clean, pushed, idle {days} days"));
    }
    Keep(format!(
        "idle {days} days, under the {max_age_days}-day limit"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean() -> Facts {
        Facts {
            idle_secs: 30 * DAY,
            ..Facts::default()
        }
    }

    #[test]
    fn an_old_clean_pushed_worktree_goes() {
        assert!(decide(&clean(), 14).removable());
    }

    #[test]
    fn a_recent_one_stays() {
        let f = Facts {
            idle_secs: 2 * DAY,
            ..clean()
        };
        assert_eq!(
            decide(&f, 14),
            Verdict::Keep("idle 2 days, under the 14-day limit".into())
        );
    }

    #[test]
    fn dirty_unpushed_locked_in_use_and_main_always_stay() {
        for f in [
            Facts {
                dirty: true,
                ..clean()
            },
            Facts {
                unpushed: 2,
                ..clean()
            },
            Facts {
                locked: true,
                ..clean()
            },
            Facts {
                in_use: true,
                ..clean()
            },
            Facts {
                is_main: true,
                ..clean()
            },
        ] {
            assert!(!decide(&f, 14).removable(), "{f:?}");
        }
    }

    #[test]
    fn merged_work_skips_the_age_wait() {
        let f = Facts {
            merged: true,
            worked: true,
            idle_secs: 0,
            ..clean()
        };
        assert!(decide(&f, 30).removable());
    }

    #[test]
    fn a_new_worktree_is_not_merged_work() {
        // Its tip is already in main because nothing has been committed yet.
        let f = Facts {
            merged: true,
            worked: false,
            idle_secs: DAY,
            ..clean()
        };
        assert!(!decide(&f, 30).removable());
    }

    #[test]
    fn an_old_untouched_merged_worktree_still_goes_by_age() {
        let f = Facts {
            merged: true,
            worked: false,
            idle_secs: 40 * DAY,
            ..clean()
        };
        assert!(decide(&f, 30).removable());
    }

    #[test]
    fn done_skips_the_age_wait_but_not_the_safety_rules() {
        let f = Facts {
            hint: Some(Hint::Done),
            idle_secs: 0,
            ..clean()
        };
        assert!(decide(&f, 14).removable());
        for f in [
            Facts {
                hint: Some(Hint::Done),
                dirty: true,
                ..clean()
            },
            Facts {
                hint: Some(Hint::Done),
                unpushed: 1,
                ..clean()
            },
            Facts {
                hint: Some(Hint::Done),
                in_use: true,
                ..clean()
            },
            Facts {
                hint: Some(Hint::Done),
                locked: true,
                ..clean()
            },
        ] {
            assert!(!decide(&f, 14).removable(), "{f:?}");
        }
    }

    #[test]
    fn orphans_are_stopped_only_for_a_done_clean_pushed_worktree() {
        let held = Facts {
            in_use: true,
            orphans: vec![42],
            ..clean()
        };
        let done = Facts {
            hint: Some(Hint::Done),
            ..held.clone()
        };
        assert!(decide(&done, 30).removable());
        assert!(!decide(&held, 30).removable(), "no done from the provider");
        for f in [
            Facts {
                dirty: true,
                ..done.clone()
            },
            Facts {
                unpushed: 1,
                ..done.clone()
            },
            Facts {
                orphans: vec![],
                ..done.clone()
            },
        ] {
            assert!(!decide(&f, 30).removable(), "{f:?}");
        }
    }

    #[test]
    fn hold_beats_merged_and_age() {
        let f = Facts {
            hint: Some(Hint::Hold),
            merged: true,
            worked: true,
            ..clean()
        };
        assert!(!decide(&f, 14).removable());
    }
}
