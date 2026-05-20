#!/usr/bin/env bash
# Fired before each tool runs.
# 1) Speaks any new Claude text written since the last read (synchronous — in order)
# 2) Announces the tool name in the background while it executes

[ -f "$HOME/.claude-tts-enabled" ] || exit 0
command -v jq >/dev/null 2>&1 || exit 0

INPUT="$(cat)"
TOOL="$(echo "$INPUT" | jq -r '.tool_name // empty' 2>/dev/null)"
[ -z "$TOOL" ] && exit 0

# Read and speak any new Claude text before the tool announcement
python3 - << 'PYEOF'
import sys, json, re, subprocess, os, glob

POS_FILE = os.path.expanduser("~/.claude-tts-pos")

def read_pos():
    try:
        with open(POS_FILE) as f:
            lines = f.read().strip().split('\n')
            return lines[0], int(lines[1]) if len(lines) > 1 else 0
    except Exception:
        return "", 0

def write_pos(transcript, count):
    with open(POS_FILE, 'w') as f:
        f.write(f"{transcript}\n{count}\n")

def read_messages(path):
    messages = []
    with open(path) as f:
        for line in f:
            line = line.strip()
            if not line: continue
            try:
                obj = json.loads(line)
                if obj.get('isSidechain', False): continue
                msg = obj.get('message', {})
                if not isinstance(msg, dict): continue
                if msg.get('role') != 'assistant': continue
                content = msg.get('content', '')
                if isinstance(content, str) and content.strip():
                    messages.append(content)
                elif isinstance(content, list):
                    parts = [c.get('text', '') for c in content
                             if isinstance(c, dict) and c.get('type') == 'text']
                    text = ' '.join(parts).strip()
                    if text:
                        messages.append(text)
            except Exception:
                pass
    return messages

def clean(text):
    text = re.sub(r'```[\s\S]*?```', ' code block. ', text)
    text = re.sub(r'`[^`\n]{1,120}`', '', text)
    text = re.sub(r'^#{1,6}\s+', '', text, flags=re.MULTILINE)
    text = re.sub(r'\*{1,3}([^*\n]+)\*{1,3}', r'\1', text)
    text = re.sub(r'https?://\S+', 'link', text)
    text = re.sub(r'\|[^\n]+\|', '', text)
    text = re.sub(r'[ \t]{3,}', ' ', text)
    text = re.sub(r'\n{3,}', '\n\n', text)
    return text.strip()

files = glob.glob(os.path.expanduser('~/.claude/projects/**/*.jsonl'), recursive=True)
if not files:
    sys.exit(0)
transcript = max(files, key=os.path.getmtime)

prev_transcript, prev_count = read_pos()
messages = read_messages(transcript)

if prev_transcript != transcript:
    write_pos(transcript, len(messages))
    sys.exit(0)

new_messages = messages[prev_count:]
if not new_messages:
    sys.exit(0)

write_pos(transcript, len(messages))

text = clean('\n\n'.join(new_messages))
if text:
    subprocess.run(['pkill', '-x', 'say'], capture_output=True)
    subprocess.run(['say', text])  # synchronous — tool waits until text is spoken
PYEOF

# Announce the tool in the background while it runs
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
