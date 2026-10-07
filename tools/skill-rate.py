#!/usr/bin/env python3
"""How often a release tag is pushed, or a pull request merged, without its skill.

For each Bash call in Claude Code transcripts that publishes — a `v*` tag
pushed, `gh pr merge`, or an HTTP client sent to `/pulls/<n>/merge` — it asks
the question `publish-without-skill` asks: was the matching skill
(`tag-release`, `merge-when-green`) called in this turn or the human turn
before it? It prints, per 1,000 Bash calls:

  matched      publishing calls
  no-skill     no call of the skill in the transcript up to that point
  stale        a call, but two or more human prompts ago
  allowance    covered only because one human prompt in between is allowed
  covered      covered in the same turn

then the uncovered rate by ISO week (weeks with at least 300 Bash calls), and the same counts per unique (session, command), since a refused command is
often retried; and the age of the covering skill call at each publish (p50,
max), to show whether a time cap is needed.

A subagent's calls are judged against its own transcript only: the parent's
position at that moment is not in the subagent's file, so a subagent covered
by its parent is counted as not covered here. The hook reads both.

    tools/skill-rate.py [--root DIR] [--since YYYY-MM-DD] [--list]

`--root` defaults to `$CLAUDE_CONFIG_DIR/projects` or `~/.claude/projects`.
`--list` prints each uncovered publish (session, verdict, command).

Keep in step with src/publish_cmd.rs and src/skill_window.rs. The matching
here is by regular expression over the command text, which the Rust lexer
does more carefully (quoted words, dry runs, clauses); the counts are a
measurement, not a replay.

Exit codes: 0 done (also when stdout is closed early, as by `| head`);
1 no transcripts under --root; 2 usage error.
"""

import argparse
import datetime
import json
import os
import pathlib
import re
import statistics
import sys

VERSION_TAG = re.compile(r"(?:^|\s)\+?(?:refs/tags/)?v\d[^\s;&|]*")
GIT_PUSH = re.compile(r"\bgit\s+(?:-C\s+\S+\s+)?push\b([^;&|\n]*)")
GH_MERGE = re.compile(r"\bgh\s+pr\s+merge\b")
API_MERGE = re.compile(r"\b(?:curl|wget|https?|httpie|xh|gh\s+api)\b[^;&|\n]*?/pulls/\d+/merge\b")


def classify(cmd: str):
    """`tag-release`, `merge-when-green`, or None."""
    for m in GIT_PUSH.finditer(cmd):
        args = m.group(1)
        if re.search(r"(?:^|\s)(?:--dry-run|-n|--delete|-d)(?:\s|$)", args):
            continue
        if VERSION_TAG.search(args) or re.search(r"(?:^|\s)--(?:tags|mirror)(?:\s|$)", args):
            return "tag-release"
    if GH_MERGE.search(cmd) or API_MERGE.search(cmd):
        return "merge-when-green"
    return None


def names(name: str, skill: str) -> bool:
    return name == skill or name.rsplit(":", 1)[-1] == skill


def typed(content, skill: str) -> bool:
    texts = [content] if isinstance(content, str) else [
        x.get("text", "") for x in (content or []) if isinstance(x, dict)]
    for t in texts:
        if not t.startswith("<command-"):
            continue
        m = re.search(r"<command-name>/([^<]+)</command-name>", t)
        if m and names(m.group(1), skill):
            return True
    return False


def stamp(e):
    try:
        return datetime.datetime.fromisoformat(e["timestamp"].replace("Z", "+00:00"))
    except (KeyError, ValueError, AttributeError):
        return None


def scan(path: pathlib.Path, since, out):
    humans = 0
    called = {}  # skill -> (humans at call, time)
    session = path.stem
    try:
        f = open(path, encoding="utf-8", errors="replace")
    except OSError:
        return
    with f:
        for line in f:
            if '"Bash"' not in line and '"Skill"' not in line and '"human"' not in line \
                    and "<command-name>" not in line:
                continue
            try:
                e = json.loads(line)
            except ValueError:
                continue
            t = stamp(e)
            kind = e.get("type")
            content = (e.get("message") or {}).get("content")
            if kind == "user":
                if (e.get("origin") or {}).get("kind") == "human":
                    humans += 1
                    for skill in ("tag-release", "merge-when-green"):
                        if typed(content, skill):
                            called[skill] = (humans, t)
                continue
            if kind != "assistant" or not isinstance(content, list):
                continue
            for c in content:
                if not isinstance(c, dict) or c.get("type") != "tool_use":
                    continue
                name, inp = c.get("name"), c.get("input") or {}
                if name == "Skill":
                    for skill in ("tag-release", "merge-when-green"):
                        if names(str(inp.get("skill", "")), skill):
                            called[skill] = (humans, t)
                elif name == "Bash":
                    if since and t and t.date() < since:
                        continue
                    out["bash"] += 1
                    week = (t.date() - datetime.timedelta(days=t.weekday())).isoformat() if t else "?"
                    wk = out["weeks"].setdefault(week, [0, 0])
                    wk[0] += 1
                    cmd = str(inp.get("command", ""))
                    skill = classify(cmd)
                    if not skill:
                        continue
                    at = called.get(skill)
                    if at is None:
                        verdict = "no-skill"
                    else:
                        gap = humans - at[0]
                        verdict = {0: "covered", 1: "allowance"}.get(gap, "stale")
                        if verdict in ("covered", "allowance") and t and at[1]:
                            out["ages"].append((t - at[1]).total_seconds())
                    out["counts"][verdict] = out["counts"].get(verdict, 0) + 1
                    key = (session, cmd.strip())
                    if key not in out["seen"]:
                        out["seen"][key] = verdict
                    if verdict in ("no-skill", "stale"):
                        wk[1] += 1
                        out["uncovered"].append((session[:8], verdict, cmd.strip()[:110]))


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    default = pathlib.Path(os.environ.get("CLAUDE_CONFIG_DIR") or pathlib.Path.home() / ".claude") / "projects"
    ap.add_argument("--root", type=pathlib.Path, default=default)
    ap.add_argument("--since", type=datetime.date.fromisoformat)
    ap.add_argument("--list", action="store_true")
    args = ap.parse_args()
    files = sorted(args.root.rglob("*.jsonl"))
    if not files:
        print(f"no transcripts under {args.root}", file=sys.stderr)
        return 1
    out = {"bash": 0, "counts": {}, "seen": {}, "ages": [], "uncovered": [], "weeks": {}}
    for p in files:
        scan(p, args.since, out)

    def per(n: int) -> float:
        return 1000 * n / out["bash"] if out["bash"] else 0.0

    c = out["counts"]
    matched = sum(c.values())
    uniq = {}
    for v in out["seen"].values():
        uniq[v] = uniq.get(v, 0) + 1
    print(f"transcripts {len(files)}   bash calls {out['bash']}")
    print(f"{'':12}{'calls':>8}{'per 1000':>10}{'unique':>9}")
    for name in ("matched", "no-skill", "stale", "allowance", "covered"):
        n = matched if name == "matched" else c.get(name, 0)
        u = sum(uniq.values()) if name == "matched" else uniq.get(name, 0)
        print(f"{name:12}{n:>8}{per(n):>10.2f}{u:>9}")
    if out["ages"]:
        ages = sorted(out["ages"])
        print(f"skill-call age at a covered publish: p50 {statistics.median(ages) / 60:.1f} min, "
              f"max {ages[-1] / 3600:.1f} h, n={len(ages)}")
    print("uncovered publishes per 1000 Bash calls, by week:")
    for week, (calls, bad) in sorted(out["weeks"].items()):
        if calls >= 300:
            print(f"  {week}  {calls:>6}  {1000 * bad / calls:6.2f}")
    if args.list:
        for row in out["uncovered"]:
            print("  ".join(row))
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except BrokenPipeError:
        sys.exit(0)
