#!/usr/bin/env bash
# Fired when Claude needs user approval for a tool call.
# Reads the command/action aloud so the user knows what they're approving.

[ -f "$HOME/.claude-tts-enabled" ] || exit 0
command -v jq >/dev/null 2>&1 || exit 0

INPUT="$(cat)"
TOOL="$(echo "$INPUT" | jq -r '.tool_name // empty' 2>/dev/null)"
[ -z "$TOOL" ] && exit 0

# Stop any speech in progress so the approval prompt is heard clearly
pkill -x say 2>/dev/null || true

case "$TOOL" in
  Bash)
    CMD="$(echo "$INPUT" | jq -r '.tool_input.command // empty' 2>/dev/null | head -c 120)"
    [ -n "$CMD" ] && say "Approve? Run: $CMD" &
    ;;
  Write)
    FILE="$(echo "$INPUT" | jq -r '.tool_input.file_path // empty' 2>/dev/null | xargs basename 2>/dev/null)"
    say "Approve? Write file: $FILE" &
    ;;
  Edit)
    FILE="$(echo "$INPUT" | jq -r '.tool_input.file_path // empty' 2>/dev/null | xargs basename 2>/dev/null)"
    say "Approve? Edit file: $FILE" &
    ;;
  MultiEdit)
    FILE="$(echo "$INPUT" | jq -r '.tool_input.file_path // empty' 2>/dev/null | xargs basename 2>/dev/null)"
    say "Approve? Multi-edit: $FILE" &
    ;;
  *)
    say "Approve? $TOOL" &
    ;;
esac

exit 0
