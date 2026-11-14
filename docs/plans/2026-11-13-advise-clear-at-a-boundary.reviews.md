# Advise /clear at a boundary past 200k — full reviews


Round 1 (body c41403df2e80):
- architect, approve-with-changes (36k, 44 s): relais spawn breaks ADR-0064 (blocking, accepted); marker as a public constant (accepted); sweep.py matched by base name (accepted).
- backend, approve-with-changes (48k, 99 s): Silent is not "no open phase" (accepted); relais deadline (moot after ADR-0064; git deadline accepted); price ratio 0.1× (rejected: Opus 5.5 reads are 0.05×, now cited); pin the input set (accepted as the pasted-output baseline); log the stance departure (accepted).
- lang:python, approve-with-changes (38k, 56 s): denominator differs from the map's (accepted); reproduce 32 of 265 (rejected: pruned); cwd claim not what was measured (accepted, re-measured 198 of 805); one dedupe and filter (accepted); script hygiene (accepted).
- lang:rust, rework (42k, 68 s): boundary 1 never fires under plan-phases-open's deny (blocking, accepted); typed open-phase fact (accepted); deny sends Continue (accepted); relais spawn timeout (moot); turn-too-long (accepted); cheapest first (accepted); Cargo.lock (accepted).
- po, approve-with-changes (30k, 27 s): Outcome measures and rollback bar (accepted); backtest pass bar (accepted); Resolution dates (accepted); map line for the dropped CI signal (accepted).
- tui, approve-with-changes (34k, 30 s): advise-means-Continue departs from convention (accepted via rust's stance fix); decision::phrase (accepted); backticked next prompt (accepted); tui reason (accepted).
- unix, approve-with-changes (47k, 44 s): bounded git children under one deadline (accepted); atomic state file (accepted); -z names (accepted); relais machine-readable status (moot).

Round 2 (body cb0eda322293):
- architect, approve-with-changes (53k, 22 s): relais window regression (high, applied via backend#7); observe gives plan-phases-open back (low, carried).
- backend, approve-with-changes: boundary-1 turn re-entry into Phase 1 (high, applied); relais from the whole tail (high, applied); picture's yes branch (medium, carried); tui reason (low, carried).
- lang:python, approve-with-changes (52k, 31 s): Destination baseline rewrite (medium, carried); explicit --since (medium, carried); unmeasured "elsewhere" split (low, carried); SessionStart source=clear is not journalled, Outcome (a) needs a journal line (medium, carried).
- lang:rust, approve-with-changes (58k, 31 s): relais window (high, applied via backend#7); open_phase is more than one spawn and unbounded (medium, carried); bytes_bounded_detail for TimedOut (low, carried).
- po, approve-with-changes (42k, 18 s): backtest 10% bar for continued implementation (medium, carried); Outcome (b) counts denied entries (low, carried); PR text names the boundary-1 behaviour change (low, carried).
- tui, approve-with-changes (45k, 14 s): advise note gives the person an action, "say hand off" (medium, carried).
- unix, approve (59k, 19 s): open_phase under the deadline (low, carried); relative -C resolution (low, carried).

Delta (body 5f3cf2a2d15f):
- backend, approve-with-changes (73k, 21 s): 6 and 7 resolved; the turn memory for plan-phases-open's silence stores the opening prompt's uuid in the state file (medium, carried); the 8 MB limit on relais ids stated and counted in the backtest (low, carried).

Person's delta (2026-11-13, "programme keep going, clear and continue itself without me. implement the choices that will help us save costs"; body 1d694f6818f6):
- backend, approve-with-changes (43k, 58 s): adds=screen (rejected: a toast and status line are not a screen; lang:typescript seated from files — the person may overrule); background_busy before check 4 hides the pending boundary (high, applied); relayed prompt read as a person prompt (high, applied as `is_injected` on the engine's plugin framing, no prefix); await clear + session.end before submit (medium, applied); relay log (medium, applied, merged with python (d)); install dry-run tests (medium, applied); tui in code review (medium, applied).
- lang:typescript, approve-with-changes (46k, 43 s): `$.clock.after`, never sleep (high, applied); own-hook events and the idle/armed/firing state machine (high, applied); pass next(e) through (medium, applied); reload disarms, clock.now guard (medium, applied); /stay at module load (medium, applied); mocked-clock and self-submit tests (low, applied).
- lang:rust, approve-with-changes (52k, 54 s): background_busy (dup of backend#2); JSON state file with pending entry (high, applied); is_injected predicate (medium, applied); Phase 1b + cap tests (medium, applied); install inside plan_install/plan_uninstall (medium, applied); embedded file list test (low, applied).
- lang:python, approve-with-changes (41k, 47 s): split-proof measures (high, applied); (b) over the relayed session and (d) relay outcomes (high, applied); (a) joined on cwd, split by cause (medium, applied); map facts amended (medium, applied).

Round 2 (body fb010600a5b6):
- lang:rust, approve (58k, 21 s): stale "check 1's background_busy" wording (low, carried); pending-lost test (low, carried).
- backend, approve-with-changes (51k, 34 s): disarm ignored every plugin's prompt (high, applied with typescript#7); screen half left to the person.
- lang:typescript, approve-with-changes (53k, 32 s): same as backend#8 (high, applied); `$.fs.write` has no append, so one relay log per session, state dir passed in userConfig (medium, carried).
- lang:python, approve-with-changes (50k, 29 s): (c) target on compactions per 1,000 calls (medium, carried); Verification still lists background_busy as silent (medium, carried); a pending boundary with later relais work stays until a prompt or pending-lost (low, carried); a person-typed clear needs a definition (low, carried).

Final (body 11fe99abc0ea):
- backend, approve (49k, 70 s): finding 8 resolved; test the plugin-message framing at the is_person_prompt call site (low, carried).
