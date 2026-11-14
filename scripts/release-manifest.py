#!/usr/bin/env python3
"""Write the release's copy of greenharness.toml, with [tool.release].

grh pins the manifest a release publishes (grh docs/extension-manifest.md,
"Shipping a manifest"). The committed greenharness.toml carries no sums: the
tarballs do not exist when it is committed. This script, run by the release
after the archives are built and BEFORE SHA256SUMS is computed, writes a copy
that adds

    [tool.release]
    repo = "greenharness/greenpiste"
    version = "<version>"
    [tool.release.sha256]
    <target> = "<sha256 of greenpiste-<version>-<target>.tar.gz>"

for each unix target grh installs, so the published manifest is checksummed
and attested with the archives it describes.

It refuses, with exit 1, anything that would publish a manifest grh then
refuses: a version that is not the manifest's [extension] version, a
manifest that already carries [tool.release], a missing tarball, or an
output that does not read back with the same values.

The amont-agent-* archives are the alias copies: never read here.

Usage: release-manifest.py <version> <dist-dir> <greenharness.toml> <out>
Everything it says goes to stderr; it writes only the <out> file.
Python 3.11 or later (tomllib); run it as `python3 -I`.
"""

import hashlib
import pathlib
import re
import sys
import tomllib
from typing import NoReturn

REPO = "greenharness/greenpiste"
USAGE = "usage: release-manifest.py <version> <dist-dir> <greenharness.toml> <out>"
# The targets grh downloads: the unix tarballs. Windows ships a .zip that
# grh does not install.
TARGETS = (
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "aarch64-unknown-linux-gnu",
    "x86_64-unknown-linux-gnu",
)


def fail(message: str) -> NoReturn:
    print(f"release-manifest: {message}", file=sys.stderr)
    sys.exit(1)


def sha256_of(path: pathlib.Path) -> str:
    try:
        with path.open("rb") as f:
            return hashlib.file_digest(f, "sha256").hexdigest()
    except OSError as e:
        fail(f"cannot read {path}: {e}")


def main() -> None:
    if len(sys.argv) != 5:
        print(USAGE, file=sys.stderr)
        sys.exit(2)
    version = sys.argv[1].removeprefix("v")
    dist = pathlib.Path(sys.argv[2])
    source = pathlib.Path(sys.argv[3])
    out = pathlib.Path(sys.argv[4])

    try:
        text = source.read_text(encoding="utf-8")
    except OSError as e:
        fail(f"cannot read {source}: {e}")
    try:
        manifest = tomllib.loads(text)
    except tomllib.TOMLDecodeError as e:
        fail(f"{source} is not TOML: {e}")
    extension = manifest.get("extension")
    if not isinstance(extension, dict):
        fail(f"{source} has no [extension] table")
    declared = extension.get("version")
    if declared != version:
        fail(f"{source} declares [extension] version {declared!r}, the release is {version!r}")
    tool = manifest.get("tool", {})
    if isinstance(tool, dict) and "release" in tool:
        fail(f"{source} already carries [tool.release]; the committed manifest holds no sums")

    # The old binary name, kept as a copy until the census: grh places it
    # from this declared list, so the committed manifest must keep it.
    aliases = tool.get("bin_aliases") if isinstance(tool, dict) else None
    if (
        not isinstance(aliases, list)
        or not aliases
        or not all(isinstance(a, str) and re.fullmatch(r"[A-Za-z0-9._-]+", a) for a in aliases)
    ):
        fail(f"{source} must declare [tool] bin_aliases as a list of plain binary names")

    sums = {}
    for target in TARGETS:
        tarball = dist / f"greenpiste-{version}-{target}.tar.gz"
        if not tarball.is_file():
            fail(f"no {tarball.name} in {dist}")
        sums[target] = sha256_of(tarball)

    block = [
        "",
        "# Written by scripts/release-manifest.py from the release's own tarballs.",
        "[tool.release]",
        f'repo = "{REPO}"',
        f'version = "{version}"',
        "[tool.release.sha256]",
    ]
    block += [f'{target} = "{sums[target]}"' for target in TARGETS]
    written = text.rstrip("\n") + "\n" + "\n".join(block) + "\n"

    back = tomllib.loads(written).get("tool", {}).get("release", {})
    if back != {"repo": REPO, "version": version, "sha256": sums}:
        fail(f"the written manifest does not read back as written: {back!r}")

    try:
        out.write_text(written, encoding="utf-8")
    except OSError as e:
        fail(f"cannot write {out}: {e}")
    print(f"release-manifest: {out} for {version} ({len(sums)} targets)", file=sys.stderr)


if __name__ == "__main__":
    main()
