#!/usr/bin/env bash
# Fired after the user approves a tool call, just before it executes.
# Announces what is about to run.

[ -f "$HOME/.claude-tts-enabled" ] || exit 0
command -v jq >/dev/null 2>&1 || exit 0

INPUT="$(cat)"
TOOL="$(echo "$INPUT" | jq -r '.tool_name // empty' 2>/dev/null)"
[ -z "$TOOL" ] && exit 0

# Kill previous announcement so they don't pile up
pkill -x say 2>/dev/null || true

case "$TOOL" in
  Bash)
    CMD="$(echo "$INPUT" | jq -r '.tool_input.command // empty' 2>/dev/null | head -c 80)"
    [ -n "$CMD" ] && say "Running: $CMD" &
    ;;
  Read)
    FILE="$(echo "$INPUT" | jq -r '.tool_input.file_path // empty' 2>/dev/null | xargs basename 2>/dev/null)"
    [ -n "$FILE" ] && say "Reading: $FILE" &
    ;;
  Write)
    FILE="$(echo "$INPUT" | jq -r '.tool_input.file_path // empty' 2>/dev/null | xargs basename 2>/dev/null)"
    [ -n "$FILE" ] && say "Writing: $FILE" &
    ;;
  Edit|MultiEdit)
    FILE="$(echo "$INPUT" | jq -r '.tool_input.file_path // empty' 2>/dev/null | xargs basename 2>/dev/null)"
    [ -n "$FILE" ] && say "Editing: $FILE" &
    ;;
  Glob)
    say "Searching files" &
    ;;
  Grep)
    PATTERN="$(echo "$INPUT" | jq -r '.tool_input.pattern // empty' 2>/dev/null | head -c 50)"
    [ -n "$PATTERN" ] && say "Searching for: $PATTERN" || say "Searching code" &
    ;;
  Agent)
    DESC="$(echo "$INPUT" | jq -r '.tool_input.description // .tool_input.prompt // empty' 2>/dev/null | head -c 60)"
    [ -n "$DESC" ] && say "Superpower: $DESC" || say "Launching agent" &
    ;;
  WebSearch)
    Q="$(echo "$INPUT" | jq -r '.tool_input.query // empty' 2>/dev/null | head -c 60)"
    [ -n "$Q" ] && say "Searching: $Q" &
    ;;
  WebFetch)
    say "Fetching webpage" &
    ;;
  NotebookEdit)
    say "Editing notebook" &
    ;;
  TodoWrite|TodoRead)
    say "Updating tasks" &
    ;;
  *)
    say "Using: $TOOL" &
    ;;
esac

exit 0
