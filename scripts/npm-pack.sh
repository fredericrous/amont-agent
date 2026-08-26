#!/usr/bin/env bash
# Lay out the seven npm packages from release artifacts already built.
#
#   scripts/npm-pack.sh <version> <artifact-dir> [out-dir]
#
# `<artifact-dir>` is the directory the release workflow downloads into: one
# `amont-agent-<version>-<target>.tar.gz` (or `.zip`) per target, exactly as
# `release.yaml`'s `Archive` step produced them. Nothing is compiled here — the
# bytes published to npm are the SAME bytes published to the GitHub release and
# checksummed in `SHA256SUMS`, which is the property that makes the npm path
# worth trusting at all.
#
# Publishing is a separate step on purpose, so the layout can be inspected —
# and `npm pack`ed — before anything irreversible happens. npm versions are
# immutable.
set -euo pipefail

VERSION="${1:?usage: npm-pack.sh <version> <artifact-dir> [out-dir]}"
ARTIFACTS="${2:?usage: npm-pack.sh <version> <artifact-dir> [out-dir]}"
OUT="${3:-dist/npm}"

HERE=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

# target | npm package | os | cpu | libc
#
# The six `release.yaml` builds, and no more. aarch64-musl and aarch64-windows
# are deliberately absent: neither is built upstream, and a package that
# resolves to nothing is worse than one npm skips — it installs, then fails the
# first time a session opens. `install.sh` refuses the same pair for the same
# reason.
#
# THIS TABLE IS THE PUBLISH ORDER'S SOURCE OF TRUTH. `release.yaml` names the
# same six suffixes in an explicit loop rather than globbing `amont-agent-*`,
# because that glob would also match the root package `amont-agent` and sort it
# FIRST — publishing it before the platform packages it optionally depends on.
# If you add a target here, add it there.
TARGETS=(
    "aarch64-apple-darwin|amont-agent-darwin-arm64|darwin|arm64|"
    "x86_64-apple-darwin|amont-agent-darwin-x64|darwin|x64|"
    "aarch64-unknown-linux-gnu|amont-agent-linux-arm64-gnu|linux|arm64|glibc"
    "x86_64-unknown-linux-gnu|amont-agent-linux-x64-gnu|linux|x64|glibc"
    "x86_64-unknown-linux-musl|amont-agent-linux-x64-musl|linux|x64|musl"
    "x86_64-pc-windows-msvc|amont-agent-win32-x64|win32|x64|"
)

say() { printf '  %s\n' "$1"; }
die() { printf '  ✗ %s\n' "$1" >&2; exit 1; }

command -v tar > /dev/null 2>&1 || die "tar is required"

rm -rf "$OUT"
mkdir -p "$OUT"

# --- the six platform packages -------------------------------------------

for row in "${TARGETS[@]}"; do
    IFS='|' read -r target pkg os cpu libc <<< "$row"

    name="amont-agent-${VERSION}-${target}"
    suffix=
    [ "$os" = "win32" ] && suffix=.exe
    exe="amont-agent$suffix"

    # Unpack into a scratch dir rather than reading the archive twice. A
    # missing archive is fatal: publishing five of six packages would leave the
    # root package's optionalDependencies naming a version that does not exist,
    # and npm rejects the whole install rather than skipping it.
    scratch="$OUT/.unpack/$target"
    mkdir -p "$scratch"
    if [ -f "$ARTIFACTS/$name.tar.gz" ]; then
        tar xzf "$ARTIFACTS/$name.tar.gz" -C "$scratch"
    elif [ -f "$ARTIFACTS/$name.zip" ]; then
        command -v unzip > /dev/null 2>&1 || die "unzip is required for $name.zip"
        unzip -q "$ARTIFACTS/$name.zip" -d "$scratch"
    else
        die "no archive for $target (looked for $ARTIFACTS/$name.tar.gz and .zip)"
    fi

    mkdir -p "$OUT/$pkg/bin"
    src="$scratch/$name/$exe"
    [ -f "$src" ] || die "$name archive holds no $exe"
    cp "$src" "$OUT/$pkg/bin/$exe"
    # npm preserves the mode of files in the tarball; a binary that arrives
    # without +x fails at the first session with "permission denied".
    chmod 0755 "$OUT/$pkg/bin/$exe"

    # `libc` is a real npm/pnpm resolution field, and the ONLY thing separating
    # the two linux-x64 packages. Without it both match a linux/x64 host and
    # the resolver picks one at random — half the time the glibc build on
    # Alpine, which cannot exec at all.
    if [ -n "$libc" ]; then
        libc_block=$(printf '  "libc": [\n    "%s"\n  ],' "$libc")
    else
        libc_block=""
    fi

    python3 - "$HERE/npm/platform/package.json.in" "$OUT/$pkg/package.json" \
        "$pkg" "$VERSION" "$os" "$cpu" "$libc_block" <<'PY'
import sys
src, dst, pkg, version, os_, cpu, libc = sys.argv[1:8]
text = open(src).read()
for token, value in (("__PKG__", pkg), ("__VERSION__", version),
                     ("__OS__", os_), ("__CPU__", cpu)):
    text = text.replace(token, value)
# The libc line is whole-line: an empty substitution must take the newline with
# it, or the JSON keeps a blank line where a key was.
text = "\n".join(l for l in text.replace("__LIBC__", libc).split("\n")
                 if l.strip() != "")
open(dst, "w").write(text + "\n")
PY

    say "$pkg  ($target)"
done

rm -rf "$OUT/.unpack"

# --- the package people depend on ----------------------------------------

mkdir -p "$OUT/amont-agent/bin"
for f in amont-agent.js native.js; do
    cp "$HERE/npm/amont-agent/bin/$f" "$OUT/amont-agent/bin/$f"
done
chmod 0755 "$OUT/amont-agent/bin/amont-agent.js"
cp "$HERE/README.md" "$OUT/amont-agent/README.md"
cp "$HERE/LICENSE" "$OUT/amont-agent/LICENSE"
sed "s/__VERSION__/$VERSION/g" "$HERE/npm/amont-agent/package.json.in" > "$OUT/amont-agent/package.json"
say "amont-agent"

# --- prove it before anybody publishes it --------------------------------
#
# Every failure below is one that cannot be fixed after the fact: npm versions
# are immutable, so a package published without its binary can only be
# superseded, never replaced.

for row in "${TARGETS[@]}"; do
    IFS='|' read -r _ pkg os _ _ <<< "$row"
    suffix=
    [ "$os" = "win32" ] && suffix=.exe
    [ -x "$OUT/$pkg/bin/amont-agent$suffix" ] || die "$pkg/bin/amont-agent$suffix missing or not executable"
    python3 -c "import json,sys; json.load(open(sys.argv[1]))" "$OUT/$pkg/package.json" \
        || die "$pkg/package.json is not valid JSON"
done
python3 -c "import json,sys; json.load(open(sys.argv[1]))" "$OUT/amont-agent/package.json" \
    || die "amont-agent/package.json is not valid JSON"

# The dependency versions must match the packages actually laid out beside them.
python3 - "$OUT" "$VERSION" <<'PY'
import json, os, sys
out, version = sys.argv[1], sys.argv[2]
main = json.load(open(os.path.join(out, "amont-agent", "package.json")))
missing = []
for dep, want in main.get("optionalDependencies", {}).items():
    path = os.path.join(out, dep, "package.json")
    if not os.path.exists(path):
        missing.append(f"{dep}: declared, not built")
        continue
    have = json.load(open(path))["version"]
    if have != want or want != version:
        missing.append(f"{dep}: declares {want}, package is {have}, release is {version}")
if missing:
    sys.exit("  ✗ " + "\n  ✗ ".join(missing))
PY

# The root package must not be mistakable for a platform package by a glob.
# `release.yaml` publishes an explicit list precisely because it is; this
# asserts the shape that made the explicit list necessary, so anybody who
# "simplifies" it back to a glob has been told why not.
if [ -d "$OUT/amont-agent" ]; then
    matched=$(cd "$OUT" && ls -d amont-agent-* 2>/dev/null | wc -l | tr -d ' ')
    [ "$matched" -eq 6 ] || die "expected 6 platform dirs matching amont-agent-*, found $matched"
    say ""
    say "NOTE: \`amont-agent-*\` matches the six platform packages; the root"
    say "package \`amont-agent\` sorts BEFORE them. Publish the six FIRST."
fi

say ""
say "laid out $VERSION in $OUT — publish the six platform packages BEFORE"
say "amont-agent, or npm rejects it for an optionalDependency that does not"
say "exist yet, permanently."
