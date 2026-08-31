#!/bin/sh
# Install the amont-agent binary. Nothing else.
#
#   curl -fsSL https://raw.githubusercontent.com/fredericrous/amont-agent/main/install/install.sh | sh
#
# This script deliberately does NOT wire the guard into Claude Code. It
# downloads a verified binary, puts it on your PATH, and tells you what to run
# next. That restraint is the point: a program that can REFUSE the commands
# your agent runs should not install itself into your settings.json as a side
# effect of you fetching it. `amont-agent install --write` is a separate,
# deliberate act, and it prints the block before it writes anything.
#
# POSIX sh, no bashisms: this has to run wherever curl does.
set -eu

REPO="fredericrous/amont-agent"
# `$HOME/.local/bin` by default. `amont-agent install` writes the ABSOLUTE
# path of this binary into settings.json, so the directory matters only for
# whether you can type the command yourself.
BIN_DIR="${AMONT_AGENT_BIN_DIR:-$HOME/.local/bin}"
VERSION="${AMONT_AGENT_VERSION:-latest}"

RED='\033[31m'; GREEN='\033[32m'; YELLOW='\033[33m'; OFF='\033[0m'
if [ -n "${NO_COLOR:-}" ] || [ ! -t 1 ]; then RED=''; GREEN=''; YELLOW=''; OFF=''; fi

say()  { printf '  %s\n' "$1"; }
ok()   { printf "  ${GREEN}✓${OFF} %s\n" "$1"; }
warn() { printf "  ${YELLOW}!${OFF} %s\n" "$1"; }
die()  { printf "  ${RED}✗${OFF} %s\n" "$1" >&2; exit 1; }

need() { command -v "$1" > /dev/null 2>&1 || die "$1 is required and was not found"; }

need uname
need tar

# curl or wget, whichever is here.
if command -v curl > /dev/null 2>&1; then
    fetch() { curl -fsSL "$1"; }
    fetch_to() { curl -fsSL "$1" -o "$2"; }
elif command -v wget > /dev/null 2>&1; then
    fetch() { wget -qO- "$1"; }
    fetch_to() { wget -qO "$2" "$1"; }
else
    die "neither curl nor wget is available"
fi

target() {
    os=$(uname -s)
    arch=$(uname -m)
    case "$os" in
        Linux)
            # musl when there is no glibc: the static build runs on distros
            # older than whatever the release was built against, which is the
            # usual reason a "linux" binary fails for somebody.
            if ldd --version 2>&1 | grep -qi musl; then libc=musl; else libc=gnu; fi
            case "$arch" in
                x86_64|amd64)  echo "x86_64-unknown-linux-$libc" ;;
                aarch64|arm64) [ "$libc" = "musl" ] && die "no aarch64 musl build yet — build from source with cargo install amont-agent"
                               echo "aarch64-unknown-linux-gnu" ;;
                *) die "unsupported architecture: $arch" ;;
            esac
            ;;
        Darwin)
            case "$arch" in
                x86_64) echo "x86_64-apple-darwin" ;;
                arm64)  echo "aarch64-apple-darwin" ;;
                *) die "unsupported architecture: $arch" ;;
            esac
            ;;
        MINGW*|MSYS*|CYGWIN*)
            # Reachable: this runs under Git Bash, which every Git for Windows
            # install ships. Point at the PowerShell installer rather than the
            # releases page, because there IS a one-liner for this platform.
            die "on Windows, use PowerShell:
    irm https://raw.githubusercontent.com/$REPO/main/install/install.ps1 | iex
  or download the .zip from https://github.com/$REPO/releases"
            ;;
        *) die "unsupported OS: $os" ;;
    esac
}

resolve_version() {
    if [ "$VERSION" != "latest" ]; then
        echo "${VERSION#v}"
        return
    fi
    # The API rather than the /releases/latest redirect, so a rate-limited or
    # offline run fails LOUDLY here instead of downloading a 404 page and
    # handing you a tarball full of HTML.
    tag=$(fetch "https://api.github.com/repos/$REPO/releases/latest" \
        | sed -n 's/.*"tag_name" *: *"\([^"]*\)".*/\1/p' | head -n 1)
    [ -n "$tag" ] || die "could not determine the latest release (rate limited? set AMONT_VERSION=vX.Y.Z)"
    echo "${tag#v}"
}

main() {
    printf '\n  amont-agent installer\n\n'

    t=$(target)
    v=$(resolve_version)
    name="amont-agent-${v}-${t}"
    base="https://github.com/$REPO/releases/download/v${v}"

    say "version:  $v"
    say "platform: $t"
    say "into:     $BIN_DIR"
    printf '\n'

    tmp=$(mktemp -d)
    # Clean up on the way out however we leave, including Ctrl-C.
    trap 'rm -rf "$tmp"' EXIT INT TERM

    say "downloading…"
    fetch_to "$base/${name}.tar.gz" "$tmp/${name}.tar.gz" \
        || die "download failed: $base/${name}.tar.gz"

    # Checksums are not optional here. This binary runs on every commit with
    # your credentials and reads every staged file; verifying what it is before
    # putting it in that position is the whole argument the project makes about
    # its own dependencies, applied to itself.
    if fetch_to "$base/SHA256SUMS" "$tmp/SHA256SUMS" 2> /dev/null; then
        if command -v sha256sum > /dev/null 2>&1; then
            got=$(sha256sum "$tmp/${name}.tar.gz" | cut -d' ' -f1)
        elif command -v shasum > /dev/null 2>&1; then
            got=$(shasum -a 256 "$tmp/${name}.tar.gz" | cut -d' ' -f1)
        else
            got=""
        fi
        if [ -n "$got" ]; then
            want=$(grep " ${name}.tar.gz\$" "$tmp/SHA256SUMS" | cut -d' ' -f1 | head -n 1)
            [ -n "$want" ] || die "SHA256SUMS has no entry for ${name}.tar.gz"
            [ "$got" = "$want" ] || die "checksum mismatch — refusing to install
    expected $want
    got      $got"
            ok "checksum verified"
        else
            warn "no sha256 tool found — the download was NOT verified"
        fi
    else
        warn "no SHA256SUMS published for this release — the download was NOT verified"
    fi

    tar xzf "$tmp/${name}.tar.gz" -C "$tmp"
    # Whether this is an upgrade decides what to say at the end: the
    # first-install steps are wrong advice the second time.
    upgrading=0
    [ -x "$BIN_DIR/amont-agent" ] && upgrading=1
    mkdir -p "$BIN_DIR"
    b=amont-agent
    if [ -f "$tmp/$name/$b" ]; then
        # Write to a temporary name and rename over the destination:
        # replacing a RUNNING binary in place fails on some platforms, and
        # rename is atomic, so a half-copied amont-agent never exists.
        cp "$tmp/$name/$b" "$BIN_DIR/.$b.new"
        chmod 755 "$BIN_DIR/.$b.new"
        mv "$BIN_DIR/.$b.new" "$BIN_DIR/$b"
        ok "installed $BIN_DIR/$b"
    fi

    printf '\n'
    case ":$PATH:" in
        *":$BIN_DIR:"*) ;;
        *) warn "$BIN_DIR is not on your PATH — add it, or you will not be able to run amont-agent yourself" ;;
    esac

    if [ "$upgrading" -eq 1 ]; then
        printf '  Upgraded. The settings.json entry points at this path, so\n'
        printf '  nothing needs rewiring. Confirm it still fires:\n\n'
        printf '    amont-agent doctor\n\n'
    else
        printf '  Installed, and wired into nothing. The guard does not run until\n'
        printf '  Claude Code is told about it:\n\n'
        printf '    amont-agent install          # print the settings block, write nothing\n'
        printf '    amont-agent install --write  # merge it into ~/.claude/settings.json\n'
        printf '    amont-agent doctor           # installed, runnable, and actually firing?\n\n'
        printf '  Every rule but pipe-to-tail ships as observe, so nothing is\n'
        printf '  refused until you have seen what it would have caught:\n\n'
        printf '    amont-agent status\n\n'
    fi
}

main "$@"
