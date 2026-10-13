#!/usr/bin/env python3
"""Writes to agent guidance files per 1,000 tool calls, from transcripts.

Counts the Edit, MultiEdit and Write calls the `agent-guidance-written` and
`agent-guidance-shape` rules look at, per kind of file, against all tool calls:

    SKILL.md, CLAUDE.md, CLAUDE.md.tmpl, AGENTS.md, MEMORY.md, agents
    (`.claude/agents/*.md` and `dot_claude/agents/*.md`)

    tools/agent-guidance-rate.py [--root DIR] [--weeks N] [--until YYYY-MM-DD]

`--root` defaults to `$CLAUDE_CONFIG_DIR/projects` or `~/.claude/projects`.
A transcript holds what the model sent, never the file, so this counts writes
to the paths; it does not size the files.

Exit codes: 0 done (also when stdout is closed early, as by `| head`);
1 no transcripts under --root; 2 usage error.

Keep in step with src/rules/agent_guidance_written.rs.
"""

import argparse
import datetime
import json
import os
import pathlib
import sys
from collections import defaultdict

KINDS = ["SKILL.md", "CLAUDE.md", "CLAUDE.md.tmpl", "AGENTS.md", "MEMORY.md", "agents"]
WRITERS = ("Write", "Edit", "MultiEdit")


def kind_of(path):
    """The kind of guidance file `path` names, or None."""
    p = pathlib.PurePosixPath(path.replace("\\", "/"))
    if p.name in KINDS[:-1]:
        return p.name
    if p.name.endswith(".md") and p.parent.name == "agents" \
            and p.parent.parent.name in (".claude", "dot_claude"):
        return "agents"
    return None


def week_of(stamp):
    try:
        when = datetime.datetime.fromisoformat(stamp.replace("Z", "+00:00"))
    except ValueError:
        return None, None
    iso = when.isocalendar()
    return f"{iso[0]}-W{iso[1]:02d}", when.date()


def scan(root, until):
    calls = defaultdict(int)
    hits = defaultdict(lambda: defaultdict(int))
    skipped = {"files": 0, "lines": 0}
    for log in root.rglob("*.jsonl"):
        try:
            lines = log.read_text(errors="replace").splitlines()
        except OSError:
            skipped["files"] += 1
            continue
        for raw in lines:
            if not raw.strip():
                continue
            try:
                rec = json.loads(raw)
            except ValueError:
                skipped["lines"] += 1
                continue
            msg = rec.get("message")
            if not isinstance(msg, dict) or not isinstance(msg.get("content"), list):
                continue
            week, day = week_of(rec.get("timestamp", ""))
            if week is None or (until is not None and day > until):
                continue
            for block in msg["content"]:
                if not isinstance(block, dict) or block.get("type") != "tool_use":
                    continue
                calls[week] += 1
                inp = block.get("input")
                path = inp.get("file_path") if isinstance(inp, dict) else None
                if block.get("name") in WRITERS and isinstance(path, str):
                    kind = kind_of(path)
                    if kind:
                        hits[week][kind] += 1
    return calls, hits, skipped


def run(argv):
    default = pathlib.Path(
        os.environ.get("CLAUDE_CONFIG_DIR") or pathlib.Path.home() / ".claude"
    ) / "projects"
    ap = argparse.ArgumentParser(
        description="Writes to agent guidance files per 1,000 tool calls."
    )
    ap.add_argument("--root", type=pathlib.Path, default=default,
                    help="transcripts directory (default: %(default)s)")
    ap.add_argument("--weeks", type=int, default=0,
                    help="only the last N weeks (default: all)")
    ap.add_argument("--until", type=datetime.date.fromisoformat, default=None,
                    metavar="YYYY-MM-DD", help="ignore calls after this day")
    args = ap.parse_args(argv)

    if not args.root.is_dir():
        print(f"agent-guidance-rate: no transcripts under {args.root}", file=sys.stderr)
        return 1
    calls, hits, skipped = scan(args.root, args.until)
    weeks = sorted(calls)
    if args.weeks > 0:
        weeks = weeks[-args.weeks:]
    total = sum(calls[w] for w in weeks)
    print(f"tool calls: {total}")
    print(f"{'kind':<16} {'writes':>7} {'per 1000':>9}")
    every = 0
    for kind in KINDS:
        n = sum(hits[w][kind] for w in weeks)
        every += n
        print(f"{kind:<16} {n:>7} {1000.0 * n / total if total else 0.0:>9.2f}")
    print(f"{'all':<16} {every:>7} {1000.0 * every / total if total else 0.0:>9.2f}")
    if skipped["files"] or skipped["lines"]:
        print(f"agent-guidance-rate: skipped {skipped['files']} unreadable "
              f"file(s), {skipped['lines']} malformed line(s)", file=sys.stderr)
    return 0


def main():
    try:
        code = run(sys.argv[1:])
        sys.stdout.flush()
        return code
    except BrokenPipeError:
        try:
            os.dup2(os.open(os.devnull, os.O_WRONLY), sys.stdout.fileno())
        except OSError:
            pass
        return 0


if __name__ == "__main__":
    sys.exit(main())
