"""Validate the shipped plugin manifests as real JSON and assert every
hooks.json command points at the bin/sonara-hook-launch launcher under the
repo root (#202: the Rust runtime, no Python)."""
import importlib.util
import json
import re
import shutil
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parent.parent
PLUGIN_JSON = REPO_ROOT / ".claude-plugin" / "plugin.json"
MARKETPLACE_JSON = REPO_ROOT / ".claude-plugin" / "marketplace.json"
HOOKS_JSON = REPO_ROOT / "hooks" / "hooks.json"
LAUNCHER = REPO_ROOT / "bin" / "sonara-hook-launch"
RUNTIME_VERSION = REPO_ROOT / "bin" / "runtime-version"
BUMP_SCRIPT = REPO_ROOT / "packaging" / "bump_version.py"


def _load(path: Path) -> dict:
    assert path.is_file(), f"missing manifest: {path}"
    return json.loads(path.read_text(encoding="utf-8"))


def test_plugin_json_is_valid_and_named():
    data = _load(PLUGIN_JSON)
    assert isinstance(data, dict)
    assert data.get("name"), "plugin.json must declare a non-empty name"


def test_the_hook_launcher_exists():
    assert LAUNCHER.is_file(), f"missing hook launcher: {LAUNCHER}"


def _iter_hook_commands(data: dict):
    """Yield every 'command' string found anywhere in the hooks.json tree.

    hooks.json shape (Claude Code): {"hooks": {<EventName>: [ {"hooks":
    [ {"type":"command","command":"..."} ] } ] }}. We walk it generically so
    the test does not over-constrain the exact nesting.
    """
    def walk(node):
        if isinstance(node, dict):
            cmd = node.get("command")
            if isinstance(cmd, str):
                yield cmd
            for v in node.values():
                yield from walk(v)
        elif isinstance(node, list):
            for v in node:
                yield from walk(v)

    yield from walk(data)


def test_hooks_json_commands_point_at_the_hook_launcher():
    data = _load(HOOKS_JSON)
    commands = list(_iter_hook_commands(data))
    assert commands, "hooks.json declares no commands"

    for cmd in commands:
        # Commands use ${CLAUDE_PLUGIN_ROOT}/bin/sonara-hook-launch <Event>.
        assert "${CLAUDE_PLUGIN_ROOT}" in cmd, (
            f"command must use ${{CLAUDE_PLUGIN_ROOT}}: {cmd!r}"
        )
        # Resolve the plugin-root-relative path to this repo and assert it
        # points at the bash launcher, which execs the runtime's
        # sonara-hook.exe (or installs the runtime first).
        rel = cmd.split("${CLAUDE_PLUGIN_ROOT}", 1)[1].lstrip("/")
        # rel looks like 'bin/sonara-hook-launch" MessageDisplay' -> the path token.
        path_token = rel.split()[0].rstrip('"')
        resolved = REPO_ROOT / path_token
        assert resolved == LAUNCHER, f"command path {path_token!r} != bin/sonara-hook-launch"
        assert resolved.is_file(), f"hook command target does not exist: {resolved}"
        event = rel.split()[1]
        assert cmd == '"${CLAUDE_PLUGIN_ROOT}/bin/sonara-hook-launch" ' + event


def test_no_plugin_file_runs_python():
    # #202: hooks.json, bin/* and commands/*.md must not need Python.
    files = [HOOKS_JSON, *sorted((REPO_ROOT / "bin").iterdir()),
             *sorted((REPO_ROOT / "commands").glob("*.md"))]
    for f in files:
        text = f.read_text(encoding="utf-8").lower()
        assert not re.search(r"\bpython[w3]?(\.exe)?\b|\bpy -3\b|#!.*python", text), f.name


def test_bash_scripts_keep_lf_line_endings():
    # Git Bash runs these: a CR would end up in every command.
    for name in ("sonara-hook-launch", "sonara", "sonara-runtime.sh", "runtime-version"):
        assert b"\r" not in (REPO_ROOT / "bin" / name).read_bytes(), name


def test_the_runtime_version_is_the_release_version():
    # The launcher downloads the release named here: it must be the one
    # release.yml publishes for this commit.
    assert RUNTIME_VERSION.read_bytes() == (_pyproject_version() + "\n").encode()


def test_the_marketplace_serves_the_plugin_from_the_repo():
    # `/plugin marketplace add Maxaubert/Sonara`, then `/plugin install sonara@sonara`.
    data = _load(MARKETPLACE_JSON)
    assert data["name"] == "sonara"
    assert data["owner"]["name"]
    (plugin,) = data["plugins"]
    assert plugin["name"] == "sonara" == _load(PLUGIN_JSON)["name"]
    assert plugin["source"] == "./"
    assert plugin["repository"] == "https://github.com/Maxaubert/Sonara"


def test_every_phase1_event_is_hooked():
    """Phase 1 wires exactly these output events; assert each appears as a
    hooks.json key so none is silently unregistered."""
    data = _load(HOOKS_JSON)
    hooks = data.get("hooks", data)
    keys = set(hooks.keys()) if isinstance(hooks, dict) else set()
    required = {
        "MessageDisplay",
        "PreToolUse",
        "Notification",
        "Stop",
        "UserPromptSubmit",
        "SessionStart",
        "SessionEnd",
    }
    missing = required - keys
    assert not missing, f"hooks.json is missing event hooks: {sorted(missing)}"


def _bump_module():
    spec = importlib.util.spec_from_file_location("bump_version", BUMP_SCRIPT)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def _pyproject_version() -> str:
    # The release version (release.yml reads pyproject.toml); test_release_zip
    # imports this helper.
    return _bump_module().read_versions(REPO_ROOT)["pyproject.toml"]


def test_every_version_file_carries_the_release_version():
    # release.yml publishes v<version> and refuses a mismatch; plugin updates
    # are keyed on the manifest version and the SDKs, the npm runtime package
    # and the Rust workspace ship from the same tag. bump_version.py owns the
    # one list of version files, so a new one is added there and checked here.
    versions = _bump_module().read_versions(REPO_ROOT)
    assert len(versions) >= 12, versions
    assert len(set(versions.values())) == 1, versions


def test_marketplace_declares_the_plugin_version_in_its_first_entry():
    plugins = _load(MARKETPLACE_JSON).get("plugins") or []
    assert plugins, "marketplace.json declares no plugins"
    assert plugins[0].get("version") == _pyproject_version()


def _copy_version_files(mod, dest: Path) -> None:
    for rel in mod.all_paths():
        target = dest / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(REPO_ROOT / rel, target)


def test_bump_version_moves_every_version_file_and_lockfile(tmp_path):
    mod = _bump_module()
    _copy_version_files(mod, tmp_path)
    old = _pyproject_version()
    before = {rel: (tmp_path / rel).read_bytes() for rel in mod.all_paths()}

    mod.bump(tmp_path, "9.8.7")

    assert set(mod.read_versions(tmp_path).values()) == {"9.8.7"}
    for rel in mod.all_paths():
        after = (tmp_path / rel).read_bytes()
        assert after != before[rel], f"{rel} was not bumped"
        # Only version strings change (line endings too stay as they were):
        # put the old one back and the file is byte for byte what it was.
        assert after.replace(b"9.8.7", old.encode()) == before[rel], rel
    # The workspace crates in Cargo.lock move, registry crates do not.
    lock = (tmp_path / "Cargo.lock").read_text(encoding="utf-8")
    assert 'name = "sonarad"\nversion = "9.8.7"' in lock
    assert (tmp_path / "bin" / "runtime-version").read_bytes() == b"9.8.7\n"


def test_bump_version_refuses_a_malformed_version(tmp_path):
    mod = _bump_module()
    _copy_version_files(mod, tmp_path)
    old = _pyproject_version()
    for bad in ("1.2", "v1.2.3", "1.2.3.4", "01.2.3", "1.2.x"):
        with pytest.raises(ValueError):
            mod.bump(tmp_path, bad)
    assert set(mod.read_versions(tmp_path).values()) == {old}


def test_sdk_packages_ship_the_mit_licence():
    # The published clients carry Sonara's MIT text: npm takes LICENSE from
    # `files`, setuptools picks up a LICENSE next to pyproject.toml.
    root = (REPO_ROOT / "LICENSE").read_text(encoding="utf-8").splitlines()
    for rel in ("clients/ts", "clients/player", "clients/python"):
        copy = REPO_ROOT / rel / "LICENSE"
        assert copy.is_file(), f"{rel}/LICENSE missing"
        assert copy.read_text(encoding="utf-8").splitlines() == root, f"{rel}/LICENSE differs from LICENSE"
    for rel in ("clients/ts", "clients/player"):
        files = _load(REPO_ROOT / rel / "package.json")["files"]
        assert "LICENSE" in files, rel


def test_player_has_no_runtime_dependencies():
    # Spec R6: the JS packages ship without runtime dependencies; the player
    # only asks for React as a peer (the headless controller needs nothing).
    pkg = _load(REPO_ROOT / "clients" / "player" / "package.json")
    assert not pkg.get("dependencies")
    assert set(pkg.get("peerDependencies", {})) == {"react"}
    assert pkg["peerDependenciesMeta"]["react"]["optional"] is True


def test_manifests_have_no_em_dash():
    # User-facing text never uses em-dashes (#45); the manifests are shown in
    # the plugin marketplace UI.
    for name in ("plugin.json", "marketplace.json"):
        raw = (REPO_ROOT / ".claude-plugin" / name).read_text(encoding="utf-8")
        text = json.dumps(json.loads(raw), ensure_ascii=False)
        assert "\u2014" not in text, name


def test_hooks_json_has_no_redundant_registrations():
    # DC9: the '' PreToolUse matcher already routes AskUserQuestion and
    # ExitPlanMode to the same command, and idle_prompt produced no message, so
    # neither is registered (each extra entry only spawned a hook process).
    hooks = _load(HOOKS_JSON)["hooks"]
    assert [e["matcher"] for e in hooks["PreToolUse"]] == [""]
    assert [e["matcher"] for e in hooks["Notification"]] == ["permission_prompt"]
