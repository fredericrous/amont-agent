# Configuration

Every key is plain `git config`, readable and removable without this tool.

## The kill switches

| key | default | effect |
|---|---|---|
| `$AMONT_AGENT_OFF` | unset | any value switches the guard off for that shell |
| `amont.agent.enabled` | `true` | switches it off everywhere |

The environment variable is checked **first, before any rule runs**, because
reading a git config key costs a process and the variable costs nothing.
`amont.agent.enabled` is consulted only once something has already fired.

Both take git's own boolean dialect — `true`/`false`, `yes`/`no`, `on`/`off`,
`1`/`0`, case-insensitively — because git parses them, not us. A value git
refuses warns once and falls back to the default rather than failing.

## Stances

| key | takes |
|---|---|
| `amont.agent.stance` | `observe` \| `advise` \| `deny` — every rule |
| `amont.agent.<rule>.stance` | the same, for one rule |

Most specific wins. See [stances](stances.md).

## The session notice

| key | default | effect |
|---|---|---|
| `amont.agent.fetch` | `true` | may a session opening touch the network at all |
| `amont.agent.agentsMdNotice` | `true` | may it mention a stale amont guidance block |
| `checkout.defaultRemote` | — | git's own key; which remote is *the* remote |

## What is deliberately not configurable

**Which file a stance can be set in.** Only your own git config, never a
committed one — see [stances](stances.md#why-git-config-and-not-a-committed-file).

**Whether a failure is silent.** It always is. See
[what it will not do](refusals.md).

**Whether the journal can affect a decision.** It cannot.
