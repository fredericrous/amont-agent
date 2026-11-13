import type { Hook, Register, Timer } from 'claude-code'

// The handoff line an answer ends with: safe to /clear; next: `<prompt>`
const HANDOFF = /^safe to \/clear; next: `(.+)`$/

const COUNTDOWN_MS = 10_000
const CLEAR_WAIT_MS = 5_000
const MIN_GAP_MS = 120_000
const MAX_PROMPT_CHARS = 2_000
const SHOWN_PROMPT_CHARS = 60
const FAILURE_TOAST_MS = 60_000

type Engine = Parameters<Hook<'turn.complete'>>[0]

type RelayEvent = 'arm' | 'disarm' | 'clear' | 'submit' | 'failure'

type Relay = {
  prompt: string
  sessionId: string
  cwd: string
}

type State =
  | { kind: 'idle' }
  | { kind: 'armed'; relay: Relay; timer: Timer }
  | { kind: 'firing'; relay: Relay }

/** The prompt of a handoff line, or undefined when the answer's last line is not one. */
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

type Settings = { isEnabled: boolean; stateDir: string }

let settings: Settings = { isEnabled: true, stateDir: '' }
let state: State = { kind: 'idle' }
let lastFiredAt: number | undefined
let clearEnded: (() => void) | undefined
// One write chain per log file: the file is read and rewritten whole, so two
// overlapping writes would each drop the other's line.
const writes = new Map<string, Promise<void>>()



async function logFile($: Engine, sessionId: string): Promise<string> {
  const base =
    settings.stateDir !== ''
      ? settings.stateDir
      : `${(await $.env.get('HOME')) ?? ''}/.local/state/amont-agent`

  return `${base}/clear-relay/${sessionId}.jsonl`
}

async function log(
  $: Engine,
  relay: Relay,
  event: RelayEvent,
  cause?: string,
): Promise<void> {
  const file = await logFile($, relay.sessionId)
  const line = JSON.stringify({
    event,
    session: relay.sessionId,
    cwd: relay.cwd,
    time: await $.clock.now(),
    ...(cause === undefined ? {} : { cause }),
  })
  const previous = writes.get(file) ?? Promise.resolve()
  const written = previous
    .then(async () => {
      const before = (await $.fs.exists(file)) ? await $.fs.read(file) : ''
      await $.fs.write(file, `${before}${line}\n`)
    })
    .catch(() => undefined) // the log is a record; failing to write it must not stop the relay
  writes.set(file, written)
  await written
}

async function disarm($: Engine, cause: string): Promise<void> {
  if (state.kind !== 'armed') return
  const { relay, timer } = state
  timer.cancel()
  state = { kind: 'idle' }
  $.ui.status(undefined)
  await log($, relay, 'disarm', cause)
}

async function fail($: Engine, relay: Relay, what: string): Promise<void> {
  state = { kind: 'idle' }
  $.ui.status(undefined)
  $.ui.toast(
    `clear-relay: ${what}. Paste this to carry on: ${relay.prompt}`,
    { timeoutMs: FAILURE_TOAST_MS },
  )
  await log($, relay, 'failure', what)
}

function waitForClear($: Engine): { ended: Promise<boolean>; stop: () => void } {
  let timer: Timer | undefined
  const ended = new Promise<boolean>(resolve => {
    clearEnded = () => resolve(true)
    timer = $.clock.after(CLEAR_WAIT_MS, () => resolve(false))
  })

  return {
    ended,
    stop: () => {
      timer?.cancel()
      clearEnded = undefined
    },
  }
}

async function fire($: Engine): Promise<void> {
  if (state.kind !== 'armed') return
  const { relay } = state
  state = { kind: 'firing', relay }

  try {
    const box = await $.prompt.read()
    if (box.text.trim() !== '') {
      state = { kind: 'idle' }
      $.ui.status(undefined)
      await log($, relay, 'disarm', 'typing')
      return
    }
    lastFiredAt = await $.clock.now()

    const wait = waitForClear($)
    try {
      await $.command.run({ command: 'clear' })
    } catch (error) {
      wait.stop()
      await fail($, relay, `/clear was refused (${String(error)})`)
      return
    }
    const isCleared = await wait.ended
    wait.stop()
    if (!isCleared) {
      await fail($, relay, 'the session did not end within 5 s of /clear')
      return
    }
    await log($, relay, 'clear')
    await declareStay($, relay)

    const submitted = await $.prompt.submit({ text: relay.prompt })
    if (submitted.drop !== undefined) {
      await fail($, relay, `the prompt was refused (${submitted.drop})`)
      return
    }
    state = { kind: 'idle' }
    $.ui.status(undefined)
    await log($, relay, 'submit')
  } catch (error) {
    await fail($, relay, `the relay failed (${String(error)})`)
  }
}

// No session.start follows a /clear, so the command is declared at every arm
// and again after a clear; declaring a name again replaces it.
async function declareStay($: Engine, relay: Relay): Promise<void> {
  await $.command
    .register({
      name: 'stay',
      description: 'Keep this session: cancel the pending clear.',
    })
    .catch(error =>
      log($, relay, 'failure', `/stay was not registered (${String(error)})`),
    )
}

async function arm($: Engine, prompt: string): Promise<void> {
  const box = await $.prompt.read()
  if (box.text.trim() !== '') return
  const now = await $.clock.now()
  if (lastFiredAt !== undefined && now - lastFiredAt < MIN_GAP_MS) return

  const relay: Relay = {
    prompt,
    sessionId: await $.session.id(),
    cwd: await $.session.cwd(),
  }
  const text = `clearing in 10 s, then: ${shown(prompt)}; type anything or /stay to keep this session`
  const timer = $.clock.after(COUNTDOWN_MS, () => {
    void fire($)
  })
  state = { kind: 'armed', relay, timer }
  $.ui.toast(text, { timeoutMs: COUNTDOWN_MS })
  $.ui.status(text)
  await log($, relay, 'arm')

  await declareStay($, relay)
}

export const register: Register = (on, options) => {
  settings = {
    isEnabled: options.enabled !== false,
    stateDir: typeof options.stateDir === 'string' ? options.stateDir : '',
  }

  on('turn.complete', async ($, e, next) => {
    const result = await next(e)
    if (e.agentId !== undefined || state.kind === 'firing') return result

    try {
      const prompt = e.reason === 'answer' ? handoffPrompt(e.answer) : undefined
      if (prompt === undefined) {
        await disarm($, 'other-ending')
      } else if (settings.isEnabled && state.kind === 'idle') {
        await arm($, prompt)
      }
    } catch (error) {
      $.ui.log(`clear-relay: ${String(error)}`) // the answer stands whatever the relay does
    }

    return result
  })

  on('prompt.edit', async ($, e, next) => {
    if (state.kind === 'armed') await disarm($, 'keystroke')

    return next(e)
  })

  on('prompt.submit', async ($, e, next) => {
    const isOwn = e.origin?.kind === 'plugin' && e.origin.name === $.plugin.name
    if (!isOwn && state.kind === 'armed') await disarm($, 'prompt-submitted')

    return next(e)
  })

  on('turn.start', async ($, e, next) => {
    if (state.kind === 'armed') await disarm($, 'turn-started')

    return next(e)
  })

  on('command.run', { command: 'stay' }, async $ => {
    if (state.kind === 'armed') {
      await disarm($, 'stay')

      return { text: 'clear-relay: staying in this session.' }
    }

    return { text: 'clear-relay: nothing to cancel.' }
  })

  on('session.end', async ($, e, next) => {
    if (e.reason === 'clear') {
      if (state.kind === 'armed') await disarm($, 'cleared-by-person')
      else clearEnded?.()
    }

    return next(e)
  })
}
