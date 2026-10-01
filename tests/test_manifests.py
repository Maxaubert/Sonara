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


def test_package_and_settings_page_versions_match_pyproject():
    # sonara.__version__ and the settings page footer are user-visible (the
    # footer is read by screen readers), so they must move with the release.
    import re
    version = _pyproject_version()
    init = (REPO_ROOT / "src" / "sonara" / "__init__.py").read_text(encoding="utf-8")
    m = re.search(r'^__version__ = "([^"]+)"', init, re.M)
    assert m and m.group(1) == version
    html = (REPO_ROOT / "src" / "sonara" / "settings.html").read_text(encoding="utf-8")
    assert f"<span>Version {version}</span>" in html


def test_pyproject_version_is_0_6_12():
    assert _pyproject_version() == "0.6.12"


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
