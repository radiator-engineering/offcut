# offcut

Find and safely remove the git worktrees that agents and builds leave behind.

Agents create a worktree per task and rarely remove it. Each one can hold
gigabytes of build output. `offcut` lists them, says which are safe to remove,
and removes them only when you ask. Every removal is recorded, so you can undo it.

It works with git alone. It works better with an [eventlog](https://github.com/radiator-engineering/eventlog),
which knows which worktrees belong to finished work.

## Install

```sh
cargo install --git ssh://git@github.com/radiator-engineering/offcut
```

## Use

Run it inside the repo you want to tidy. It finds that repo's worktrees
wherever they live (`.worktrees/`, `.claude/worktrees/`, a sibling folder).

```sh
offcut                      # same as `offcut report`; deletes nothing
offcut apply                # show the plan, ask, then remove
offcut restore              # list what `apply` removed
offcut restore <path>       # put one back
```

| Flag | Meaning |
|---|---|
| `--root <dir>` | Scan the repos under a folder instead. Repeatable. |
| `--all` | Scan everything under `~/Development`. |
| `--max-age <days>` | Idle days before a clean, pushed worktree goes. Default 30. |
| `--sizes` | Measure every worktree, not only the removable ones. Slower. |
| `--json` | Print JSON. |
| `apply --yes` | Skip the confirmation. |

## What it will and won't remove

It never removes a worktree that:

- has uncommitted or untracked changes,
- has commits that no remote branch contains,
- is locked,
- has a process with a file open in it, or
- a provider says to hold.

It removes a worktree that passes all of those and is either merged into the
default branch or idle for `--max-age` days. It runs `git worktree remove`
without `--force`, and it does not delete branches.

Files that git ignores (build output, dependencies, `.env`) are not in git.
They are gone after a removal, and `restore` cannot bring them back.

## Providers

A provider is a command that tells `offcut` what it knows about paths. It reads
absolute paths on stdin and prints one JSON line per path it has an opinion on:

```json
{"path": "/abs/path", "verdict": "hold", "reason": "agent api has an open claim"}
```

`hold` protects a path. `done` says its owner has finished, which skips the idle
wait; the safety rules above still apply. A provider that fails or times out
holds every path.

Configure one in `.offcut.toml` in the repo, or `~/.config/offcut/config.toml`:

```toml
[[provider]]
command = "eventlog worktree-facts --json"
```

If the repo has an event log and `eventlog` is on PATH, `offcut` uses that
provider with no config.

See [docs/design.md](docs/design.md) for the design and what is not built yet.
