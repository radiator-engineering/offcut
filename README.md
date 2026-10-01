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
offcut --caches report      # also list unused build caches (Bazel output bases)
offcut schedule install     # keep the machine clean in the background
```

| Flag | Meaning |
|---|---|
| `--root <dir>` | Scan the repos under a folder instead. Repeatable. |
| `--all` | Scan everything under `~/Development`, and the build caches. |
| `--caches` | Also look at build caches. Works outside a repo. |
| `apply --only worktrees\|caches` | Remove only one kind of item. |
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

## Build caches

Bazel keeps one output base per workspace under `/private/var/tmp/_bazel_<user>/`.
They grow without limit. `offcut` lists each one and removes it when no process
has a file open in it (a running Bazel server holds files open) and nothing in
it has changed for 7 days. Caches rebuild, so removal is not recorded.

To manage other cache folders, add them to `~/.config/offcut/config.toml`.
A path ending in `/*` means each folder inside it. This replaces the Bazel rule:

```toml
[[cache]]
path = "/private/var/tmp/_bazel_me/*"
max_age_days = 7

[[cache]]
path = "~/Library/Caches/some-tool"
max_age_days = 30
```

## Keeping it clean

```sh
offcut schedule install     # macOS launchd job, every 6 hours
offcut schedule status
offcut schedule remove
```

The job removes unused build caches and saves a report of everything else to
`~/.local/state/offcut/last-report.json`. It never removes a worktree. Worktrees
are for a person to review: run `offcut`, read the list, then `offcut apply`.

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
