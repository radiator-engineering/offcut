//! `offcut mod install|remove`: the Claude Code mod that runs `offcut apply`
//! when a turn or a session ends.

use include_dir::{Dir, include_dir};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

static MOD: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/mod");

/// User level, so the mod loads in every trusted workspace.
fn target() -> PathBuf {
    crate::home().join(".claude/skills/offcut")
}

pub fn install() -> ExitCode {
    let t = target();
    if t.is_dir()
        && let Err(e) = std::fs::remove_dir_all(&t)
    {
        eprintln!("offcut: cannot replace {}: {e}", t.display());
        return ExitCode::FAILURE;
    }
    if let Err(e) = write_dir(&MOD, &t) {
        eprintln!("offcut: cannot write {}: {e}", t.display());
        return ExitCode::FAILURE;
    }
    println!("wrote {}", t.display());
    println!("New Claude Code sessions now run `offcut apply --only auto --yes` after each turn.");
    println!(
        "Function hooks must be on: CLAUDE_CODE_ENABLE_FUNCTION_HOOKS=1 (Claude Code 2.1.278+)."
    );
    println!("Turn it off for one session with OFFCUT=off.");
    ExitCode::SUCCESS
}

pub fn remove() -> ExitCode {
    let t = target();
    if !t.exists() {
        println!("not installed");
        return ExitCode::SUCCESS;
    }
    match std::fs::remove_dir_all(&t) {
        Ok(()) => {
            println!("removed {}", t.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("offcut: cannot remove {}: {e}", t.display());
            ExitCode::FAILURE
        }
    }
}

/// Write the mod's runtime files: not its tests or tsconfig.
fn write_dir(dir: &Dir<'_>, target: &Path) -> std::io::Result<()> {
    for file in dir.files() {
        if file
            .path()
            .file_name()
            .is_some_and(|n| n == "tsconfig.json")
        {
            continue;
        }
        let dest = target.join(file.path());
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&dest, file.contents())?;
    }
    for sub in dir.dirs() {
        if sub.path().file_name().is_some_and(|n| n == "tests") {
            continue;
        }
        write_dir(sub, target)?;
    }
    Ok(())
}
