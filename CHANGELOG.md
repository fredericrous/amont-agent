# Changelog

## v2.0.1

### Removed

- **There is no npm package, and there will not be one.** `amont-agent` was
  published to npm as part of v2.0.0 — or rather, five of its seven packages
  were, before npm's spam filter rejected the sixth by name. That block was
  worth listening to, because it stopped a package that should not have
  existed.

  `install` bakes the ABSOLUTE path of the running binary into
  `settings.json`, deliberately: a `PATH`-resolved command exits 127 into
  Claude Code's non-blocking bucket the moment `PATH` differs, which disables
  the guard with nothing to notice it. npm cannot supply a stable absolute
  path. Under `npx` the binary sits in npm's `_npx` cache, which npm garbage-
  collects. Under a project-local `npm i -D` it sits in one project's
  `node_modules`, while the guard it configures is machine-global — so
  `rm -rf node_modules` in a single repository would silently disable the
  guard for every session on the machine.

  amont's npm package exists for a reason that does not transfer: it is a
  per-repository tool, and `npm i -D amont` plus a `prepare` script makes the
  hooks travel with the repository. This is per-developer machine
  configuration, and nothing about it belongs to a project.

  The five platform packages published under v2.0.0 have been unpublished.
  The root package `amont-agent` was never published — the release workflow
  publishes platform packages before the package that depends on them, so
  the failure stopped short of creating a root package with a broken
  `optionalDependency`.

  **Install with Homebrew, cargo, the shell installer, or a release binary.**
  Each hands `install` a path that stays put.

## v2.0.0

`amont-agent` is now its own project. It was previously a third binary inside
the [amont](https://github.com/fredericrous/amont) release — bundled in the
tarball, in the `amont` npm package, and in amont's installers.

**Nothing about how the guard behaves has changed.** The hook's output is
byte-identical to 1.18.2 for every rule. Config keys are unchanged
(`amont.agent.*`, `$AMONT_AGENT_OFF`), the journal is still at
`~/.claude/amont-agent/journal.log`, and an existing `settings.json` entry
keeps working.

### If you had it from amont

Your installed copy still works and is not removed. amont 1.19.0 stops
shipping it, so to keep getting updates install it from here:

```sh
brew install fredericrous/tap/amont-agent
# or
curl -fsSL https://raw.githubusercontent.com/fredericrous/amont-agent/main/install/install.sh | sh
```

Then `rm ~/.local/bin/amont-agent` is safe once the new one is on `PATH`, and
`amont-agent doctor` will confirm which one Claude Code is actually running.

### Why the major bump

The version had to exceed 1.18.2 on crates.io regardless. `2.0.0` marks the
real break — it is no longer bundled and must be installed separately — and
keeps the two version streams independent, so `amont 1.19` and
`amont-agent 2.0` never look like they must match.

### Changed

- **No `amont-runtime` dependency.** Six small modules replace what this crate
  used to reach for. `cargo tree` is now `serde` and `serde_json` and nothing
  else — both only for *reading*.
- **A cloned repository can no longer weaken a stance.** amont's config reader
  lets a repository's committed policy `set` lines outrank system and global
  git config — correct for a hook manager, wrong for a guard, because it meant
  a cloned repository could set `amont.agent.<rule>.stance = observe` and
  disarm the guard on the machine of whoever cloned it. The reader here has no
  policy ladder. Stances answer to your own git config and to nothing a
  `git clone` can carry.
- **`settings.json` permissions are no longer widened.** The previous write
  path set mode `0644` unconditionally. An existing file now keeps the mode it
  had, and one created here starts at `0600` — it can hold MCP environment
  blocks, and those hold credentials.
- **The stale-`AGENTS.md` notice now shells out** to `amont agents-md --check`
  instead of linking amont's generator, and decides on amont's *stderr* rather
  than its exit code. `agents-md --check` returns 1 for a file it could not
  read as well as for a drifted one, so the exit code alone would announce
  staleness for a permissions error. With no `amont` on `PATH` the check is
  silent, which is the right answer for anyone who does not use amont.

### Fixed

- **The declared MSRV was not buildable.** Every published version up to 1.18.2
  claimed `rust-version = "1.74"`, inherited from amont's dependency-free
  commit path and — as that manifest's own comment predicted — long since
  drifted: `serde_json`'s `preserve_order` pulls `indexmap` → `hashbrown
  0.17.1`, which is edition 2024 and requires 1.85. Cargo 1.74 could not parse
  it at all. The floor is now **1.85.0**, measured (1.84 fails, 1.85 passes)
  and enforced by CI against the committed lockfile.

### Fixed (2)

- **A closed pipe no longer panics.** `amont-agent rules | head`,
  `--help | grep -q`, or any listing whose reader hangs up early printed a
  Rust backtrace — "failed printing to stdout: Broken pipe" — instead of
  dying from the signal like every other Unix filter. Rust ignores SIGPIPE at
  startup, so `println!` hits EPIPE and EPIPE is a panic.

  This is a bug the split introduced by omission: amont's `main` has restored
  SIGPIPE's default disposition since `amont list | head` panicked, and this
  crate left the workspace with the modules it imported rather than the ones
  it needed. The v2.0.0 release dry run caught it, from a smoke step doing
  `amont-agent --help | grep -q backtest` — which failed on exactly one of
  six build targets, because the output is small enough that whether the race
  fires depends on the machine.

### Added

- **Documentation.** `backtest` → `explain` → `corpus check` → `graduate` is a
  coherent measure-then-block workflow and was previously undocumented
  anywhere. It now has [a page of its own](docs/measuring.md), with the
  `pipe-to-tail` decision as the worked example.
- Windows is now in the test matrix alongside Linux and macOS.
