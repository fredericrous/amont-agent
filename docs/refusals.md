# What it will not do

**It never emits `allow`.** That would short-circuit your own permission
prompt, so a guard approving everything it has no objection to would have
switched off the permission system it was installed beside. Silence is how it
says "no objection".

**Every failure path is silence.** An unreadable payload, an unknown event, a
command it cannot parse, a rule that panics, a journal it cannot write — all of
them exit 0 having written nothing.

A hook that fails toward *refusing* gets in the way of work you knew was
correct, and the fix people reach for at that moment is to delete it from
`settings.json`, which switches off every rule at once. One that fails toward
silence loses a single firing. That trade is the whole posture, and it is why
the hook payload is parsed with `serde_json::Value` and hand-written accessors
rather than a derived struct: a field that is missing or has changed type
becomes "no opinion", not a parse error somebody would be tempted to treat as
an opinion.

**It does not judge what it cannot read.** Heredocs without terminators,
`eval`, `sh -c`, unbalanced quotes — all opaque, and opaque never fires.

**It does not phone home.** No telemetry, no update checks, no fetches — with
one exception, which is `git fetch` against your own remote for the
[session notice](session-notice.md), and `amont.agent.fetch false` switches
that off.

## The journal

Every firing is recorded at `~/.claude/amont-agent/journal.log`, redacted, and
never transmitted anywhere.

It only **counts**. Nothing in it may participate in a decision — the rules
read the command in front of them and nothing else. A guard whose verdict
depended on its own history would be one you could not reason about from the
command alone, and could not test from a corpus.
