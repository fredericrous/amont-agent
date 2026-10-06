#!/usr/bin/env python3
"""Rewrite the homebrew formula for a release — asserts, not hope.

The tap bump was amont's last manual release step, done by hand four times in
three days: copy a version, copy four sha256 lines, push. A transcription
error there ships a formula brew refuses (checksum mismatch) or, worse,
accepts. This script is the closed loop: it reads the release's own
SHA256SUMS, rewrites exactly the url/sha pairs the formula declares, and
refuses loudly on ANY surprise — a checksum file for the wrong version, a
target the formula wants that the release did not build, a rewrite count
other than what the formula carries, a stale version string surviving.

It also writes the caveat's list of rules that refuse by default, from the
released binary's own `amont-agent rules --json`: the `%w[...]` line inside
the formula's `def refusing_by_default`. That list was hand-written once
and went stale for four releases (it said only pipe-to-tail refuses, while
plan-review-panel had shipped at deny since 2.22.0); the formula's `brew
test` now checks it against the brewed binary as well.

Idempotent on purpose: re-run against a formula already at the version it
produces byte-identical output, so the workflow's "nothing to commit" path
is how a resumed release run no-ops.

Usage: bump-tap.py <version> <SHA256SUMS> <rules.json> <Formula/amont-agent.rb>
"""

import json
import pathlib
import re
import sys


def refusing_by_default(rules_path: str) -> list[str]:
    """The rules (not assertions) that ship at deny, sorted."""
    entries = json.loads(pathlib.Path(rules_path).read_text())
    assert isinstance(entries, list), f"{rules_path} is not a JSON array"
    ids = sorted(
        e["id"]
        for e in entries
        if e["kind"] == "rule" and e["default_stance"] == "deny"
    )
    # A release where nothing refuses is a surprise worth a red run, not a
    # caveat that says "none".
    assert ids, f"no rule ships at deny in {rules_path} — wrong file?"
    return ids


def rewrite_refusing(text: str, ids: list[str]) -> str:
    """Replace the one `%w[...]` line of `def refusing_by_default`."""
    method = re.compile(
        r"(?P<head>  def refusing_by_default\n    )%w\[[^\]\n]*\](?P<tail>\n  end\n)"
    )
    text, n = method.subn(
        lambda m: f"{m.group('head')}%w[{' '.join(ids)}]{m.group('tail')}", text
    )
    assert n == 1, (
        f"expected one `def refusing_by_default` holding a %w[...] line, found {n}"
    )
    return text


def main() -> None:
    assert len(sys.argv) == 5, __doc__.strip().splitlines()[-1]
    version, sums_path, rules_path, formula_path = sys.argv[1:5]
    version = version.lstrip("v")
    refusing = refusing_by_default(rules_path)

    sums: dict[str, tuple[str, str]] = {}
    for line in pathlib.Path(sums_path).read_text().splitlines():
        parts = line.split()
        if len(parts) != 2:
            continue
        sha, name = parts
        m = re.fullmatch(r"amont-agent-([0-9.]+)-(.+?)\.(?:tar\.gz|zip)", name)
        if not m:
            continue
        assert m.group(1) == version, (
            f"SHA256SUMS is for {m.group(1)}, not {version} — wrong release?"
        )
        sums[m.group(2)] = (sha, name)
    assert sums, f"no amont-agent checksums parsed from {sums_path}"

    formula = pathlib.Path(formula_path)
    text = formula.read_text()
    text, n_version = re.subn(
        r'version "[0-9.]+"', f'version "{version}"', text, count=1
    )
    assert n_version == 1, "the formula carries no version line"

    def rewrite(m: "re.Match[str]") -> str:
        target = m.group("target")
        assert target in sums, (
            f"the formula wants {target} and this release did not publish it"
        )
        sha, name = sums[target]
        return (
            f'url "https://github.com/fredericrous/amont-agent/releases/'
            f'download/v{version}/{name}"'
            f'{m.group("mid")}sha256 "{sha}"'
        )

    pair = re.compile(
        r'url "https://github\.com/fredericrous/amont-agent/releases/'
        r'download/v[0-9.]+/amont-agent-[0-9.]+-(?P<target>[a-z0-9_-]+)\.tar\.gz"'
        r'(?P<mid>\s+)sha256 "[0-9a-f]{64}"'
    )
    text, n = pair.subn(rewrite, text)
    assert n == 4, f"expected 4 url/sha pairs in the formula, rewrote {n}"

    stale = set(re.findall(r"amont-agent-([0-9.]+)-", text)) - {version}
    assert not stale, f"stale release versions survived the rewrite: {stale}"

    text = rewrite_refusing(text, refusing)

    formula.write_text(text)
    print(f"formula -> {version} ({n} targets; refusing: {' '.join(refusing)})")


if __name__ == "__main__":
    main()
