#!/usr/bin/env bash
# install.sh — sets up claude-tts on this machine
set -euo pipefail

REPO="$(cd "$(dirname "$0")" && pwd)"
BIN="$HOME/.local/bin"
COMMANDS_DIR="$HOME/.claude/commands"
HOOKS_DIR="$HOME/.claude/hooks"
SETTINGS="$HOME/.claude/settings.json"
SHELL_RC="$HOME/.zshrc"

echo "Installing claude-tts..."

# 1 — Directories
mkdir -p "$BIN" "$COMMANDS_DIR" "$HOOKS_DIR"

# 2 — Binaries
cp "$REPO/bin/claude-speak" "$BIN/claude-speak"
cp "$REPO/bin/claude-tts"   "$BIN/claude-tts"
chmod +x "$BIN/claude-speak" "$BIN/claude-tts"
echo "  ✓ Binaries → $BIN"

# 3 — Hook scripts
cp "$REPO/hooks/permission-request.sh" "$HOOKS_DIR/claude-tts-permission.sh"
cp "$REPO/hooks/pre-tool-use.sh"       "$HOOKS_DIR/claude-tts-pre-tool.sh"
chmod +x "$HOOKS_DIR/claude-tts-permission.sh" "$HOOKS_DIR/claude-tts-pre-tool.sh"
echo "  ✓ Hooks → $HOOKS_DIR"

# 4 — Slash command
cp "$REPO/commands/read.md" "$COMMANDS_DIR/read.md"
echo "  ✓ Slash command /read → $COMMANDS_DIR"

# 5 — Merge hooks into settings.json
python3 - "$SETTINGS" "$HOOKS_DIR" << 'PYEOF'
import json, os, sys

settings_path = sys.argv[1]
hooks_dir     = sys.argv[2]

settings = {}
if os.path.exists(settings_path):
    with open(settings_path) as f:
        try:
            settings = json.load(f)
        except json.JSONDecodeError:
            pass

hooks = settings.setdefault("hooks", {})

def add_hook(event, command):
    entry = {"type": "command", "command": command}
    if event not in hooks:
        hooks[event] = [{"hooks": [entry]}]
        return
    for group in hooks[event]:
        for h in group.get("hooks", []):
            if h.get("command") == command:
                return  # already present
    hooks[event].append({"hooks": [entry]})

add_hook("PermissionRequest", f"{hooks_dir}/claude-tts-permission.sh")
add_hook("PreToolUse",        f"{hooks_dir}/claude-tts-pre-tool.sh")

os.makedirs(os.path.dirname(settings_path), exist_ok=True)
with open(settings_path, "w") as f:
    json.dump(settings, f, indent=2)
    f.write("\n")
PYEOF
echo "  ✓ Hooks registered in $SETTINGS"

# 6 — Shell alias (idempotent)
if ! grep -q "# claude-tts" "$SHELL_RC" 2>/dev/null; then
  printf '\n# claude-tts\nalias claude='"'"'claude-speak'"'"'\n' >> "$SHELL_RC"
  echo "  ✓ Alias added to $SHELL_RC"
else
  echo "  ✓ Alias already present in $SHELL_RC"
fi

# 7 — Ensure ~/.local/bin is in PATH
if ! grep -q 'local/bin' "$SHELL_RC" 2>/dev/null; then
  printf '\nexport PATH="$HOME/.local/bin:$PATH"  # claude-tts\n' >> "$SHELL_RC"
  echo "  ✓ ~/.local/bin added to PATH in $SHELL_RC"
fi

echo ""
echo "Done! Reload your shell, then:"
echo "  /read          — toggle speaking mode on/off"
echo "  /read -1       — read last response"
echo "  /read stop     — stop speech immediately"
