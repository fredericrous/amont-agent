#!/usr/bin/env python3
"""Weekly rate of unbounded Reads over 16 KB, from Claude Code transcripts.

For each ISO week it prints the tool calls, the Reads with no `offset` and no
`limit` whose result is over 16 KB (after the exemptions the
`read-unbounded-large` rule makes), that count per 1000 tool calls, and the
characters those results carried.

The size is the length of the Read result, which is what landed in the
context; the rule itself stats the file, so the two agree to within the
line numbers the Read tool adds.

    tools/read-rate.py [--root DIR] [--weeks N] [--threshold BYTES]

`--root` defaults to `$CLAUDE_CONFIG_DIR/projects` or `~/.claude/projects`.

Exit codes: 0 done (also when stdout is closed early, as by `| head`);
1 no transcripts under --root; 2 usage error.
"""

import argparse
import datetime
import json
import os
import pathlib
import sys
from collections import defaultdict

# Keep in step with src/rules/read_unbounded_large.rs.
LARGE = 16 * 1024
MEDIA = {"png", "jpg", "jpeg", "gif", "webp", "pdf", "ipynb"}
DOCS = {"diff", "patch"}


def exempt(path: str) -> bool:
    p = pathlib.PurePosixPath(path)
    ext = p.suffix.lstrip(".").lower()
    if ext in MEDIA or ext in DOCS:
        return True
    parts = p.parts
    for a, b in zip(parts, parts[1:]):
        if b == "plans" and a in (".claude", "docs"):
            return True
    return "/tool-results/" in path.replace("\\", "/")


def text_of(content) -> str:
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "".join(
            c.get("text", "") for c in content if isinstance(c, dict)
        )
    return ""


def week_of(stamp: str):
    try:
        when = datetime.datetime.fromisoformat(stamp.replace("Z", "+00:00"))
    except ValueError:
        return None
    iso = when.isocalendar()
    return f"{iso[0]}-W{iso[1]:02d}"


def scan(root: pathlib.Path, threshold: int):
    calls = defaultdict(int)
    hits = defaultdict(int)
    chars = defaultdict(int)
    for log in root.rglob("*.jsonl"):
        pending = {}  # tool_use_id -> (week, path) for unbounded Reads
        try:
            lines = log.read_text(errors="replace").splitlines()
        except OSError:
            continue
        for raw in lines:
            try:
                rec = json.loads(raw)
            except ValueError:
                continue
            msg = rec.get("message")
            if not isinstance(msg, dict) or not isinstance(msg.get("content"), list):
                continue
            week = week_of(rec.get("timestamp", ""))
            if week is None:
                continue
            for block in msg["content"]:
                if not isinstance(block, dict):
                    continue
                kind = block.get("type")
                if kind == "tool_use":
                    calls[week] += 1
                    inp = block.get("input") or {}
                    path = inp.get("file_path")
                    if (
                        block.get("name") == "Read"
                        and isinstance(path, str)
                        and "offset" not in inp
                        and "limit" not in inp
                        and not exempt(path)
                    ):
                        pending[block.get("id")] = (week, path)
                elif kind == "tool_result":
                    got = pending.pop(block.get("tool_use_id"), None)
                    if got is None:
                        continue
                    size = len(text_of(block.get("content")))
                    if size > threshold:
                        hits[got[0]] += 1
                        chars[got[0]] += size
    return calls, hits, chars


def run() -> int:
    default = pathlib.Path(
        os.environ.get("CLAUDE_CONFIG_DIR") or pathlib.Path.home() / ".claude"
    ) / "projects"
    ap = argparse.ArgumentParser(
        description="Weekly rate of unbounded Reads over 16 KB, per 1000 tool calls."
    )
    ap.add_argument("--root", type=pathlib.Path, default=default,
                    help="transcripts directory (default: %(default)s)")
    ap.add_argument("--weeks", type=int, default=0,
                    help="only the last N weeks (default: all)")
    ap.add_argument("--threshold", type=int, default=LARGE,
                    help="result size in characters (default: %(default)s)")
    args = ap.parse_args()

    if not args.root.is_dir():
        print(f"read-rate: no transcripts under {args.root}", file=sys.stderr)
        return 1
    calls, hits, chars = scan(args.root, args.threshold)
    weeks = sorted(calls)
    if args.weeks > 0:
        weeks = weeks[-args.weeks:]
    print(f"{'week':<9} {'calls':>8} {'unbounded':>10} {'per 1000':>9} {'chars':>12}")
    for w in weeks:
        rate = 1000.0 * hits[w] / calls[w] if calls[w] else 0.0
        print(f"{w:<9} {calls[w]:>8} {hits[w]:>10} {rate:>9.1f} {chars[w]:>12}")
    return 0


def main() -> int:
    try:
        code = run()
        sys.stdout.flush()
        return code
    except BrokenPipeError:
        # The reader went away (`| head`). Point stdout at the null device so
        # the interpreter's exit flush does not raise again, and leave quietly.
        try:
            os.dup2(os.open(os.devnull, os.O_WRONLY), sys.stdout.fileno())
        except OSError:
            pass
        return 0


if __name__ == "__main__":
    sys.exit(main())
