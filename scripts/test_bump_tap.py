#!/usr/bin/env python3
"""bump-tap.py's caveat rewrite: one line, from the binary's own list.

Run: python3 scripts/test_bump_tap.py (CI's `rust` job and `make lint` do).
Standard library only, like the script it tests.
"""

import importlib.util
import json
import pathlib
import shutil
import subprocess
import sys
import tempfile
import unittest

# Importing the script by path would leave a __pycache__ in scripts/.
sys.dont_write_bytecode = True

HERE = pathlib.Path(__file__).resolve().parent
SCRIPT = HERE / "bump-tap.py"

spec = importlib.util.spec_from_file_location("bump_tap", SCRIPT)
bump_tap = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bump_tap)

TARGETS = [
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "aarch64-unknown-linux-gnu",
    "x86_64-unknown-linux-gnu",
]

FORMULA = (
    'class AmontAgent < Formula\n  version "1.0.0"\n'
    + "".join(
        f'  url "https://github.com/fredericrous/amont-agent/releases/download/'
        f'v1.0.0/amont-agent-1.0.0-{t}.tar.gz"\n  sha256 "{"0" * 64}"\n'
        for t in TARGETS
    )
    + "\n  def refusing_by_default\n    %w[pipe-to-tail]\n  end\nend\n"
)


def rule(id_: str, ships: str, kind: str = "rule") -> dict:
    return {
        "id": id_,
        "kind": kind,
        "default_stance": ships,
        "stance": ships,
        "max_stance": "deny",
        "per_1000": 1.0,
        "measured": "2026-10-06",
    }


class BumpTap(unittest.TestCase):
    def setUp(self) -> None:
        self.dir = pathlib.Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.dir)
        self.formula = self.dir / "amont-agent.rb"
        self.formula.write_text(FORMULA)
        self.sums = self.dir / "SHA256SUMS"
        self.sums.write_text(
            "".join(f"{'a' * 64}  amont-agent-2.0.0-{t}.tar.gz\n" for t in TARGETS)
        )

    def run_script(self, rules: list) -> subprocess.CompletedProcess:
        path = self.dir / "rules.json"
        path.write_text(json.dumps(rules))
        return subprocess.run(
            [sys.executable, "-I", str(SCRIPT), "2.0.0", str(self.sums), str(path), str(self.formula)],
            capture_output=True,
            text=True,
        )

    def test_the_list_is_the_sorted_rules_that_ship_deny(self) -> None:
        r = self.run_script(
            [
                rule("pipe-to-tail", "deny"),
                rule("no-verify", "observe"),
                rule("plan-review-panel", "deny"),
                rule("push-landed", "deny", kind="assertion"),
            ]
        )
        self.assertEqual(r.returncode, 0, r.stderr)
        text = self.formula.read_text()
        self.assertIn("    %w[pipe-to-tail plan-review-panel]\n", text)
        self.assertNotIn("push-landed", text, "an assertion is not a rule that refuses")
        self.assertIn('version "2.0.0"', text)

    def test_a_second_run_changes_nothing(self) -> None:
        rules = [rule("pipe-to-tail", "deny"), rule("plan-review-panel", "deny")]
        self.assertEqual(self.run_script(rules).returncode, 0)
        once = self.formula.read_text()
        self.assertEqual(self.run_script(rules).returncode, 0)
        self.assertEqual(self.formula.read_text(), once)

    def test_no_rule_at_deny_is_refused(self) -> None:
        r = self.run_script([rule("pipe-to-tail", "advise"), rule("push-landed", "deny", kind="assertion")])
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("no rule ships at deny", r.stderr)
        self.assertEqual(self.formula.read_text(), FORMULA, "nothing written")

    def test_a_formula_without_the_method_is_refused(self) -> None:
        self.formula.write_text(FORMULA.replace("refusing_by_default", "something_else"))
        r = self.run_script([rule("pipe-to-tail", "deny")])
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("def refusing_by_default", r.stderr)

    def test_the_rewrite_touches_only_that_line(self) -> None:
        text = bump_tap.rewrite_refusing(FORMULA, ["a", "b"])
        changed = [
            (old, new)
            for old, new in zip(FORMULA.splitlines(), text.splitlines())
            if old != new
        ]
        self.assertEqual(changed, [("    %w[pipe-to-tail]", "    %w[a b]")])


if __name__ == "__main__":
    unittest.main()
