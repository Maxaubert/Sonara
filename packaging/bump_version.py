"""Set the release version in every version file and lockfile (#252).

Usage: python packaging/bump_version.py <major.minor.patch>
       python packaging/bump_version.py --check

The release version is Cargo.toml's [workspace.package] version (#248).
VERSION_FILES is the one list of files that carry it; tests/repo/test_manifests.py
reads it to check they all agree. LOCKFILES are the lockfile entries for the
repo's own packages, which follow the same version so `cargo build` and
`npm ci` need no extra step. Line endings are kept as they are.

--check prints the release version when every version file agrees, else
names the ones that differ and exits 1. release.yml reads the version this
way, and ci.yml runs the same command on every PR as its dry run.
"""
from __future__ import annotations

import re
import sys
from pathlib import Path
from typing import Dict, Iterable, List, Tuple

REPO_ROOT = Path(__file__).resolve().parent.parent

_SEMVER = re.compile(r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)")

# (path, pattern). Each pattern has three groups, prefix, version and suffix,
# and must match exactly once in its file.
VERSION_FILES: Tuple[Tuple[str, str], ...] = (
    ("Cargo.toml", r'(^\[workspace\.package\][^\[]*?^version = ")([^"]+)(")'),
    ("bin/runtime-version", r"(\A)([^\r\n]+)(\r?\n\Z)"),
    (".claude-plugin/plugin.json", r'(^  "version": ")([^"]+)(")'),
    (".claude-plugin/marketplace.json", r'(^      "version": ")([^"]+)(")'),
    ("clients/ts/package.json", r'(^  "version": ")([^"]+)(")'),
    ("clients/ts/src/version.ts", r'(^export const VERSION = ")([^"]+)(";)'),
    ("clients/player/package.json", r'(^  "version": ")([^"]+)(")'),
    ("packaging/npm-runtime/package.json", r'(^  "version": ")([^"]+)(")'),
    ("clients/python/pyproject.toml", r'(^\[project\][^\[]*?^version = ")([^"]+)(")'),
    ("clients/python/src/sonara_client/version.py", r'(^__version__ = ")([^"]+)(")'),
)

# (path, pattern, minimum matches). Same groups; every match is replaced.
LOCKFILES: Tuple[Tuple[str, str, int], ...] = (
    # Workspace crates: a [[package]] whose next lines are name and version
    # with no `source` line (registry and git crates always have one).
    ("Cargo.lock", r'(^name = "[^"]+"\r?\nversion = ")([^"]+)("\r?\n(?!source = ))', 1),
    # The package itself: the top level and the packages[""] entry.
    ("clients/ts/package-lock.json", r'("name": "@sonara/client",\r?\n\s*"version": ")([^"]+)(")', 2),
    ("clients/player/package-lock.json", r'("name": "@sonara/player",\r?\n\s*"version": ")([^"]+)(")', 2),
)

_FLAGS = re.M | re.S

# The release version (release.yml tags v<this>).
RELEASE_SOURCE = "Cargo.toml"


def all_paths() -> List[str]:
    return [p for p, _ in VERSION_FILES] + [p for p, _, _ in LOCKFILES]


def _read(root: Path, rel: str) -> str:
    # newline="" keeps CRLF or LF as the checkout has it.
    with open(root / rel, encoding="utf-8", newline="") as f:
        return f.read()


def _write(root: Path, rel: str, text: str) -> None:
    with open(root / rel, "w", encoding="utf-8", newline="") as f:
        f.write(text)


def read_versions(root: Path = REPO_ROOT) -> Dict[str, str]:
    """The version each VERSION_FILES entry declares, by path."""
    out = {}
    for rel, pattern in VERSION_FILES:
        matches = list(re.finditer(pattern, _read(root, rel), _FLAGS))
        if len(matches) != 1:
            raise ValueError(f"{rel}: expected one version, found {len(matches)}")
        out[rel] = matches[0].group(2)
    return out


def release_version(root: Path = REPO_ROOT) -> str:
    """Cargo.toml's [workspace.package] version, once every version file agrees."""
    versions = read_versions(root)
    release = versions[RELEASE_SOURCE]
    differ = {rel: v for rel, v in versions.items() if v != release}
    if differ:
        raise ValueError(f"{RELEASE_SOURCE} is {release}, but {differ}: run bump_version.py {release}")
    return release


def _replace(text: str, rel: str, pattern: str, version: str, expect: Iterable[int]) -> str:
    new, n = re.subn(pattern, lambda m: m.group(1) + version + m.group(3), text, flags=_FLAGS)
    if n not in expect:
        raise ValueError(f"{rel}: pattern matched {n} times")
    return new


def bump(root: Path, version: str) -> None:
    """Write `version` into every file, or change nothing if any check fails."""
    if not _SEMVER.fullmatch(version):
        raise ValueError(f"not a major.minor.patch version: {version!r}")
    current = read_versions(root)
    if len(set(current.values())) != 1:
        raise ValueError(f"version files disagree, fix them first: {current}")
    old = next(iter(current.values()))
    updated = {}
    for rel, pattern in VERSION_FILES:
        updated[rel] = _replace(_read(root, rel), rel, pattern, version, (1,))
    for rel, pattern, minimum in LOCKFILES:
        text = _read(root, rel)
        # Only entries that carry the current release version move, so a
        # vendored crate with its own version is left alone.
        scoped = pattern.replace("([^\"]+)", "(" + re.escape(old) + ")", 1)
        n = len(re.findall(scoped, text, _FLAGS))
        if n < minimum:
            raise ValueError(f"{rel}: expected at least {minimum} entries at {old}, found {n}")
        updated[rel] = _replace(text, rel, scoped, version, (n,))
    for rel, text in updated.items():
        _write(root, rel, text)


def main(argv: List[str]) -> int:
    if len(argv) != 1:
        print("\n".join(__doc__.strip().splitlines()[2:4]), file=sys.stderr)
        return 2
    if argv[0] == "--check":
        try:
            print(release_version(REPO_ROOT))
        except ValueError as e:
            print(f"bump_version: {e}", file=sys.stderr)
            return 1
        return 0
    try:
        old = read_versions(REPO_ROOT)[RELEASE_SOURCE]
        bump(REPO_ROOT, argv[0])
    except ValueError as e:
        print(f"bump_version: {e}", file=sys.stderr)
        return 1
    print(f"{old} -> {argv[0]} in {len(all_paths())} files")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
