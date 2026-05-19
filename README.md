# claude-tts

Text-to-speech accessibility layer for [Claude Code](https://claude.ai/code) on macOS.

Streams Claude's responses aloud as they are written, announces tool calls, and reads commands before you approve them — all without changing how Claude looks or behaves.

## How it works

- A lightweight PTY wrapper (`claude-speak`) sits between your terminal and the `claude` binary. It relays all output unchanged while feeding Claude's prose to macOS `say` sentence by sentence.
- Two hooks fire on tool events: one reads commands aloud before you approve them (`PermissionRequest`), one announces what is running after approval (`PreToolUse`).
- A `/read` slash command toggles everything on or off, reads past responses on demand, and stops speech instantly.
- A single flag file (`~/.claude-tts-enabled`) is the master switch. Both the PTY wrapper and all hooks check it before speaking anything.

## Requirements

- macOS (uses the built-in `say` command)
- Python 3 (pre-installed on macOS)
- `jq` — `brew install jq`
- Claude Code CLI

## Install

```bash
git clone https://github.com/nimkimi/claude-tts ~/projects/claude-tts
cd ~/projects/claude-tts
./install.sh
source ~/.zshrc
```

## Uninstall

```bash
cd ~/projects/claude-tts
./uninstall.sh
source ~/.zshrc
```

## Usage

All control happens inside a Claude Code session via the `/read` slash command.

| Command | Effect |
|---|---|
| `/read` | Toggle speaking mode on / off |
| `/read -1` | Read the last response aloud (one-shot) |
| `/read -2` | Read the second-to-last response |
| `/read stop` | Stop speech immediately |
| `/read status` | Print whether speaking mode is on or off |

### What gets read

| Content | Behaviour |
|---|---|
| Claude's prose / reasoning | Read aloud as it streams |
| Code blocks | Announced as "code block", content skipped |
| Bash command (before approval) | "Approve? Run: git status" |
| File write (before approval) | "Approve? Write file: main.py" |
| Tool running (after approval) | "Running: …" / "Reading: …" / "Writing: …" |
| Agent / superpower | "Superpower: brainstorming" |
| JSON, spinners, box-drawing | Silently suppressed |

### Speaking mode vs one-shot reads

- **Speaking mode on** — everything is read aloud automatically as Claude works
- **Speaking mode off** — silent by default; use `/read -1` to catch up on a specific response when you want it

## Multiple machines

```bash
# On each new Mac
git clone https://github.com/nimkimi/claude-tts ~/projects/claude-tts
cd ~/projects/claude-tts && ./install.sh && source ~/.zshrc
```

Updates: `git pull && ./install.sh`

The `~/.claude-tts-enabled` flag is per-machine and is not synced — each machine starts silent.
