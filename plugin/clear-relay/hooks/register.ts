import type { Hook, Register, Timer } from 'claude-code'

// The handoff line an answer ends with: safe to /clear; next: `<prompt>`
const HANDOFF = /^safe to \/clear; next: `(.+)`$/

const COUNTDOWN_MS = 10_000
const CLEAR_WAIT_MS = 5_000
const MIN_GAP_MS = 120_000
const MAX_PROMPT_CHARS = 2_000
const SHOWN_PROMPT_CHARS = 30
const FAILURE_TOAST_MS = 60_000

type Engine = Parameters<Hook<'turn.complete'>>[0]

type Relay = {
  prompt: string
  sessionId: string
  cwd: string
}

type State =
  | { kind: 'idle' }
  | { kind: 'armed'; relay: Relay; timer: Timer }
  | { kind: 'firing'; relay: Relay }

type DisarmCause =
  | 'keystroke'
  | 'typing'
  | 'prompt-submitted'
  | 'turn-started'
  | 'stay'
  | 'other-ending'
  | 'cleared-by-person'

type RelayFailure =
  | { kind: 'clear-refused'; detail: string }
  | { kind: 'clear-timeout' }
  | { kind: 'submit-dropped'; detail: string }
  | { kind: 'stay-unregistered'; detail: string }
  | { kind: 'threw'; detail: string }

/** One line of the relay's log. A failure carries the prompt, so it can be pasted from the log. */
type LogEntry =
  | { event: 'arm' | 'clear' | 'submit' }
  | { event: 'disarm'; cause: DisarmCause }
  | { event: 'failure'; failure: RelayFailure; prompt: string }

/**
 * What one registration of the module holds. `register` creates it and every
 * helper takes it: the hooks compiler wants the functions `$` is passed to at
 * the top of the file, so the state travels as this argument instead.
 */
type Relayer = {
  readonly isEnabled: boolean
  readonly stateDir: string
  state: State
  lastFiredAt: number | undefined
  clearEnded: (() => void) | undefined
  // One write chain per log file: the file is read and rewritten whole, so two
  // overlapping writes would each drop the other's line.
  readonly writes: Map<string, Promise<void>>
}

/** The handoff prompt, or undefined when the answer's last line is not a handoff line. */
function handoffPrompt(answer: string): string | undefined {
  const lines = answer.split('\n').filter(line => line.trim() !== '')
  const last = lines[lines.length - 1]?.trim()
  const prompt = last === undefined ? undefined : HANDOFF.exec(last)?.[1]

  return prompt !== undefined &&
    prompt.trim() !== '' &&
    prompt.length <= MAX_PROMPT_CHARS
    ? prompt
    : undefined
}

function shown(prompt: string): string {
  return prompt.length > SHOWN_PROMPT_CHARS
    ? `${prompt.slice(0, SHOWN_PROMPT_CHARS - 1)}…`
    : prompt
}

function describe(failure: RelayFailure): string {
  switch (failure.kind) {
    case 'clear-refused':
      return `/clear was refused (${failure.detail})`
    case 'clear-timeout':
      return `the session did not end within ${CLEAR_WAIT_MS / 1000} s of /clear`
    case 'submit-dropped':
      return `the prompt was refused (${failure.detail})`
    case 'stay-unregistered':
      return `/stay was not registered (${failure.detail})`
    case 'threw':
      return `the relay failed (${failure.detail})`
  }
}

async function logFile($: Engine, relayer: Relayer, sessionId: string): Promise<string> {
  const base =
    relayer.stateDir !== ''
      ? relayer.stateDir
      : `${(await $.env.get('HOME')) ?? ''}/.local/state/amont-agent`

  return `${base}/clear-relay/${sessionId}.jsonl`
}

async function log($: Engine, relayer: Relayer, relay: Relay, entry: LogEntry): Promise<void> {
  const file = await logFile($, relayer, relay.sessionId)
  const line = JSON.stringify({
    ...entry,
    session: relay.sessionId,
    cwd: relay.cwd,
    time: await $.clock.now(),
  })
  const previous = relayer.writes.get(file) ?? Promise.resolve()
  // The log is a record: a failed write is said in the UI log and never stops the relay.
  const written = previous
    .then(async () => {
      const before = (await $.fs.exists(file)) ? await $.fs.read(file) : ''
      await $.fs.write(file, `${before}${line}\n`)
    })
    .catch(error => $.ui.log(`clear-relay: log write to ${file} failed (${String(error)})`))
  relayer.writes.set(file, written)
  await written
}

async function disarm($: Engine, relayer: Relayer, cause: DisarmCause): Promise<void> {
  if (relayer.state.kind !== 'armed') return
  const { relay, timer } = relayer.state
  timer.cancel()
  relayer.state = { kind: 'idle' }
  $.ui.status(undefined)
  await log($, relayer, relay, { event: 'disarm', cause })
}

async function fail($: Engine, relayer: Relayer, relay: Relay, failure: RelayFailure): Promise<void> {
  relayer.state = { kind: 'idle' }
  $.ui.status(undefined)
  const file = await logFile($, relayer, relay.sessionId)
  $.ui.toast(
    `clear-relay: ${describe(failure)}. Paste this to carry on (also in ${file}): ${relay.prompt}`,
    { timeoutMs: FAILURE_TOAST_MS },
  )
  await log($, relayer, relay, { event: 'failure', failure, prompt: relay.prompt })
}

function waitForClear($: Engine, relayer: Relayer): { ended: Promise<boolean>; stop: () => void } {
  let timer: Timer | undefined
  const ended = new Promise<boolean>(resolve => {
    relayer.clearEnded = () => resolve(true)
    timer = $.clock.after(CLEAR_WAIT_MS, () => resolve(false))
  })

  return {
    ended,
    stop: () => {
      timer?.cancel()
      relayer.clearEnded = undefined
    },
  }
}

async function fire($: Engine, relayer: Relayer): Promise<void> {
  if (relayer.state.kind !== 'armed') return
  const { relay } = relayer.state
  relayer.state = { kind: 'firing', relay }

  try {
    const box = await $.prompt.read()
    if (box.text.trim() !== '') {
      relayer.state = { kind: 'idle' }
      $.ui.status(undefined)
      await log($, relayer, relay, { event: 'disarm', cause: 'typing' })
      return
    }
    relayer.lastFiredAt = await $.clock.now()

    const wait = waitForClear($, relayer)
    try {
      await $.command.run({ command: 'clear' })
    } catch (error) {
      wait.stop()
      await fail($, relayer, relay, { kind: 'clear-refused', detail: String(error) })
      return
    }
    const isCleared = await wait.ended
    wait.stop()
    if (!isCleared) {
      await fail($, relayer, relay, { kind: 'clear-timeout' })
      return
    }
    await log($, relayer, relay, { event: 'clear' })
    await declareStay($, relayer, relay)

    const submitted = await $.prompt.submit({ text: relay.prompt })
    if (submitted.drop !== undefined) {
      await fail($, relayer, relay, { kind: 'submit-dropped', detail: String(submitted.drop) })
      return
    }
    relayer.state = { kind: 'idle' }
    $.ui.status(undefined)
    await log($, relayer, relay, { event: 'submit' })
  } catch (error) {
    await fail($, relayer, relay, { kind: 'threw', detail: String(error) })
  }
}

// No session.start follows a /clear, so the command is declared at every arm
// and again after a clear; declaring a name again replaces it.
async function declareStay($: Engine, relayer: Relayer, relay: Relay): Promise<void> {
  await $.command
    .register({
      name: 'stay',
      description: 'Keep this session: cancel the pending clear.',
    })
    .catch(error =>
      log($, relayer, relay, {
        event: 'failure',
        failure: { kind: 'stay-unregistered', detail: String(error) },
        prompt: relay.prompt,
      }),
    )
}

async function arm($: Engine, relayer: Relayer, prompt: string): Promise<void> {
  const box = await $.prompt.read()
  if (box.text.trim() !== '') return
  const now = await $.clock.now()
  if (relayer.lastFiredAt !== undefined && now - relayer.lastFiredAt < MIN_GAP_MS) return

  const relay: Relay = {
    prompt,
    sessionId: await $.session.id(),
    cwd: await $.session.cwd(),
  }
  // The way out first, so a cut line still says how to keep the session.
  const text = `clear within 10 s (type or /stay to keep), then: ${shown(prompt)}`
  const timer = $.clock.after(COUNTDOWN_MS, () => {
    void fire($, relayer)
  })
  relayer.state = { kind: 'armed', relay, timer }
  $.ui.toast(text, { timeoutMs: COUNTDOWN_MS })
  $.ui.status(text)
  await log($, relayer, relay, { event: 'arm' })

  await declareStay($, relayer, relay)
}

export const register: Register = (on, options) => {
  const relayer: Relayer = {
    // plugin.json types `enabled` as a boolean, and the engine refuses any other value.
    isEnabled: options.enabled !== false,
    stateDir: typeof options.stateDir === 'string' ? options.stateDir : '',
    state: { kind: 'idle' },
    lastFiredAt: undefined,
    clearEnded: undefined,
    writes: new Map(),
  }

  on('turn.complete', async ($, e, next) => {
    const completion = await next(e)
    if (e.agentId !== undefined || relayer.state.kind === 'firing') return completion

    try {
      const prompt = e.reason === 'answer' ? handoffPrompt(e.answer) : undefined
      if (prompt === undefined) {
        await disarm($, relayer, 'other-ending')
      } else if (relayer.isEnabled && relayer.state.kind === 'idle') {
        await arm($, relayer, prompt)
      }
    } catch (error) {
      $.ui.log(`clear-relay: ${String(error)}`) // the answer stands whatever the relay does
    }

    return completion
  })

  on('prompt.edit', async ($, e, next) => {
    if (relayer.state.kind === 'armed') await disarm($, relayer, 'keystroke')

    return next(e)
  })

  on('prompt.submit', async ($, e, next) => {
    // The relay submits only while firing, which no disarm reaches; the origin
    // check also covers its own submit arriving after the state moved on.
    const isOwn = e.origin?.kind === 'plugin' && e.origin.name === $.plugin.name
    if (!isOwn && relayer.state.kind === 'armed') await disarm($, relayer, 'prompt-submitted')

    return next(e)
  })

  on('turn.start', async ($, e, next) => {
    if (relayer.state.kind === 'armed') await disarm($, relayer, 'turn-started')

    return next(e)
  })

  on('command.run', { command: 'stay' }, async $ => {
    if (relayer.state.kind === 'armed') {
      await disarm($, relayer, 'stay')

      return { text: 'clear-relay: staying in this session.' }
    }

    return { text: 'clear-relay: nothing to cancel.' }
  })

  on('session.end', async ($, e, next) => {
    if (e.reason === 'clear') {
      if (relayer.state.kind === 'armed') await disarm($, relayer, 'cleared-by-person')
      else relayer.clearEnded?.()
    }

    return next(e)
  })
}
