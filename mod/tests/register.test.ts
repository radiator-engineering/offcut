import type { On, TurnCompleteInput } from 'claude-code'
import type { Engine } from 'claude-code/testing'
import { describe, expect, mock, test, tier } from 'claude-code/testing'

import { launchArgv } from '../hooks/register.ts'

tier('user')

const turn = (over: Partial<TurnCompleteInput> = {}): TurnCompleteInput =>
  ({ answer: 'ok', durationMs: 10, isAborted: false, turnId: 't1', reason: 'answer', ...over }) as TurnCompleteInput

type Opts = { env?: Record<string, string>; offcut?: string | null; common?: string | null; launchThrows?: boolean }

/** The world beneath the mod: `command -v`, git, and the launch. */
function world(on: On, opts: Opts = {}) {
  const launched: { argv: readonly string[]; cwd?: string }[] = []
  const logs: string[] = []
  mock.env(on, { HOME: '/home/me', ...opts.env })
  on('process.run', ($, e) => {
    const line = e.argv[2] ?? ''
    if (e.argv[0] === 'sh' && line === 'command -v offcut') {
      const bin = opts.offcut === undefined ? '/bin/offcut' : opts.offcut
      return { value: bin === null ? { exitCode: 1, stdout: '', stderr: '' } : { exitCode: 0, stdout: `${bin}\n`, stderr: '' } }
    }
    if (e.argv[0] === 'git') {
      const c = opts.common === undefined ? '/repo/.git' : opts.common
      return { value: c === null ? { exitCode: 128, stdout: '', stderr: 'not a git repository' } : { exitCode: 0, stdout: `${c}\n`, stderr: '' } }
    }
    if (opts.launchThrows) throw new Error('boom')
    launched.push({ argv: e.argv, cwd: e.init?.cwd })
    return { value: { exitCode: 0, stdout: '', stderr: '' } }
  })
  on('ui.log', ($, e) => {
    logs.push(e.text)
    return { value: undefined }
  })
  on('turn.complete', ($, e) => ({ text: e.answer }))
  on('session.start', ($, e) => ({ cwd: e.cwd }) as never)
  on('session.end', () => ({ sessionId: 's1' }) as never)
  const start = ($: Engine) => $.session.start({ surface: 'terminal', isInteractive: true, cwd: '/repo/.worktrees/a' })
  return { launched, logs, start }
}

describe('when it cleans up', () => {
  test('every finished turn starts a cleanup; the first also clears caches', async ($, on) => {
    const w = world(on)
    await w.start($)
    await $.turn.complete(turn())
    expect(w.launched).toEqual([
      { argv: ['/bin/offcut', '--caches', 'apply', '--only', 'auto', '--yes', '--detach'], cwd: '/repo' },
    ])
    await $.turn.complete(turn())
    expect(w.launched[1]?.argv).toEqual(['/bin/offcut', 'apply', '--only', 'auto', '--yes', '--detach'])
  })

  test("a subagent's finished turn starts one too", async ($, on) => {
    const w = world(on)
    await w.start($)
    await $.turn.complete(turn({ agentId: 'sub-1' } as Partial<TurnCompleteInput>))
    expect(w.launched).toHaveLength(1)
  })

  test('the end of the session starts a delayed one that also clears caches', async ($, on) => {
    const w = world(on)
    await w.start($)
    await $.session.end({ reason: 'prompt_input_exit', sessionId: 's1', resume: { id: 's1' } } as never)
    expect(w.launched).toEqual([{ argv: launchArgv('/bin/offcut', 5, true), cwd: '/repo' }])
  })
})

describe('when it stays out of the way', () => {
  test('offcut not installed', async ($, on) => {
    const w = world(on, { offcut: null })
    await w.start($)
    await $.turn.complete(turn())
    expect(w.launched).toEqual([])
    expect(w.logs).toContain('offcut: inert (offcut is not on PATH)')
  })

  test('not a git repo', async ($, on) => {
    const w = world(on, { common: null })
    await w.start($)
    await $.turn.complete(turn())
    expect(w.launched).toEqual([])
  })

  test('OFFCUT=off', async ($, on) => {
    const w = world(on, { env: { OFFCUT: 'off' } })
    await w.start($)
    await $.turn.complete(turn())
    expect(w.launched).toEqual([])
  })

  test('the turn still completes when the launch fails', async ($, on) => {
    const w = world(on, { launchThrows: true })
    await w.start($)
    const t = await $.turn.complete(turn())
    expect(t.text).toBe('ok')
    expect(w.logs.some(l => l.startsWith('offcut: cleanup not started:'))).toBe(true)
  })
})

describe('the launch command', () => {
  test('an ending session waits, then also clears caches', () => {
    expect(launchArgv('/bin/offcut', 5, true)).toEqual([
      '/bin/offcut',
      '--caches',
      'apply',
      '--only',
      'auto',
      '--yes',
      '--detach',
      '--delay',
      '5',
    ])
  })
})
