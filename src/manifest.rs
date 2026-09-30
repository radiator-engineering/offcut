//! A record of every removal, so any of them can be undone.
//!
//! One JSON line per removed worktree, appended before git removes it:
//! `~/.local/state/offcut/removed.jsonl` (or `$XDG_STATE_HOME/offcut/`).

use serde::{Deserialize, Serialize};
use std::io::{BufRead, Write};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub ts: u64,
    pub repo: String,
    pub path: String,
    pub branch: Option<String>,
    pub head: String,
    pub size_bytes: u64,
}

pub fn file() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::home().join(".local/state"));
    base.join("offcut/removed.jsonl")
}

pub fn append(e: &Entry) -> std::io::Result<()> {
    let f = file();
    if let Some(dir) = f.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&f)?;
    writeln!(out, "{}", serde_json::to_string(e)?)?;
    out.sync_all()
}

pub fn read() -> Vec<Entry> {
    let Ok(f) = std::fs::File::open(file()) else {
        return Vec::new();
    };
    std::io::BufReader::new(f)
        .lines()
        .map_while(Result::ok)
        .filter_map(|l| serde_json::from_str(&l).ok())
        .collect()
}
