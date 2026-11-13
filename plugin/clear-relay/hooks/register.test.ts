import { expect, mock, test } from 'claude-code/testing'
import type { Engine, On } from 'claude-code/testing'

const PROMPT = 'continue with Phase 2 of docs/plans/x.md'
const LINE = `safe to /clear; next: \`${PROMPT}\``
const STATE_DIR = '/state'
const OPTIONS = { options: { stateDir: STATE_DIR } }
const COUNTDOWN_MS = 10_000
const TWO_MINUTES_MS = 120_000

type World = {
  clock: ReturnType<typeof mock.clock>
  files: Map<string, string>
  commands: string[]
  submitted: string[]
  toasts: string[]
  session: { id: string; draft: string }
}

type Refusals = { clear?: true; submit?: true }

// What the engine does beneath the plugin: a clock the test moves, a file
// system in memory, a /clear that ends the session and starts another.
function world($test: Engine, on: On, refusals: Refusals = {}): World {
  const sessions = ['s1', 's2', 's3', 's4', 's5']
  const w: World = {
    clock: mock.clock(on, { now: 1_000_000 }),
    files: new Map(),
    commands: [],
    submitted: [],
    toasts: [],
    session: { id: sessions[0] ?? 's1', draft: '' },
  }
  on('turn.complete', (_, e) => ({ text: e.answer }))
  on('turn.start', (_, e) => ({ turnId: e.turnId }))
  on('prompt.edit', (_, e) => ({ text: e.text, cursor: e.cursor }))
  on('session.end', (_, e) => ({ sessionId: e.sessionId }))
  on('session.id', () => ({ value: w.session.id }))
  on('session.cwd', () => ({ value: '/work/repo' }))
  on('env.get', () => ({ value: '/home/person' }))
  on('prompt.read', () => ({ value: { text: w.session.draft, cursor: 0 } }))
  on('command.register', () => ({ value: { command: 'stay' } }))
  on('ui.toast', (_, e) => {
    w.toasts.push(e.text)
    return { value: undefined }
  })
  on('ui.status', () => ({ value: undefined }))
  on('fs.exists', (_, e) => ({ value: w.files.has(e.path) }))
  on('fs.read', (_, e) => ({ value: w.files.get(e.path) ?? '' }))
  on('fs.write', (_, e) => {
    w.files.set(e.path, e.text)
    return { value: undefined }
  })
  on('command.run', { command: 'clear' }, async (_, e) => {
    w.commands.push(e.command)
    if (refusals.clear) throw new Error('clear refused')
    const ended = w.session.id
    w.session.id = sessions[sessions.indexOf(ended) + 1] ?? 's9'
    await $test.session.end({
      reason: 'clear',
      sessionId: ended,
      resume: { sessionId: ended },
    } as never)
    return { text: '' }
  })
  on('prompt.submit', (_, e) => {
    if (refusals.submit) return { drop: 'refused' }
    w.submitted.push(e.text)
    return { text: e.text }
  })
  return w
}

function answer($: Engine, text: string, extra: object = {}) {
  return $.turn.complete({
    answer: text,
    reason: 'answer',
    durationMs: 1,
    isAborted: false,
    turnId: 't1',
    ...extra,
  } as never)
}

function logLines(w: World, id: string): { event: string; cause?: string; session: string; cwd: string }[] {
  const text = w.files.get(`${STATE_DIR}/clear-relay/${id}.jsonl`) ?? ''
  return text
    .split('\n')
    .filter(line => line !== '')
    .map(line => JSON.parse(line))
}

const events = (w: World, id: string) => logLines(w, id).map(l => l.event)

test('the line arms; after 10 s the relay runs clear, then submits', OPTIONS, async ($, on) => {
  const w = world($, on)
  await answer($, `Done.\n\n${LINE}\n`)
  expect(w.toasts[0]).toContain('clearing in 10 s')
  expect(w.toasts[0]).toContain(PROMPT)

  await w.clock.advance(COUNTDOWN_MS - 1)
  expect(w.commands).toEqual([])
  await w.clock.advance(1)
  expect(w.commands).toEqual(['clear'])
  expect(w.submitted).toEqual([PROMPT])
  expect(events(w, 's1')).toEqual(['arm', 'clear', 'submit'])
  expect(logLines(w, 's1')[0]?.cwd).toBe('/work/repo')
})

test('a keystroke disarms', OPTIONS, async ($, on) => {
  const w = world($, on)
  await answer($, LINE)
  await $.prompt.edit({ origin: { kind: 'composer' }, text: 'x', cursor: 1, start: 0, end: 0, inputText: 'x' })
  await w.clock.advance(COUNTDOWN_MS)
  expect(w.commands).toEqual([])
  expect(logLines(w, 's1')[1]).toMatchObject({ event: 'disarm', cause: 'keystroke' })
})

test('a non-empty prompt box at the end of the countdown disarms', OPTIONS, async ($, on) => {
  const w = world($, on)
  await answer($, LINE)
  w.session.draft = 'half a thought'
  await w.clock.advance(COUNTDOWN_MS)
  expect(w.commands).toEqual([])
  expect(logLines(w, 's1')[1]).toMatchObject({ event: 'disarm', cause: 'typing' })
})

test('a submitted prompt disarms', OPTIONS, async ($, on) => {
  const w = world($, on)
  await answer($, LINE)
  await $.prompt.submit({ text: 'something else' })
  await w.clock.advance(COUNTDOWN_MS)
  expect(w.commands).toEqual([])
  expect(logLines(w, 's1')[1]).toMatchObject({ event: 'disarm', cause: 'prompt-submitted' })
})

test('a started turn disarms', OPTIONS, async ($, on) => {
  const w = world($, on)
  await answer($, LINE)
  await $.turn.start({ text: 'go', turnId: 't2' })
  await w.clock.advance(COUNTDOWN_MS)
  expect(w.commands).toEqual([])
  expect(logLines(w, 's1')[1]).toMatchObject({ event: 'disarm', cause: 'turn-started' })
})

test('/stay disarms', OPTIONS, async ($, on) => {
  const w = world($, on)
  await answer($, LINE)
  const stayed = await $.command.run({ command: 'stay' })
  expect(stayed.text).toContain('staying')
  await w.clock.advance(COUNTDOWN_MS)
  expect(w.commands).toEqual([])
  expect(logLines(w, 's1')[1]).toMatchObject({ event: 'disarm', cause: 'stay' })
})

test('an aborted turn, a subagent turn and a refusal do not arm', OPTIONS, async ($, on) => {
  const w = world($, on)
  await answer($, LINE, { reason: 'aborted', isAborted: true })
  await answer($, LINE, { agentId: 'sub' })
  await answer($, LINE, { reason: 'refusal', refusal: { category: null, explanation: null } })
  await w.clock.advance(COUNTDOWN_MS)
  expect(w.toasts).toEqual([])
  expect(w.commands).toEqual([])
})

test('a line that is not last, an empty or a multi-line prompt do not arm', OPTIONS, async ($, on) => {
  const w = world($, on)
  await answer($, `${LINE}\nand one more thing`)
  await answer($, 'safe to /clear; next: ``')
  await answer($, 'safe to /clear; next: `first\nsecond`')
  await answer($, `safe to /clear; next: \`${'x'.repeat(2_001)}\``)
  await w.clock.advance(COUNTDOWN_MS)
  expect(w.toasts).toEqual([])
  expect(w.commands).toEqual([])
})

test('a refused clear toasts the prompt and does not submit', OPTIONS, async ($, on) => {
  const w = world($, on, { clear: true })
  await answer($, LINE)
  await w.clock.advance(COUNTDOWN_MS)
  expect(w.submitted).toEqual([])
  expect(w.toasts.at(-1)).toContain(PROMPT)
  expect(events(w, 's1')).toEqual(['arm', 'failure'])
})

test('a second line within 2 minutes does not arm', OPTIONS, async ($, on) => {
  const w = world($, on)
  await answer($, LINE)
  await w.clock.advance(COUNTDOWN_MS)
  expect(w.commands).toEqual(['clear'])

  await w.clock.advance(TWO_MINUTES_MS - COUNTDOWN_MS - 1_000)
  await answer($, LINE)
  await w.clock.advance(COUNTDOWN_MS)
  expect(w.commands).toEqual(['clear'])

  await w.clock.advance(TWO_MINUTES_MS)
  await answer($, LINE)
  await w.clock.advance(COUNTDOWN_MS)
  expect(w.commands).toEqual(['clear', 'clear'])
})

test("the relay's own submit does not disarm it", OPTIONS, async ($, on) => {
  const w = world($, on)
  await answer($, LINE)
  await w.clock.advance(COUNTDOWN_MS)
  expect(w.submitted).toEqual([PROMPT])
  expect(events(w, 's1')).toEqual(['arm', 'clear', 'submit'])
})

test("another plugin's submit during the countdown disarms", OPTIONS, async ($, on) => {
  const w = world($, on)
  await answer($, LINE)
  await $.prompt.submit({ text: 'from elsewhere', origin: { kind: 'plugin', name: 'other' } } as never)
  await w.clock.advance(COUNTDOWN_MS)
  expect(w.commands).toEqual([])
})

test('a refused submit after the clear toasts the prompt', OPTIONS, async ($, on) => {
  const w = world($, on, { submit: true })
  await answer($, LINE)
  await w.clock.advance(COUNTDOWN_MS)
  expect(w.commands).toEqual(['clear'])
  expect(w.toasts.at(-1)).toContain(PROMPT)
  expect(events(w, 's1')).toEqual(['arm', 'clear', 'failure'])
})

test('/stay works after a clear', OPTIONS, async ($, on) => {
  const w = world($, on)
  await answer($, LINE)
  await w.clock.advance(COUNTDOWN_MS)
  await w.clock.advance(TWO_MINUTES_MS)
  await answer($, LINE)
  const stayed = await $.command.run({ command: 'stay' })
  expect(stayed.text).toContain('staying')
  await w.clock.advance(COUNTDOWN_MS)
  expect(w.commands).toEqual(['clear'])
})

test('enabled false never arms', { options: { enabled: false, stateDir: STATE_DIR } }, async ($, on) => {
  const w = world($, on)
  await answer($, LINE)
  await w.clock.advance(COUNTDOWN_MS)
  expect(w.toasts).toEqual([])
  expect(w.commands).toEqual([])
})

test('a turn complete returns the answer unchanged', OPTIONS, async ($, on) => {
  world($, on)
  const result = await answer($, `text\n${LINE}`)
  expect(result.text).toBe(`text\n${LINE}`)
})

test('two sessions each keep all their log lines', OPTIONS, async ($, on) => {
  const w = world($, on)
  await answer($, LINE)
  await $.command.run({ command: 'stay' })
  w.session.id = 'other'
  await w.clock.advance(TWO_MINUTES_MS)
  await answer($, LINE)
  await $.command.run({ command: 'stay' })
  expect(events(w, 's1')).toEqual(['arm', 'disarm'])
  expect(events(w, 'other')).toEqual(['arm', 'disarm'])
})
