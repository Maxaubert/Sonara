"""Validate the shipped plugin manifests as real JSON and assert every
hooks.json command points at an existing bin/sonari-hook under the repo root."""
import json
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
PLUGIN_JSON = REPO_ROOT / ".claude-plugin" / "plugin.json"
HOOKS_JSON = REPO_ROOT / "hooks" / "hooks.json"
SONARI_HOOK = REPO_ROOT / "bin" / "sonari-hook"


def _load(path: Path) -> dict:
    assert path.is_file(), f"missing manifest: {path}"
    return json.loads(path.read_text(encoding="utf-8"))


def test_plugin_json_is_valid_and_named():
    data = _load(PLUGIN_JSON)
    assert isinstance(data, dict)
    assert data.get("name"), "plugin.json must declare a non-empty name"


def test_sonari_hook_shim_exists():
    assert SONARI_HOOK.is_file(), f"missing hook shim: {SONARI_HOOK}"


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


def test_hooks_json_commands_point_at_existing_sonari_hook():
    data = _load(HOOKS_JSON)
    commands = list(_iter_hook_commands(data))
    assert commands, "hooks.json declares no commands"

    for cmd in commands:
        # Commands use ${CLAUDE_PLUGIN_ROOT}/bin/sonari-hook <Event>.
        assert "${CLAUDE_PLUGIN_ROOT}" in cmd, (
            f"command must use ${{CLAUDE_PLUGIN_ROOT}}: {cmd!r}"
        )
        # Resolve the plugin-root-relative path to this repo and assert it
        # points at the existing bin/sonari-hook shim.
        rel = cmd.split("${CLAUDE_PLUGIN_ROOT}", 1)[1].lstrip("/")
        # rel looks like 'bin/sonari-hook MessageDisplay' -> take the path token.
        path_token = rel.split()[0]
        resolved = REPO_ROOT / path_token
        assert resolved == SONARI_HOOK, f"command path {path_token!r} != bin/sonari-hook"
        assert resolved.is_file(), f"hook command target does not exist: {resolved}"


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
