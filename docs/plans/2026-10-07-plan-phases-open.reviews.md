# Plan phases open — full reviews

**tui** — approve-with-changes (34k, 28 s). high: silent cap release → systemMessage to the person (applied). medium: quote the phase label ≤80 chars, clamp (applied); separate advise wording (applied). low: fact+remedy via `decision::phrase` (applied); "a line starting with WAITING:" (applied).

**backend** — R1 approve-with-changes (42k, 51 s): plan choice when several in scope; existing installs need `install` re-run + doctor test; cap semantics, sweep call, session guard; docs counts from `RULES.len()`; actual: slots; hyperfine budget; rollback line — all applied. R2 approve-with-changes (37k, 32 s): reset on UserPromptSubmit (applied, logged); `--diff-filter=d` + per-candidate skip (applied); walk up to `.git`, detached HEAD silent (applied). Delta 1 (33k, 23 s): wire `on_prompt` in Phase 2; step 7 wording — applied. Delta 2 approve (30k, 16 s); low, carried into implementation: the release-binary `../x` check also drives a UserPromptSubmit payload and asserts a sentinel `<journal dir>/x` survives.

**unix** — approve-with-changes (40k, 35 s). high: session-id path traversal guard (applied); unwritable counter fails open (applied). medium: `git diff -z` via raw helper (applied). low: untracked plans (applied, `ls-files --others`); cheap exits before git (applied).

**lang:rust** — approve-with-changes (42k, 47 s). high: empty/unsafe session id silent (applied). medium: required vs optional payload fields, empty cwd silent (applied); fact+remedy (applied); one test per escape + `decision.rs` top-level unit test (applied). low: fences and indented boxes (applied); journal `stop_hook_active` (applied).
