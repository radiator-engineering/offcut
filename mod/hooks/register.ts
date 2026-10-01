import type { Register } from 'claude-code'

const TAG = 'offcut'
const TIMEOUT_MS = 5_000

const errText = (err: unknown): string => (err instanceof Error ? err.message : String(err))

/**
 * The command that starts a cleanup of the repo. `--detach` returns at once
 * and leaves `offcut` running in a process group of its own, so neither a
 * turn nor an exit waits on it and Claude Code's exit does not kill it.
 * `delay` gives an ending session time to exit, so its own worktree is no
 * longer in use when offcut looks. `caches` also clears unused build caches,
 * which takes longer, so only an ending session asks for it.
 */
export const launchArgv = (offcut: string, delay: number, caches: boolean): string[] => [
  offcut,
  ...(caches ? ['--caches'] : []),
  'apply',
  '--only',
  'auto',
  '--yes',
  '--detach',
  ...(delay > 0 ? ['--delay', String(delay)] : []),
]

/** The part of `$` this mod uses. */
type Host = {
  process: {
    run: (
      argv: readonly string[],
      init?: { cwd?: string; timeoutMs?: number },
    ) => Promise<{ exitCode: number; stderr: string }>
  }
  ui: { log: (text: string) => void }
}

/** Where this session cleans up. Unset means the mod is inert for it. */
type Target = { root: string; offcut: string }

/** Start a detached `offcut apply` for `t`. Never throws. */
async function launch($: Host, t: Target | undefined, delay: number, caches: boolean): Promise<void> {
  if (t === undefined) return
  try {
    const r = await $.process.run(launchArgv(t.offcut, delay, caches), { cwd: t.root, timeoutMs: TIMEOUT_MS })
    if (r.exitCode !== 0) $.ui.log(`${TAG}: cleanup not started: ${r.stderr.trim() || `exit ${r.exitCode}`}`)
  } catch (err) {
    $.ui.log(`${TAG}: cleanup not started: ${errText(err)}`)
  }
}

export const register: Register = on => {
  let target: Target | undefined

  on('session.start', async ($, e, next) => {
    const started = await next(e)
    target = undefined
    try {
      if ((await $.env.get('OFFCUT')) === 'off') return started
      const which = await $.process.run(['sh', '-c', 'command -v offcut'], { timeoutMs: TIMEOUT_MS })
      const bin = which.stdout.trim()
      if (which.exitCode !== 0 || bin === '') {
        $.ui.log(`${TAG}: inert (offcut is not on PATH)`)
        return started
      }
      // The main repo, even when the session runs inside a linked worktree.
      const git = await $.process.run(['git', 'rev-parse', '--path-format=absolute', '--git-common-dir'], {
        cwd: e.cwd,
        timeoutMs: TIMEOUT_MS,
      })
      const common = git.stdout.trim()
      if (git.exitCode !== 0 || !common.endsWith('/.git')) {
        $.ui.log(`${TAG}: inert (not in a git repo)`)
        return started
      }
      target = { root: common.slice(0, -'/.git'.length), offcut: bin }
    } catch (err) {
      $.ui.log(`${TAG}: inert (setup failed: ${errText(err)})`)
    }
    return started
  })

  // Any finished turn, the main loop's or a subagent's: a subagent that
  // worked in its own worktree has just finished with it.
  on('turn.complete', async ($, e, next) => {
    const result = await next(e)
    await launch($, target, 0, false)
    return result
  })

  on('session.end', async ($, e, next) => {
    await launch($, target, 5, true)
    return next(e)
  })
}
