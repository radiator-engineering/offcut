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
| Claude Code mod (`mod/`) | When a turn or session ends, start `offcut apply --only auto --yes --detach`. Holds no rules of its own. |

`offcut` never writes to an event log. A provider only reads.

## Commands

```
offcut report [--root <dir>]... [--all] [--sizes] [--json]   # default; deletes nothing
offcut apply  [--root <dir>]... [--only worktrees|caches] [--yes]
offcut restore [<path>]...                  # undo removals; no path lists them
offcut mod install|remove                   # the Claude Code mod
offcut config                               # print the effective config (not built yet)
```

**Scope.** With no flag, `offcut` works on the repo you are in, found through
its git dir, so it also works from inside a linked worktree. `git worktree
list` then finds that repo's worktrees wherever they live. `--root <dir>`
scans the repos under a folder. `--all` scans `~/Development`. Outside a repo
with no flag, it says so and does nothing.

Sizes come from `du`, the slowest step, so `report` measures only removable
worktrees unless `--sizes` is given.

`apply` prints the plan and asks for confirmation unless `--yes` is given.

Every removal is undoable. Before git removes a worktree, `apply` appends its
repo, path, branch and commit to `~/.local/state/offcut/removed.jsonl`. If
that write fails, `apply` stops. `restore` re-adds the worktree on its branch,
or detached at the recorded commit when the branch is gone or in use. Files
git ignores (build output, dependencies, `.env`) do not come back.

## Cleaning up as you go

Cleanup happens when work ends, not on a timer. `offcut mod install` writes a
Claude Code mod to `~/.claude/skills/offcut/`. It loads in every session and:

- at `session.start`, finds the main repo of the session's folder and
  `offcut` on PATH. Without either, it does nothing for that session.
- at every `turn.complete`, the main agent's or a subagent's, starts
  `offcut apply --only auto --yes --detach` in the main repo.
- at `session.end`, starts the same with `--caches --delay 5`. The delay lets
  the session exit first, so its own worktree is no longer in use when offcut
  looks.

`--only auto` removes unused caches and finished worktrees only: merged with
commits made in them, or a provider says done. A worktree that is only idle
waits for a person to run `offcut apply`.

`--detach` starts the run in a process group of its own and returns at once,
so a turn never waits on it. Claude Code kills its children's process group
when it exits, and without its own group the session-end run would die
during its delay. Output goes to `~/.local/state/offcut/apply.log`.

Runs can overlap: several sessions, one run per turn. `apply` takes a lock
(`~/.local/state/offcut/apply.lock`), waits up to 60 s for it, then skips. A
lock whose process is dead is taken over. `OFFCUT=off` turns the mod off for
one session.

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
default branch and a commit, rebase or merge was made in it (a new worktree
has none, and its tip is already in the default branch), or it is clean, pushed, and older than `max_age` (default 30
days, by the newest of the last commit, the index and the folder).

Removal uses `git worktree remove`, then deletes the branch only if it is
merged. It never uses `--force`.

Caches are removed only when no process has a file open inside them.

### Why idle is not the same as finished

On 2026-09-30 a hand-run version of these rules removed 84 clean, pushed
worktrees. Some were in use: an idle worktree the owner planned to return to
looks the same as a finished one. Nothing committed was lost, but ignored
files were. That is why the default wait is 30 days, why every removal is
recorded for `restore`, and why a provider's `done` is the only fast path
other than a merged branch.

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

## Built so far

`report`, `apply` and `restore` for worktrees; providers, including the
eventlog one; Bazel output bases and configured cache folders; the Claude
Code mod with `apply --detach`, `--delay` and the apply lock.

A launchd job that ran every 6 hours was built and then removed: cleanup
belongs at the end of the work, not on a timer.

Not built: `offcut config`, `[[cache]] max_size`.

## Open questions

- Should `apply` also run `git worktree prune` and `git gc`?
- Bazel: call `bazel clean --expunge` or delete the output base directly?
  Check first whether Bazel's own disk-cache size limit makes this unnecessary.
