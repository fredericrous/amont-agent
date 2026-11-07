# Full reviews: A sliced reviewer is bound to the part of the plan it reviews

Round 1 (body aae58f836b7c), all opus-5-5:
- lang:rust, approve-with-changes (35k, 33 s): high, Verification and Decision-log rows in shared sections move every slice (resolved by the decision-1 flip); medium, the `now` closure in judge must share the slice rule (applied); medium, a second backtest on plans that name files in every phase (applied).
- architect, approve-with-changes (43k, 49 s): high, per-span shas in the baseline so the refusal can name the phase (applied); high, decision 1 as first recommended re-seats on almost every delta (recommendation flipped); medium, missing-key transitions (rejected: duplicate of backend 2); medium, ship the skill before installing 3.23.0 (applied).
- po, approve-with-changes (32k, 48 s): high, the Goal hid that the plan adds re-runs and saves no seat (applied); high, no measure after release (14-day measure with a rollback bar, applied); medium, decision 1's cost unstated (applied in the 👉 line).
- tui, approve-with-changes (30k, 35 s): skill §3.3 carries one or two lines, stdout only; a message for every exit-2 case; refusal ends with the re-run command (all applied).
- unix, approve-with-changes (36k, 51 s): `PLAN_SHA_USAGE` one-line contract; missing-value and unknown-role exits; `amont-agent:` prefix; a malformed `slices` reads as absent (all applied, merged with tui by hand).
- backend, approve-with-changes (50k, 95 s): measure decision 1 as n/45; areas→paths and sliced→whole transitions; a two-seat fixture; downgrade path; journal fields (all applied).

Round 2 (body 311cc28c6ae8), backend resumed, approve-with-changes (60k, 18 s): round-1 findings 1–5 resolved. Carried into implementation: A medium, the Context bullet on decision 1's cost reads backwards after the flip; B low, Preview/Approach/Outcome are in no list (they are outside every slice); C low, a positional phase label is for display only.

Codex (gpt-6-astra, high effort), approve-with-changes: high, slicing before parsing loses reference-link definitions a kept phase uses (applied to Behaviour; backend re-ran); medium, a slice-rule version bump is bypassed by the `current` fast path and the `sha=` fallback (carried: `is_current` and `now` must compare slice shas under the current `slice/` prefix).

Backend re-run (body 60b87cfbf4b7), approve-with-changes (70k, 34 s): Codex edit resolved, nothing else moved. Carried: low, add the reference-definition and footnote case to `## Tests`.

Panel total: about 299k tokens across 9 reviews (t10's target is 150k).
