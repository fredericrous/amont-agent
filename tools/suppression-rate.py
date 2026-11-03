#!/usr/bin/env python3
"""Weekly rate of Edit/Write calls that add a lint suppression, from transcripts.

For each ISO week it prints the tool calls, the Edit, MultiEdit and Write calls
that add a suppression marker or loosen a lint setting (what the
`lint-suppression-added` rule advises on), and that count per 1000 tool calls.

FRAGMENTS ONLY. A transcript records what the model sent, never the file on
disk, so this compares `old_string` against `new_string` (or an empty text
against a Write's `content`), the hook's fragment fallback. It has no section
context, and it undercounts the config rows that need one:

  - `[tool.pyright]` and `[tool.ruff*]` in pyproject.toml,
  - `[lints.*]` and `[workspace.lints.*]` in Cargo.toml,
  - the `disable:` and `exclude*` items of .golangci.yml,
  - an entry added to an `ignore` array whose key is outside the edit,

unless the edited text carries the header or the key itself. The rule, with
the file in hand, sees them; the number here is a floor for them.

    tools/suppression-rate.py [--root DIR] [--weeks N] [--until YYYY-MM-DD]
    tools/suppression-rate.py --self-test

`--root` defaults to `$CLAUDE_CONFIG_DIR/projects` or `~/.claude/projects`.
`--self-test` classifies tests/fixtures/suppressions.txt, the file the Rust
unit test classifies too, and exits 0 when every sample agrees with its tag.

Exit codes: 0 done (also when stdout is closed early, as by `| head`);
1 no transcripts under --root, or a failed self-test; 2 usage error.

Keep in step with crates/amont-agent-guard/src/rules/lint_suppression_added.rs.
"""

import argparse
import datetime
import json
import os
import pathlib
import re
import sys
from collections import Counter, defaultdict

# ---- classifier, a port of the rule's scanners --------------------------------


def after_prefix(s, prefix):
    head = s[: len(prefix)]
    return s[len(prefix):] if head.lower() == prefix.lower() else None


def boundary(tail):
    return not tail or not (tail[0].isalnum() or tail[0] in "_-")


def bracket_codes(tail):
    if not tail.startswith("["):
        return []
    inner = tail[1:].split("]")[0]
    return [c.strip() for c in inner.split(",") if c.strip()]


def is_code(t):
    return (
        t[:1].isascii() and t[:1].isalpha()
        and t.isascii() and t.isalnum()
        and any(c.isdigit() for c in t)
    )


def noqa_codes(tail):
    tail = tail.lstrip()
    if not tail.startswith(":"):
        return []
    out = []
    for tok in [t for t in re.split(r"[,\s]+", tail[1:]) if t]:
        if not is_code(tok):
            break
        out.append(tok.upper())
    return out


def pyright_loose(key, value):
    return (
        (key.startswith("report") and value in ("none", "false", "warning", "information"))
        or (key == "typeCheckingMode" and value in ("off", "basic"))
        or (key == "enableTypeIgnoreComments" and value == "true")
    )


def codes_of(kind, codes, line):
    """One marker per code, or a single bare one, as (kind, code, section, line)."""
    return [(kind, c, "", line) for c in codes] or [(kind, "", "", line)]


def py_comment(line, state):
    """The comment on this line, if a `#` falls outside every string."""
    single = None
    i = 0
    while i < len(line):
        c = line[i]
        if state["triple"]:
            q = state["triple"]
            if c == "\\":
                i += 2
            elif c == q and line.startswith(q * 3, i):
                state["triple"] = None
                i += 3
            else:
                i += 1
            continue
        if single:
            if c == "\\":
                i += 2
                continue
            if c == single:
                single = None
            i += 1
            continue
        if c == "#":
            return line[i + 1:]
        if c in "\"'":
            if line.startswith(c * 3, i):
                state["triple"] = c
                i += 3
                continue
            single = c
        i += 1
    return None


def python(lines):
    out = []
    state = {"triple": None}
    for number, line in enumerate(lines, 1):
        comment = py_comment(line, state)
        if comment is None:
            continue
        for seg in comment.split("#"):
            seg = seg.strip()
            rest = after_prefix(seg, "type:")
            if rest is not None:
                tail = after_prefix(rest.lstrip(), "ignore")
                if tail is not None and boundary(tail):
                    out += codes_of("type: ignore", bracket_codes(tail), number)
                continue
            rest = after_prefix(seg, "pyright:")
            if rest is not None:
                rest = rest.lstrip()
                tail = after_prefix(rest, "ignore")
                if tail is not None and boundary(tail):
                    out += codes_of("pyright: ignore", bracket_codes(tail), number)
                    continue
                for item in rest.split(","):
                    item = "".join(item.split())
                    if "=" in item:
                        k, v = item.split("=", 1)
                        loose = pyright_loose(k, v)
                    else:
                        loose = item in ("basic", "off")
                    if loose:
                        out.append(("pyright", item, "", number))
                continue
            rest = after_prefix(seg, "ruff:")
            tail = after_prefix(rest.lstrip(), "noqa") if rest is not None else None
            if tail is not None and boundary(tail):
                out += codes_of("ruff: noqa", noqa_codes(tail), number)
                continue
            tail = after_prefix(seg, "noqa")
            if tail is not None and boundary(tail):
                out += codes_of("noqa", noqa_codes(tail), number)
    return out


def c_comments(text):
    """(line, is_block, body) of every comment outside a string."""
    out = []
    line = 1
    counted = 0
    state = "code"
    quote = ""
    i = 0
    n = len(text)
    while i < n:
        c = text[i]
        line += text.count("\n", counted, i)
        counted = i
        if state == "code":
            if text.startswith("//", i):
                end = text.find("\n", i)
                end = n if end < 0 else end
                out.append((line, False, text[i + 2:end]))
                i = end
            elif text.startswith("/*", i):
                j = i + 2
                while j < n and text[j] != "\n" and not text.startswith("*/", j):
                    j += 1
                out.append((line, True, text[i + 2:j]))
                if text.startswith("*/", j):
                    i = j + 2
                else:
                    i = j
                    state = "block"
            elif c in "\"'`":
                state = "str"
                quote = c
                i += 1
            else:
                i += 1
        elif state == "str":
            if c == "\\":
                i += 2
            elif c == "\n":
                if quote != "`":
                    state = "code"
                i += 1
            elif c == quote:
                state = "code"
                i += 1
            else:
                i += 1
        else:
            if text.startswith("*/", i):
                state = "code"
                i += 2
            else:
                i += 1
    return out


def script(text):
    out = []
    for line, _block, body in c_comments(text):
        body = body.strip().lstrip("/*").strip()
        if body.startswith("eslint-disable"):
            rest = body[len("eslint-disable"):]
            suffix = ""
            for s in ("-next-line", "-line"):
                if rest.startswith(s):
                    suffix, rest = s, rest[len(s):]
                    break
            if not boundary(rest):
                continue
            codes = [c.strip() for c in rest.split("--")[0].split(",") if c.strip()]
            out += codes_of("eslint-disable" + suffix, codes, line)
            continue
        for tag in ("@ts-ignore", "@ts-nocheck", "@ts-expect-error"):
            if body.startswith(tag) and boundary(body[len(tag):]):
                out.append((tag, "", "", line))
    return out


def go(text):
    out = []
    for line, block, body in c_comments(text):
        if block or not body.startswith("nolint") or not boundary(body[6:]):
            continue
        rest = body[6:]
        codes = []
        if rest.startswith(":"):
            word = (rest[1:].split() or [""])[0]
            codes = [c.strip() for c in word.split(",") if c.strip()]
        out += codes_of("nolint", codes, line)
    return out


def paren_items(s):
    items, cur, depth, quote, prev = [], "", 1, False, " "
    for c in s:
        if quote:
            cur += c
            if c == '"' and prev != "\\":
                quote = False
        elif c == '"':
            quote = True
            cur += c
        elif c == "(":
            depth += 1
            cur += c
        elif c == ")":
            depth -= 1
            if depth == 0:
                break
            cur += c
        elif c == "," and depth == 1:
            items.append(cur)
            cur = ""
        else:
            cur += c
        prev = c
    items.append(cur)
    items = [i.strip() for i in items]
    return [i for i in items if i and not i.startswith("reason")]


def rust(lines):
    out = []
    for number, line in enumerate(lines, 1):
        t = line.lstrip()
        if not t.startswith("#"):
            continue
        t = t[1:]
        if t.startswith("!"):
            t = t[1:]
        if not t.startswith("["):
            continue
        inner = t[1:].lstrip()
        for kind in ("allow", "expect"):
            if inner.startswith(kind + "("):
                out += codes_of(kind, paren_items(inner[len(kind) + 1:]), number)
        if inner.startswith("cfg_attr("):
            args = inner[len("cfg_attr("):]
            found = []
            for kind in ("allow", "expect"):
                needle = kind + "("
                at = args.find(needle)
                while at >= 0:
                    if at > 0 and args[at - 1] in " ,":
                        found.append((at, kind))
                    at = args.find(needle, at + 1)
            for at, kind in sorted(found):
                out += codes_of(kind, paren_items(args[at + len(kind) + 1:]), number)
    return out


def shell_comment(line):
    """The text after the first `#` that starts a shell comment: at the start of
    the line or after whitespace, outside quotes."""
    quote = None
    prev = " "
    for i, c in enumerate(line):
        if quote:
            if c == quote:
                quote = None
        elif c in "'\"":
            quote = c
        elif c == "#" and prev.isspace():
            return line[i + 1:]
        prev = c
    return None


def shellcheck_disable(comment):
    """(codes, text after the list) of a `shellcheck disable=..` comment."""
    rest = comment.lstrip()
    if not rest.startswith("shellcheck"):
        return None
    rest = rest[len("shellcheck"):]
    if not rest[:1].isspace():
        return None
    rest = rest.lstrip()
    if not rest.startswith("disable="):
        return None
    listing = rest[len("disable="):]
    words = listing.split(None, 1)
    head = listing.split()[0] if words else ""
    after = listing[len(head):]
    return [c.strip() for c in head.split(",") if c.strip()], after


def shell(lines):
    out = []
    for number, line in enumerate(lines, 1):
        comment = shell_comment(line)
        parsed = shellcheck_disable(comment) if comment is not None else None
        if parsed:
            out += [("shellcheck disable", c, "", number) for c in parsed[0]]
    return out


def shell_by_shebang(base, text):
    """An extensionless file whose first line is a sh, bash or zsh shebang."""
    if "." in base.lstrip("."):
        return False
    first = text.split("\n", 1)[0].rstrip("\r")
    if not first.startswith("#!"):
        return False
    words = first[2:].split()
    program = words[0] if words else ""
    if program.rsplit("/", 1)[-1] == "env":
        program = next((w for w in words[1:] if not w.startswith("-")), "")
    return program.rsplit("/", 1)[-1] in ("sh", "bash", "zsh")


def ident(c):
    return c.isalnum() or c in "_@/.-$"


def kv_pairs(line):
    """Every `key: value` on a line of JSON, JS or YAML."""
    out = []
    i = 0
    n = len(line)

    def quoted(at):
        end = line.find(line[at], at + 1)
        return None if end < 0 else (line[at + 1:end], end + 1)

    while i < n:
        c = line[i]
        if c in "\"'":
            got = quoted(i)
            if got is None:
                break
            key, nxt = got
        elif ident(c):
            nxt = i
            while nxt < n and ident(line[nxt]):
                nxt += 1
            key = line[i:nxt]
        else:
            i += 1
            continue
        j = nxt
        while j < n and line[j].isspace():
            j += 1
        if j >= n or line[j] != ":":
            i = nxt
            continue
        j += 1
        while j < n and line[j].isspace():
            j += 1
        if j < n and line[j] == "[":
            j += 1
            while j < n and line[j].isspace():
                j += 1
        if j >= n:
            break
        if line[j] in "\"'":
            value, after = quoted(j) or ("", j)
        else:
            after = j
            while after < n and ident(line[after]):
                after += 1
            value = line[j:after]
        out.append((key, value))
        i = max(after, j + 1)
    return out


def json_lines(lines, loose):
    out = []
    for line in lines:
        t = line.lstrip()
        if t.startswith("//") or t.startswith("#"):
            continue
        out += [(k, v, "") for k, v in kv_pairs(line) if loose(k, v)]
    return out


def strip_toml_comment(line):
    quote = None
    for i, c in enumerate(line):
        if quote:
            if c == quote:
                quote = None
        elif c in "\"'":
            quote = c
        elif c == "#":
            return line[:i]
    return line


def array_entries(s):
    out, cur, quote = [], "", None
    for c in s:
        if quote:
            if c == quote:
                out.append(cur)
                cur, quote = "", None
            else:
                cur += c
        elif c in "\"'":
            quote = c
        elif c == "]":
            return out, True
    return out, False


def unquote(v):
    v = v.strip()
    if v[:1] in ("'", '"'):
        return v[1:].split(v[0])[0]
    return (v.split() or [""])[0]


def split_kv(line):
    if line[:1] in ("'", '"'):
        end = line.find(line[0], 1)
        if end < 0:
            return None
        rest = line[end + 1:].lstrip()
        if not rest.startswith("="):
            return None
        return line[1:end], rest[1:].strip()
    if "=" not in line:
        return None
    k, v = line.split("=", 1)
    return k.strip(), v.strip()


def lints_section(section):
    return any(
        section == p or section.startswith(p + ".")
        for p in ("lints", "workspace.lints")
    )


def toml(lines, kind):
    out = []
    section = ""
    open_key = None

    def ruff_section():
        return kind == "ruff" or (kind == "pyproject" and section.startswith("tool.ruff"))

    def entry(key, e):
        ignoring = key in ("ignore", "extend-ignore") or section.endswith("per-file-ignores")
        if ruff_section() and ignoring:
            out.append((key, e, section))

    for raw in lines:
        line = strip_toml_comment(raw).strip()
        if not line:
            continue
        if open_key is not None:
            entries, closed = array_entries(line)
            for e in entries:
                entry(open_key, e)
            if closed:
                open_key = None
            continue
        if line.startswith("["):
            head = line.lstrip("[").rstrip("]").strip()
            section = ".".join(p.strip() for p in head.split("."))
            continue
        kv = split_kv(line)
        if kv is None:
            continue
        key, val = kv
        if val.startswith("["):
            entries, closed = array_entries(val[1:])
            for e in entries:
                entry(key, e)
            if not closed:
                open_key = key
            continue
        v = unquote(val)
        if kind == "pyproject" and section == "tool.pyright" and pyright_loose(key, v):
            out.append((key, v, section))
        elif kind == "cargo" and lints_section(section) and v == "allow":
            out.append((key, v, section))
    return out


def golangci(lines):
    out = []
    stack = []  # (indent, key) of the `key:` lines that opened a block
    for raw in lines:
        trimmed = raw.lstrip()
        if not trimmed or trimmed.startswith("#"):
            continue
        indent = len(raw) - len(trimmed)
        body = trimmed.split(" #")[0].rstrip()
        if body == "-" or body.startswith("- "):
            while stack and stack[-1][0] > indent:
                stack.pop()
            item = body[1:].lstrip()
            hit = next(
                (k for _, k in reversed(stack)
                 if k == "disable" or k.startswith("exclude") or k == "exclusions"),
                None,
            )
            if hit and item:
                out.append((hit, item, ".".join(k for _, k in stack)))
            indent += len(body) - len(item)
            body = item
        else:
            while stack and stack[-1][0] >= indent:
                stack.pop()
        if ":" not in body:
            continue
        key, value = body.split(":", 1)
        key = key.strip().strip("\"'")
        value = value.strip()
        while stack and stack[-1][0] >= indent:
            stack.pop()
        if not value:
            stack.append((indent, key))
        elif (key == "disable-all" and value == "true") or (key == "default" and value == "none"):
            out.append((key, value, ".".join(k for _, k in stack)))
    return out


def tsconfig_loose(k, v):
    return (k.startswith("strict") or k == "noImplicitAny") and v == "false"


def eslint_loose(k, v):
    return k not in ("ecmaVersion", "version") and v in ("off", "warn", "0", "1")


# ---- reasons --------------------------------------------------------------------


def has_text(s):
    return bool(s.strip(" \t\r\n:-*/"))


def after_brackets(tail):
    if tail.startswith("["):
        return tail[1:].split("]", 1)[1] if "]" in tail else ""
    return tail


def after_noqa_codes(tail):
    rest = tail.lstrip()
    rest = rest[1:] if rest.startswith(":") else tail
    while True:
        t = rest.lstrip(", \t\r\n")
        m = re.match(r"[^,\s]+", t)
        if m and is_code(m.group(0)):
            rest = t[m.end():]
        else:
            return t


def starts_a_marker(seg):
    return any(after_prefix(seg, p) is not None for p in ("type:", "pyright:", "noqa", "ruff:"))


def python_reasoned(kind, line_text):
    comment = py_comment(line_text, {"triple": None})
    if comment is None:
        return False
    segs = [seg.strip() for seg in comment.split("#")]
    for i, seg in enumerate(segs):
        rest = None
        if kind in ("type: ignore", "pyright: ignore"):
            head = after_prefix(seg, "type:" if kind == "type: ignore" else "pyright:")
            tail = after_prefix(head.lstrip(), "ignore") if head is not None else None
            if tail is not None and boundary(tail):
                rest = after_brackets(tail)
        else:
            tail = after_prefix(seg, "noqa")
            if tail is not None and boundary(tail):
                rest = after_noqa_codes(tail)
        if rest is None:
            continue
        trimmed = rest.lstrip()
        if trimmed.startswith("--") and trimmed[2:].strip():
            return True
        if any(s and not starts_a_marker(s) for s in segs[i + 1:]):
            return True
    return False


def comment_bodies(line_text):
    return [
        (block, body.strip().lstrip("/*").strip())
        for _line, block, body in c_comments(line_text)
    ]


def rust_reasoned(line_text):
    _, found, after_word = line_text.partition("reason")
    if not found:
        return False
    after_word = after_word.lstrip()
    if not after_word.startswith("="):
        return False
    quoted = after_word[1:].lstrip()
    if not quoted.startswith('"'):
        return False
    return bool(quoted[1:].split('"')[0].strip())


def reasoned(marker, line_text):
    """Whether the marker carries a reason in its tool's own reason form, read
    from the text of the line it was found on. A configuration setting (line 0),
    a marker with no code, @ts-ignore, @ts-nocheck and a file-level ruff: noqa
    or pyright: comment never do."""
    kind, code, _section, line = marker
    if line == 0 or kind in ("@ts-ignore", "@ts-nocheck", "pyright", "ruff: noqa"):
        return False
    if kind == "@ts-expect-error":
        return any(
            body.startswith(kind) and boundary(body[len(kind):]) and has_text(body[len(kind):])
            for _block, body in comment_bodies(line_text)
        )
    if not code:
        return False
    if kind in ("type: ignore", "pyright: ignore", "noqa"):
        return python_reasoned(kind, line_text)
    if kind in ("allow", "expect"):
        return rust_reasoned(line_text)
    if kind == "nolint":
        for block, body in comment_bodies(line_text):
            if block or not body.startswith("nolint"):
                continue
            rest = body[len("nolint"):]
            rest = rest[1:] if rest.startswith(":") else rest
            parts = rest.lstrip().split(None, 1)
            if len(parts) == 2:
                tail = parts[1].lstrip()
                if tail.startswith("//") and tail[2:].strip():
                    return True
        return False
    if kind == "shellcheck disable":
        comment = shell_comment(line_text)
        parsed = shellcheck_disable(comment) if comment is not None else None
        if not parsed:
            return False
        after = parsed[1].lstrip()
        return after.startswith("#") and bool(after[1:].strip())
    if kind.startswith("eslint-disable"):
        for _block, body in comment_bodies(line_text):
            if body.startswith(kind) and boundary(body[len(kind):]):
                _, found, why = body[len(kind):].partition("--")
                if found and why.strip():
                    return True
        return False
    return False


def markers_at(name, text):
    """Normalised markers of `text` as (kind, code, section, line). Comment
    markers carry their 1-based line; a configuration setting carries 0. [] when
    the file type is not examined."""
    base = pathlib.PurePosixPath(name.replace("\\", "/")).name.lower()
    lines = [ln.rstrip("\r") for ln in text.split("\n")]

    def settings(found):
        return [(*m, 0) for m in found]

    if base.startswith("tsconfig") and base.endswith(".json"):
        return settings(json_lines(lines, tsconfig_loose))
    if base.startswith("eslint.config.") or base.startswith(".eslintrc"):
        return settings(json_lines(lines, eslint_loose))
    if base == "pyrightconfig.json":
        return settings(json_lines(lines, pyright_loose))
    if base == "pyproject.toml":
        return settings(toml(lines, "pyproject"))
    if base in ("ruff.toml", ".ruff.toml"):
        return settings(toml(lines, "ruff"))
    if base == "cargo.toml":
        return settings(toml(lines, "cargo"))
    if base in (".golangci.yml", ".golangci.yaml"):
        return settings(golangci(lines))
    ext = base.rsplit(".", 1)[-1] if "." in base.lstrip(".") else ""
    if ext in ("py", "pyi"):
        return python(lines)
    if ext in ("ts", "tsx", "js", "jsx", "mjs", "cjs", "mts", "cts", "vue", "svelte"):
        return script("\n".join(lines))
    if ext == "rs":
        return rust(lines)
    if ext == "go":
        return go("\n".join(lines))
    if ext in ("sh", "bash", "zsh") or (not ext and shell_by_shebang(base, text)):
        return shell(lines)
    return []


def markers(name, text):
    """Markers of `text` as (kind, code, section), the rule's identity."""
    return [m[:3] for m in markers_at(name, text)]


def added(name, before, after):
    """How many markers `after` has past the count `before` has."""
    had = Counter(markers(name, before))
    now = Counter(markers(name, after))
    return sum(max(0, n - had[k]) for k, n in now.items())


def net_added(changes):
    """The markers a whole change adds, net of the ones it removes anywhere.
    `changes` is a list of (old_path, before, new_path, after); each side is
    classified by its own path. Counts are summed over every file, keyed without
    the path and with whether the marker is reasoned. Returns
    (new_path, marker) in path then line order."""

    def keyed(marker, text_lines):
        line_text = text_lines[marker[3] - 1] if marker[3] > 0 else ""
        return (*marker[:3], reasoned(marker, line_text))

    had = Counter()
    for old_path, before, _new_path, _after in changes:
        text_lines = [ln.rstrip("\r") for ln in before.split("\n")]
        had.update(keyed(m, text_lines) for m in markers_at(old_path, before))
    now = []
    for _old_path, _before, new_path, after in changes:
        text_lines = [ln.rstrip("\r") for ln in after.split("\n")]
        now += [(new_path, m, keyed(m, text_lines)) for m in markers_at(new_path, after)]
    now.sort(key=lambda row: (row[0], row[1][3]))
    seen = Counter()
    out = []
    for path, marker, key in now:
        seen[key] += 1
        if seen[key] > had[key]:
            out.append((path, marker))
    return out


# ---- the fixture ----------------------------------------------------------------


def fixture_path():
    return pathlib.Path(__file__).resolve().parent.parent / "tests" / "fixtures" / "suppressions.txt"


def self_test():
    path = fixture_path()
    try:
        rows = path.read_text().splitlines()
    except OSError as err:
        print(f"suppression-rate: cannot read {path}: {err}", file=sys.stderr)
        return 1
    checked = 0
    failed = 0
    for n, row in enumerate(rows, 1):
        if not row.strip() or row.startswith("#"):
            continue
        parts = row.split("\t", 2)
        if len(parts) != 3 or parts[0] not in ("+", "-", "r", "b", "n+", "n-"):
            print(f"suppression-rate: {path.name}:{n}: malformed row", file=sys.stderr)
            failed += 1
            continue
        tag, name, sample = parts
        sample = sample.replace("\\n", "\n")
        if tag in ("+", "-"):
            got = added(name, "", sample) > 0
        elif tag in ("r", "b"):
            sample_lines = sample.split("\n")
            hits = net_added([("", "", name, sample)])
            flags = [reasoned(m, sample_lines[m[3] - 1]) for _path, m in hits]
            got = bool(flags) and all(f == (tag == "r") for f in flags)
        else:
            old, _, new = name.partition(">")
            before, found, after = sample.partition("@@")
            if not found:
                before, after = "", sample
            if not new:
                old = new = name
            got = bool(net_added([(old, before, new, after)]))
        checked += 1
        if got != (tag in ("+", "r", "b", "n+")):
            print(f"suppression-rate: {path.name}:{n}: tagged {tag}, got "
                  f"{'+' if got else '-'}: {row}", file=sys.stderr)
            failed += 1
    if checked < 40:
        print(f"suppression-rate: only {checked} samples in {path.name}", file=sys.stderr)
        failed += 1
    if failed:
        return 1
    print(f"self-test: {checked} samples agree")
    return 0


# ---- transcripts ----------------------------------------------------------------


def week_of(stamp):
    try:
        when = datetime.datetime.fromisoformat(stamp.replace("Z", "+00:00"))
    except ValueError:
        return None, None
    iso = when.isocalendar()
    return f"{iso[0]}-W{iso[1]:02d}", when.date()


def change_of(name, inp):
    """(before, after) of an Edit, MultiEdit or Write input, or None."""
    if name == "Write" and isinstance(inp.get("content"), str):
        return "", inp["content"]
    if name == "Edit" and isinstance(inp.get("old_string"), str) \
            and isinstance(inp.get("new_string"), str):
        return inp["old_string"], inp["new_string"]
    if name == "MultiEdit" and isinstance(inp.get("edits"), list):
        edits = [e for e in inp["edits"] if isinstance(e, dict)]
        return (
            "\n".join(str(e.get("old_string", "")) for e in edits),
            "\n".join(str(e.get("new_string", "")) for e in edits),
        )
    return None


def scan(root, until):
    calls = defaultdict(int)
    hits = defaultdict(int)
    # Counted, not dropped quietly: the total is the rule's evidence.
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
                if not isinstance(path, str):
                    continue
                change = change_of(block.get("name"), inp)
                if change and added(path, *change) > 0:
                    hits[week] += 1
    return calls, hits, skipped


def run(argv):
    default = pathlib.Path(
        os.environ.get("CLAUDE_CONFIG_DIR") or pathlib.Path.home() / ".claude"
    ) / "projects"
    ap = argparse.ArgumentParser(
        description="Weekly rate of Edit/Write calls adding a lint suppression "
                    "(transcript fragments only)."
    )
    ap.add_argument("--root", type=pathlib.Path, default=default,
                    help="transcripts directory (default: %(default)s)")
    ap.add_argument("--weeks", type=int, default=0,
                    help="only the last N weeks (default: all)")
    ap.add_argument("--until", type=datetime.date.fromisoformat, default=None,
                    metavar="YYYY-MM-DD", help="ignore calls after this day")
    ap.add_argument("--self-test", action="store_true",
                    help="classify tests/fixtures/suppressions.txt and exit")
    args = ap.parse_args(argv)

    if args.self_test:
        return self_test()
    if not args.root.is_dir():
        print(f"suppression-rate: no transcripts under {args.root}", file=sys.stderr)
        return 1
    calls, hits, skipped = scan(args.root, args.until)
    weeks = sorted(calls)
    if args.weeks > 0:
        weeks = weeks[-args.weeks:]
    print(f"{'week':<9} {'calls':>8} {'edits':>7} {'per 1000':>9}")
    for w in weeks:
        rate = 1000.0 * hits[w] / calls[w] if calls[w] else 0.0
        print(f"{w:<9} {calls[w]:>8} {hits[w]:>7} {rate:>9.2f}")
    total = sum(calls[w] for w in weeks)
    found = sum(hits[w] for w in weeks)
    rate = 1000.0 * found / total if total else 0.0
    print(f"{'total':<9} {total:>8} {found:>7} {rate:>9.2f}")
    if skipped["files"] or skipped["lines"]:
        print(f"suppression-rate: skipped {skipped['files']} unreadable "
              f"file(s), {skipped['lines']} malformed line(s)", file=sys.stderr)
    return 0


def main():
    try:
        code = run(sys.argv[1:])
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
