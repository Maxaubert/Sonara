#!/usr/bin/env bash
# uninstall.sh — removes claude-tts from this machine
set -euo pipefail

BIN="$HOME/.local/bin"
SETTINGS="$HOME/.claude/settings.json"
SHELL_RC="$HOME/.zshrc"

echo "Uninstalling claude-tts..."

# Binaries
rm -f "$BIN/claude-speak" "$BIN/claude-tts"
echo "  ✓ Binaries removed"

# Hook scripts
rm -f "$HOME/.claude/hooks/claude-tts-permission.sh"
rm -f "$HOME/.claude/hooks/claude-tts-pre-tool.sh"
rm -f "$HOME/.claude/hooks/claude-tts-stop.sh"
echo "  ✓ Hook scripts removed"

# Slash command
rm -f "$HOME/.claude/commands/read.md"
echo "  ✓ Slash command removed"

# Remove hooks from settings.json
if [ -f "$SETTINGS" ]; then
  python3 - "$SETTINGS" << 'PYEOF'
import json, sys

path = sys.argv[1]
with open(path) as f:
    settings = json.load(f)

hooks = settings.get("hooks", {})
for event in list(hooks.keys()):
    hooks[event] = [
        g for g in hooks[event]
        if not any("claude-tts" in h.get("command", "")
                   for h in g.get("hooks", []))
    ]
    if not hooks[event]:
        del hooks[event]

with open(path, "w") as f:
    json.dump(settings, f, indent=2)
    f.write("\n")
PYEOF
  echo "  ✓ Hooks removed from $SETTINGS"
fi

# Remove alias from .zshrc
sed -i '' '/# claude-tts/d; /claude-speak/d' "$SHELL_RC" 2>/dev/null || true
echo "  ✓ Alias removed from $SHELL_RC"

# Remove TTS flag
rm -f "$HOME/.claude-tts-enabled"

echo ""
echo "Done. Reload your shell to apply."
