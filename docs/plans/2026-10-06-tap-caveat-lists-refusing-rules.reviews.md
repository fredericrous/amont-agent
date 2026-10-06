# Full reviews: Tap caveat lists the rules that refuse

- **tui** (round 1) — approve-with-changes, 35k, 35 s. Blocking: the guard took the wrong branch on 2.25.0, which ignores `--json` and exits 0; `flags()` refuses nothing; `rules --help` would become exit 2; one deny rule per caveat line.
- **backend** (round 1) — approve-with-changes, 39k, 48 s. Same blocker; `flags()`; no float format (NaN/inf); machine-dependent verification; `kind` filter in the formula; the JSON keys as a contract.
- **unix** (round 1) — approve-with-changes, 39k, 40 s. Same blocker (parse-based guard proposed); `rest` not rejected; publish step unspecified; trailing newline; bump-tap docstring.
- **lang:rust** (round 1) — approve-with-changes, 41k, 54 s. Same blocker; `flags()`; `per_1000` is `f32`; named integration tests; `kind` from the list.
- **backend** (round 2) — approve-with-changes, 38k, 69 s. The parse fallback would hide a later `--json` regression → version gate; `pipefail` not set by the job; test (b) hard-coded a rule; ambiguous `find`.
- **backend** (round 2, final) — approve, 28k, 18 s. Low, applied at implementation: the by-hand formula check needs a copy at `version "2.26.0"` with a local tarball url, or the gate never takes the JSON branch.
