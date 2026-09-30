# offcut: design

Status: draft.

`offcut` finds the leftovers that agents and builds leave on disk, says how
much space they hold, and removes them when it is safe. It works with git
alone. It works better with an event log.

## Measured need

On the author's machine (2026-09-30), `.worktrees/` and `.claude/worktrees/`
folders held about 154 GB across six repos. Bazel's output base held about
40 GB. Worktrees come first.

## Split

| Part | Job |
|---|---|
| CLI (`offcut`) | Find, measure, decide, delete. Owns every safety rule. |
| Providers | Commands that print facts about paths. Optional. |
| Mod (later) | At `session.start`, run `offcut report --json` and print one line. Never deletes. |
| Scheduled job (later) | Run `offcut report`, then `offcut apply` once reports have looked right. |

`offcut` never writes to an event log. A provider only reads.

## Commands

```
offcut report [--root <dir>]... [--json]    # default command; deletes nothing
offcut apply  [--root <dir>]... [--only worktrees|caches] [--yes]
offcut config                               # print the effective config
```

`apply` prints the plan and asks for confirmation unless `--yes` is given.

## Candidates

**Worktrees.** For each repo under the roots, `git worktree list --porcelain`.
This covers `.worktrees/`, `.claude/worktrees/` and any other location. The
main worktree is never a candidate.

**Caches.** Directories named in config (`[[cache]] path = ...`). Examples:
Bazel's output base, `target/`, `node_modules`. A cache is a candidate when
it is over `max_size` or untouched for `max_age`.

## Safety rules

These are fixed. Config and providers cannot weaken them.

A worktree is never removed if any of these is true:

1. It has uncommitted or untracked changes.
2. It has commits that no remote branch contains.
3. Its branch is checked out anywhere else, or it is locked.
4. A process has a file open inside it (`lsof`).
5. A provider says `hold`.

A worktree is removed only when it is clean and its branch is merged into the
default branch, or it is clean, pushed, and older than `max_age` (default 14
days, by last commit and last file change).

Removal uses `git worktree remove`, then deletes the branch only if it is
merged. It never uses `--force`.

Caches are removed only when no process has a file open inside them.

## Provider protocol

A provider is a command. It reads paths on stdin, one per line, and prints
one JSON object per line:

```json
{"path": "/abs/path", "verdict": "hold" | "done", "reason": "agent api-x has an open claim"}
```

- `hold`: do not remove.
- `done`: the owner has finished. Skip the age wait. Safety rules still apply.
- No line for a path means no opinion.

A provider that fails or times out counts as `hold` for every path. When in
doubt, keep it.

Config:

```toml
[[provider]]
command = "eventlog worktree-facts --json"
```

If the repo has an eventlog and `eventlog` is on PATH, offcut uses that
provider without config.

## Eventlog side (separate change in event-log)

`eventlog worktree-facts --json` reads paths on stdin and answers from the log:

- agent retired, `result` recorded: `done`.
- agent spawned and not retired, or any open `claim` on the path: `hold`.
- path not matched to an agent: no line.

## Config

`~/.config/offcut/config.toml`, then `.offcut.toml` in a repo. Keys: `roots`,
`max_age`, `[[cache]]`, `[[provider]]`. Defaults work with no file.

## Output

`report` lists each candidate with size, age, verdict (`remove`, `keep`) and
the reason. `--json` prints the same. It ends with the total reclaimable.

## Language and release

Rust, one binary. Release with cargo-dist and a mise pin, as eventlog does.

## Open questions

- Should `apply` also run `git worktree prune` and `git gc`?
- Bazel: call `bazel clean --expunge` or delete the output base directly?
  Check first whether Bazel's own disk-cache size limit makes this unnecessary.
- Does the scheduled job ever run `apply`, or only `report`? Start with `report`.
