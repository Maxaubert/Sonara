#!/usr/bin/env bash
# Fired when Claude finishes a turn. Auto-reads the new response if TTS is enabled.

[ -f "$HOME/.claude-tts-enabled" ] || exit 0

TRANSCRIPT=$(python3 -c "
import glob, os
files = glob.glob(os.path.expanduser('~/.claude/projects/**/*.jsonl'), recursive=True)
if files:
    print(max(files, key=os.path.getmtime))
")

[ -z "$TRANSCRIPT" ] || [ ! -f "$TRANSCRIPT" ] && exit 0

python3 - "$TRANSCRIPT" << 'PYEOF'
import sys, json, re, subprocess, time

transcript = sys.argv[1]

def is_real_user_turn(msg):
    content = msg.get('content', '')
    if isinstance(content, str):
        return bool(content.strip())
    if isinstance(content, list):
        return any(isinstance(c, dict) and c.get('type') == 'text' for c in content)
    return False

def extract_assistant_text(msg):
    content = msg.get('content', '')
    if isinstance(content, str):
        return content.strip()
    if isinstance(content, list):
        parts = [c.get('text', '') for c in content
                 if isinstance(c, dict) and c.get('type') == 'text']
        return ' '.join(parts).strip()
    return ''

def read_turns(path):
    turns = []
    current_blocks = []
    with open(path) as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            try:
                obj = json.loads(line)
                if obj.get('isSidechain', False):
                    continue
                msg = obj.get('message', {})
                if not isinstance(msg, dict):
                    continue
                role = msg.get('role', '')
                if role == 'user' and is_real_user_turn(msg):
                    if current_blocks:
                        turns.append('\n\n'.join(current_blocks))
                    current_blocks = []
                elif role == 'assistant':
                    text = extract_assistant_text(msg)
                    if text:
                        current_blocks.append(text)
            except Exception:
                pass
    if current_blocks:
        turns.append('\n\n'.join(current_blocks))
    return turns

# Poll up to 3s for the new response to land in the transcript
initial_count = len(read_turns(transcript))
turns = read_turns(transcript)
for _ in range(6):
    time.sleep(0.5)
    turns = read_turns(transcript)
    if len(turns) > initial_count:
        break

# Only speak when a new turn appeared — skip silent turns (/read, tool-only)
if len(turns) <= initial_count or not turns:
    sys.exit(0)

text = turns[-1]
text = re.sub(r'```[\s\S]*?```', ' code block. ', text)
text = re.sub(r'`[^`\n]{1,120}`', '', text)
text = re.sub(r'^#{1,6}\s+', '', text, flags=re.MULTILINE)
text = re.sub(r'\*{1,3}([^*\n]+)\*{1,3}', r'\1', text)
text = re.sub(r'https?://\S+', 'link', text)
text = re.sub(r'\|[^\n]+\|', '', text)
text = re.sub(r'[ \t]{3,}', ' ', text)
text = re.sub(r'\n{3,}', '\n\n', text)
text = text.strip()

if text:
    subprocess.Popen(['say', text])
PYEOF

exit 0
