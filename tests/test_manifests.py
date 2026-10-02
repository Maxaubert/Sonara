"""Validate the shipped plugin manifests as real JSON and assert every
hooks.json command points at the bin/sonara-hook-run launcher under the repo root."""
import json
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
PLUGIN_JSON = REPO_ROOT / ".claude-plugin" / "plugin.json"
HOOKS_JSON = REPO_ROOT / "hooks" / "hooks.json"
SONARA_HOOK = REPO_ROOT / "bin" / "sonara-hook"
SONARA_HOOK_RUN = REPO_ROOT / "bin" / "sonara-hook-run"


def _load(path: Path) -> dict:
    assert path.is_file(), f"missing manifest: {path}"
    return json.loads(path.read_text(encoding="utf-8"))


def test_plugin_json_is_valid_and_named():
    data = _load(PLUGIN_JSON)
    assert isinstance(data, dict)
    assert data.get("name"), "plugin.json must declare a non-empty name"


def test_sonara_hook_shim_exists():
    assert SONARA_HOOK.is_file(), f"missing hook shim: {SONARA_HOOK}"


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
        # Commands use ${CLAUDE_PLUGIN_ROOT}/bin/sonara-hook-run <Event>.
        assert "${CLAUDE_PLUGIN_ROOT}" in cmd, (
            f"command must use ${{CLAUDE_PLUGIN_ROOT}}: {cmd!r}"
        )
        # Resolve the plugin-root-relative path to this repo and assert it
        # points at the existing interpreter launcher, which runs
        # bin/sonara-hook on a real Python (E3).
        rel = cmd.split("${CLAUDE_PLUGIN_ROOT}", 1)[1].lstrip("/")
        # rel looks like 'bin/sonara-hook-run" MessageDisplay' -> the path token.
        path_token = rel.split()[0].rstrip('"')
        resolved = REPO_ROOT / path_token
        assert resolved == SONARA_HOOK_RUN, f"command path {path_token!r} != bin/sonara-hook-run"
        assert resolved.is_file(), f"hook command target does not exist: {resolved}"
        assert SONARA_HOOK.is_file()


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


def _pyproject_version() -> str:
    # Regex, not tomllib: the suite also runs on Python 3.9 (no tomllib there).
    import re
    text = (REPO_ROOT / "pyproject.toml").read_text(encoding="utf-8")
    m = re.search(r'^version = "([^"]+)"', text, re.MULTILINE)
    assert m, "pyproject.toml declares no [project] version"
    return m.group(1)


def test_manifest_versions_match_pyproject():
    # release.yml publishes v<pyproject version> and refuses a mismatch; plugin
    # updates are keyed on the manifest version, so all three must move together.
    version = _pyproject_version()
    assert _load(PLUGIN_JSON).get("version") == version
    plugins = _load(REPO_ROOT / ".claude-plugin" / "marketplace.json").get("plugins") or []
    assert plugins, "marketplace.json declares no plugins"
    assert plugins[0].get("version") == version


def test_package_version_matches_pyproject():
    # sonara.__version__ is user-visible (doctor, the settings page footer
    # renders it from /api/state, #161), so it must move with the release.
    import re
    version = _pyproject_version()
    init = (REPO_ROOT / "src" / "sonara" / "__init__.py").read_text(encoding="utf-8")
    m = re.search(r'^__version__ = "([^"]+)"', init, re.M)
    assert m and m.group(1) == version


def test_pyproject_version_is_0_10_1():
    assert _pyproject_version() == "0.10.2"


def test_sdk_package_versions_match_pyproject():
    # The SDKs and the npm runtime package ship from the same release tag
    # as the runtime they talk to and bundle (runtime plan, M9).
    import re
    version = _pyproject_version()
    for rel in ("clients/ts/package.json", "clients/player/package.json", "packaging/npm-runtime/package.json"):
        assert _load(REPO_ROOT / rel).get("version") == version, rel
    ts_version = (REPO_ROOT / "clients" / "ts" / "src" / "version.ts").read_text(encoding="utf-8")
    m = re.search(r'^export const VERSION = "([^"]+)";', ts_version, re.M)
    assert m and m.group(1) == version, "clients/ts/src/version.ts"
    py_client = REPO_ROOT / "clients" / "python"
    m = re.search(r'^version = "([^"]+)"', (py_client / "pyproject.toml").read_text(encoding="utf-8"), re.M)
    assert m and m.group(1) == version, "clients/python/pyproject.toml"
    init = (py_client / "src" / "sonara_client" / "version.py").read_text(encoding="utf-8")
    m = re.search(r'^__version__ = "([^"]+)"', init, re.M)
    assert m and m.group(1) == version, "clients/python/src/sonara_client/version.py"


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


def test_cargo_workspace_version_matches_pyproject():
    # The Rust runtime (crates/) ships from the same release tag, so the
    # [workspace.package] version moves with pyproject (runtime plan, M1).
    import re
    text = (REPO_ROOT / "Cargo.toml").read_text(encoding="utf-8")
    section = re.search(r"^\[workspace\.package\]\s*$(.*?)(?=^\[|\Z)", text, re.M | re.S)
    assert section, "Cargo.toml declares no [workspace.package]"
    m = re.search(r'^version = "([^"]+)"', section.group(1), re.M)
    assert m and m.group(1) == _pyproject_version()


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
