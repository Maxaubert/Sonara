#!/usr/bin/env bash
# Fired after each tool completes. Reads any new Claude text since the last read.

[ -f "$HOME/.claude-tts-enabled" ] || exit 0

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

# On first run or new session, initialize to current count — skip history
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
    subprocess.Popen(['say', text])
PYEOF

exit 0
