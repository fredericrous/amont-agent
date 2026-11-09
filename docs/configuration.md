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
| `amont.agent.<rule>.denyAgents` | repeatable: an agent type (`relais:relais-worker-haiku-default`) or a prefix ending in `*` (`relais:relais-worker-haiku-*`) |

Most specific wins. See [stances](stances.md).

`denyAgents` raises `advise` to `deny` for a subagent of a named type, and only
for `symbol-grep-not-lsp`. It is read from `--global` and `--system` config only
(a repository cannot set it), and the two lists are merged. An empty value, a
bare `*`, or a `*` anywhere but the end is ignored with one warning; a scope git
refuses adds no values and is reported once. The deny is refuse-once: the first
search for a pattern by that agent is refused, and the identical retry runs. If
the refusal cannot be recorded, the rule advises instead of refusing every retry.

```sh
# add the value once (idempotent), and take it back out (exit 5 means it was already absent)
git config --global --get-all amont.agent.symbol-grep-not-lsp.denyAgents | grep -qxF 'relais:relais-worker-haiku-*' \
  || git config --global --add amont.agent.symbol-grep-not-lsp.denyAgents 'relais:relais-worker-haiku-*'
git config --global --fixed-value --unset-all amont.agent.symbol-grep-not-lsp.denyAgents 'relais:relais-worker-haiku-*'
```

## The session notice

| key | default | effect |
|---|---|---|
| `amont.agent.fetch` | `true` | may a session opening touch the network at all |
| `amont.agent.agentsMdNotice` | `true` | may it mention a stale amont guidance block |
| `amont.agent.opener` | platform opener | the command `amont-agent open` and `preview register --open` use to show a page: a template, split like shell words, `{}` replaced by the page inside each word (appended when absent), run with no shell. Unset: `open`, `xdg-open` or `rundll32 url.dll,FileProtocolHandler {}` |
| `checkout.defaultRemote` | — | git's own key; which remote is *the* remote |

### Showing a page: `amont-agent open`

`amont-agent open <url|path>` opens a page in a window the person sees
(`work.page-shown-before-asked`). A path must exist and is opened as a
percent-encoded `file://` URL. The opener is spawned in its own process group
with no stdin or stdout and its error output kept in a temporary file; it is
waited for up to 2 s. Exit 0, or still running at 2 s (a browser that became
the process), counts as launched and it is never killed. Success prints
nothing; a failure prints one line naming the page, the opener's error output
and `amont.agent.opener`, and exits 1. A usage error exits 2.

The platform default can hand a page to an automation instance of the
browser, so name the command that reaches your own:

```sh
git config --global amont.agent.opener '"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" {}'
```

`preview register --open` uses the same opener and prints its JSON after the
attempt, with `"opened": true` or `"opened": false`; an opener failure leaves
the registration standing and the exit status 0. `amont-agent doctor` reports
the opener (and warns, with this command, when its program is not found).

## What is deliberately not configurable

**Which file any of these keys can be set in.** `--global`, then `--system`,
and nothing else. Not a committed file, and not the `--local`/`--worktree`
config of the repository you happen to be standing in — see
[stances](stances.md#why-git-config-and-not-a-committed-file). Write them with
`git config --global`; a bare `git config` inside a repository writes
`--local`, where this tool will not look.

**Whether a failure is silent.** It always is. See
[what it will not do](refusals.md).

**Whether the journal can affect a decision.** It cannot.
