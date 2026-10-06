"""Run the gates a change needs, the way CI runs them (#256).

Usage: python packaging/gate.py [--all] [--quick] [--base REF] [--dry-run] [--keep-going]

The changed paths are the commits since the merge base with --base (default
origin/main) plus the working tree (staged, unstaged and untracked). Each
gate runs when a changed path starts with one of its triggers (GATES):

  rust         fmt, clippy, tests (cargo nextest when installed, else cargo test)
  deps         cargo deny and THIRD_PARTY_NOTICES.md (a dependency changed)
  conformance  protocol v1 and plugin conformance against the debug build
  python       ruff, the version check and tests/repo (always, when anything changed)
  e2e          the settings-page browser tests (fails without the e2e dependency group)
  sdk          the SDKs, the player demo, the npm runtime package and the smoke hosts
  embed        the embedder e2e suite (tests/embed, #274): Node, Python and raw
               protocol host apps on the packed packages, the wheel and the
               release zip, with the audio checked through --output wav:<dir>
  earcons      the bundled earcon WAVs match packaging/sounds

--all runs every gate except earcons (it needs numpy and scipy); --quick
limits clippy and the tests to the changed crates and every crate that
depends on them. Conformance and e2e use target/debug (SONARAD and
SONARA_HOOK point there), so a release build of the SDK gate never shadows
them. Works from PowerShell or Git Bash; needs ~/.cargo/bin on PATH.

REQUIRED_CHECKS are the ci.yml job names that branch protection requires;
tests/repo/test_gate.py keeps the two in step.
"""
from __future__ import annotations

import argparse
import fnmatch
import importlib.util
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path
from typing import Dict, Iterable, List, Optional, Sequence, Tuple

REPO_ROOT = Path(__file__).resolve().parent.parent


def _load_bump_version():
    spec = importlib.util.spec_from_file_location("bump_version", REPO_ROOT / "packaging" / "bump_version.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


bump_version = _load_bump_version()

# The ci.yml jobs every PR must pass (branch protection, #250).
REQUIRED_CHECKS = ("check", "rust", "conformance", "clients", "deny")

# Files every crate builds with: a change here tests the whole workspace.
WORKSPACE_FILES = ("Cargo.toml", "Cargo.lock", ".cargo/", "rust-toolchain.toml", "rustfmt.toml",
                   "clippy.toml", ".config/nextest.toml", "tests/fixtures/")
RUST = ("crates/",) + WORKSPACE_FILES
DEPS = ("Cargo.toml", "crates/*/Cargo.toml", "Cargo.lock", "deny.toml", "packaging/notices/")
CONFORMANCE = RUST + ("conformance/", "bin/", "hooks/", "packaging/release_zip.py",
                      "packaging/runtime_dlls.py")
E2E = ("crates/sonarad/assets/settings/", "crates/sonarad/src/settings_page.rs",
       "crates/sonarad/src/config.rs", "tests/e2e/")
SDK = ("clients/", "packaging/npm-runtime/", "packaging/smoke/", "examples/") + tuple(bump_version.all_paths())
# What an app that embeds Sonara runs: the runtime and the crates it speaks
# with, the SDKs and packages, and the docs the raw host is written from.
EMBED = ("clients/", "packaging/npm-runtime/", "packaging/release_zip.py", "packaging/runtime_dlls.py",
         "tests/embed/", "docs/protocol-v1.md", "docs/bundling.md", "crates/sonarad/", "crates/sonara-reader/",
         "crates/sonara-engine/", "crates/sonara-audio/") + tuple(bump_version.all_paths())
EARCONS = ("crates/sonara-agent/sounds/", "packaging/sounds/")

PLAYWRIGHT_CHECK = ("import importlib.util, sys; sys.exit(0 if importlib.util.find_spec('playwright') "
                    "else 'playwright is missing, so tests/e2e would skip every test: "
                    "python -m pip install --group e2e; python -m playwright install chromium')")

# In the order they run: the fast and likely-to-fail gates first.
GATES: Tuple[Tuple[str, Tuple[str, ...]], ...] = (
    ("rust", RUST),
    ("deps", DEPS),
    ("conformance", CONFORMANCE),
    ("python", ("",)),
    ("e2e", E2E),
    ("sdk", SDK),
    ("embed", EMBED),
    ("earcons", EARCONS),
)


def _matches(path: str, trigger: str) -> bool:
    if "*" in trigger:
        return fnmatch.fnmatchcase(path, trigger)
    if trigger.endswith("/") or trigger == "":
        return path.startswith(trigger)
    return path == trigger


def select(paths: Iterable[str]) -> List[str]:
    """The gates the changed ``paths`` (repo-relative, forward slashes) need."""
    paths = list(paths)
    if not paths:
        return []
    return [name for name, triggers in GATES
            if any(_matches(p, t) for p in paths for t in triggers)]


def affected_crates(paths: Iterable[str], metadata: dict) -> Optional[List[str]]:
    """The workspace crates a quick run tests: each crate a path is in and,
    transitively, every crate that depends on one. None means the whole
    workspace (a shared file changed)."""
    paths = list(paths)
    if any(_matches(p, t) and not p.startswith("crates/") for p in paths for t in WORKSPACE_FILES):
        return None
    members = set(metadata["workspace_members"])
    root = Path(metadata["workspace_root"])
    dirs: Dict[str, str] = {}
    deps: Dict[str, List[str]] = {}
    for pkg in metadata["packages"]:
        if pkg["id"] not in members:
            continue
        rel = Path(pkg["manifest_path"]).parent.relative_to(root).as_posix()
        dirs[pkg["name"]] = rel + "/"
        deps[pkg["name"]] = [d["name"] for d in pkg["dependencies"]]
    hit = {name for name, d in dirs.items() if any(p.startswith(d) for p in paths)}
    grew = True
    while grew:
        more = {name for name, ds in deps.items() if name not in hit and hit.intersection(ds)}
        hit |= more
        grew = bool(more)
    return sorted(hit)


def _git(*args: str) -> List[str]:
    out = subprocess.run(["git", *args], cwd=REPO_ROOT, check=True, capture_output=True, text=True).stdout
    return [line.strip() for line in out.splitlines() if line.strip()]


def changed_paths(base: str) -> List[str]:
    merge_base = _git("merge-base", base, "HEAD")[0]
    paths = set(_git("diff", "--name-only", merge_base))
    paths.update(_git("ls-files", "--others", "--exclude-standard"))
    return sorted(paths)


def _cargo_metadata() -> dict:
    out = subprocess.run(["cargo", "metadata", "--format-version", "1", "--no-deps"], cwd=REPO_ROOT,
                         check=True, capture_output=True, text=True).stdout
    return json.loads(out)


def _has_nextest() -> bool:
    try:
        return subprocess.run(["cargo", "nextest", "--version"], cwd=REPO_ROOT,
                              capture_output=True).returncode == 0
    except OSError:
        return False


Step = Tuple[str, List[str], Optional[str], Dict[str, str]]


def _step(label: str, argv: Sequence[str], cwd: Optional[str] = None,
          env: Optional[Dict[str, str]] = None) -> Step:
    return (label, list(argv), cwd, dict(env or {}))


def _debug_env() -> Dict[str, str]:
    debug = REPO_ROOT / "target" / "debug"
    return {"SONARAD": str(debug / "sonarad.exe"), "SONARA_HOOK": str(debug / "sonara-hook.exe")}


def steps_for(gate: str, crates: Optional[List[str]], nextest: bool) -> List[Step]:
    py = sys.executable
    scope = ["--workspace"] if crates is None else [a for c in crates for a in ("-p", c)]
    if gate == "rust":
        if crates == []:
            return [_step("cargo fmt", ["cargo", "fmt", "--all", "--", "--check"])]
        tests = ([_step("cargo nextest", ["cargo", "nextest", "run", *scope]),
                  _step("cargo test --doc", ["cargo", "test", *scope, "--doc"])] if nextest
                 else [_step("cargo test", ["cargo", "test", *scope])])
        return [_step("cargo fmt", ["cargo", "fmt", "--all", "--", "--check"]),
                _step("cargo clippy", ["cargo", "clippy", *scope, "--all-targets", "--", "-D", "warnings"]),
                *tests]
    if gate == "deps":
        return [_step("cargo deny", ["cargo", "deny", "check", "licenses", "bans"]),
                _step("notices", [py, "packaging/notices/gen_notices.py", "--check"])]
    if gate == "conformance":
        return [_step("cargo build (debug)", ["cargo", "build", "-p", "sonarad", "-p", "sonara-hook", "-p", "sonara-cli"]),
                _step("conformance", [py, "-m", "pytest", "conformance", "-q"], env=_debug_env())]
    if gate == "python":
        return [_step("ruff", [py, "-m", "ruff", "check", "tests", "conformance", "clients/python", "packaging"]),
                _step("version check", [py, "packaging/bump_version.py", "--check"]),
                _step("tests/repo", [py, "-m", "pytest", "tests/repo", "-q"])]
    if gate == "e2e":
        # tests/e2e skips every test without playwright, which would pass the gate unrun.
        return [_step("playwright installed", [py, "-c", PLAYWRIGHT_CHECK]),
                _step("cargo build sonarad (debug)", ["cargo", "build", "-p", "sonarad"]),
                _step("tests/e2e", [py, "-m", "pytest", "tests/e2e", "-q"], env=_debug_env())]
    if gate == "sdk":
        npm = shutil.which("npm") or "npm"
        node = shutil.which("node") or "node"
        exe = str(REPO_ROOT / "target" / "release" / "sonarad.exe")
        release = {"SONARAD": exe}
        # The Python host imports sonara_client the way an app does; CI pip-installs it.
        host = {"SONARA_RUNTIME": exe, "PYTHONPATH": str(REPO_ROOT / "clients" / "python" / "src")}
        out = [_step("cargo build sonarad (release)", ["cargo", "build", "-p", "sonarad", "--release"]),
               _step("stage runtime DLLs", [py, "packaging/runtime_dlls.py", "stage", "target/release"])]
        for where in ("clients/ts", "clients/player"):
            out += [_step(where + ": npm ci", [npm, "ci"], where),
                    _step(where + ": typecheck", [npm, "run", "typecheck"], where),
                    _step(where + ": build", [npm, "run", "build"], where),
                    _step(where + ": test", [npm, "test"], where, release)]
        demo = "examples/player-demo"
        out += [_step(demo + ": npm ci", [npm, "ci"], demo),
                _step(demo + ": build", [npm, "run", "build"], demo),
                _step("npm-runtime: build", [npm, "run", "build"], "packaging/npm-runtime"),
                _step("npm-runtime: test", [npm, "test"], "packaging/npm-runtime", release),
                _step("Node smoke host", [node, "packaging/smoke/run-node.mjs"], env=release),
                _step("clients/python tests", [py, "-m", "pytest", "clients/python/tests", "-q"], env=release),
                _step("Python smoke host", [py, "packaging/smoke/python_host.py"], env=host)]
        return out
    if gate == "embed":
        npm = shutil.which("npm") or "npm"
        exe = str(REPO_ROOT / "target" / "release" / "sonarad.exe")
        return [_step("cargo build runtime (release)", ["cargo", "build", "-p", "sonarad", "-p", "sonara-hook",
                                                        "-p", "sonara-cli", "--release"]),
                _step("stage runtime DLLs", [py, "packaging/runtime_dlls.py", "stage", "target/release"]),
                _step("clients/ts: npm ci", [npm, "ci"], "clients/ts"),
                _step("clients/ts: build", [npm, "run", "build"], "clients/ts"),
                _step("tests/embed", [py, "-m", "pytest", "tests/embed", "-q", "-rs"], env={"SONARAD": exe})]
    if gate == "earcons":
        return [_step("earcons", [py, "packaging/sounds/build_earcons.py", "--check"])]
    raise ValueError(gate)


def _short(arg: str) -> str:
    """``arg`` as a reader types it: the program's name, a repo path relative."""
    p = Path(arg)
    if p.is_absolute():
        try:
            return p.relative_to(REPO_ROOT).as_posix()
        except ValueError:
            return p.stem.lower() if p.suffix.lower() in (".exe", ".cmd") else arg
    return arg


def describe(step: Step) -> str:
    label, argv, cwd, env = step
    envs = "".join("{0}={1} ".format(k, _short(v)) for k, v in env.items())
    where = " (in {0})".format(cwd) if cwd else ""
    return "{0}: {1}{2}{3}".format(label, envs, " ".join(_short(a) for a in argv), where)


def _run(step: Step) -> Tuple[bool, float]:
    label, argv, cwd, env = step
    full_env = dict(os.environ)
    full_env.update(env)
    start = time.monotonic()
    try:
        code = subprocess.run(argv, cwd=REPO_ROOT / cwd if cwd else REPO_ROOT, env=full_env).returncode
    except OSError as e:
        print("gate: {0}: {1}".format(label, e), file=sys.stderr)
        code = 1
    return code == 0, time.monotonic() - start


def main(argv: Optional[List[str]] = None) -> int:
    ap = argparse.ArgumentParser(description="Run the gates a change needs (#256).")
    ap.add_argument("--all", action="store_true", help="every gate but earcons, whatever changed")
    ap.add_argument("--quick", action="store_true", help="clippy and tests only for the changed crates and their dependents")
    ap.add_argument("--base", default="origin/main", help="compare with the merge base of this ref (default origin/main)")
    ap.add_argument("--dry-run", action="store_true", help="print the plan, run nothing")
    ap.add_argument("--keep-going", action="store_true", help="run every step even after a failure")
    args = ap.parse_args(argv)

    paths = [] if args.all else changed_paths(args.base)
    gates = [g for g, _ in GATES if g != "earcons"] if args.all else select(paths)
    if not gates:
        print("gate: nothing changed against {0} (use --all to run everything)".format(args.base))
        return 0
    crates = affected_crates(paths, _cargo_metadata()) if args.quick and not args.all else None
    nextest = _has_nextest()
    print("gate: {0} changed path(s); gates: {1}{2}; tests via {3}".format(
        len(paths) if not args.all else "all", ", ".join(gates),
        "" if crates is None else " (crates: {0})".format(", ".join(crates) or "none"),
        "cargo nextest" if nextest else "cargo test"))

    plan: List[Step] = []
    for step in (s for g in gates for s in steps_for(g, crates, nextest)):
        # A step two gates share (the release build, npm ci) runs once.
        if not any(step[1:] == done[1:] for done in plan):
            plan.append(step)
    if args.dry_run:
        for step in plan:
            print("  " + describe(step))
        return 0

    results: List[Tuple[str, bool, float]] = []
    for step in plan:
        print("\n== gate: {0}".format(step[0]), flush=True)
        ok, secs = _run(step)
        results.append((step[0], ok, secs))
        if not ok and not args.keep_going:
            break

    print("\n== gate summary")
    for label, ok, secs in results:
        print("{0}: {1} ({2:.0f} s)".format(label, "ok" if ok else "FAILED", secs))
    skipped = len(plan) - len(results)
    if skipped:
        print("{0} step(s) not run after the failure (--keep-going runs them)".format(skipped))
    return 0 if all(ok for _, ok, _ in results) and not skipped else 1


if __name__ == "__main__":
    sys.exit(main())
