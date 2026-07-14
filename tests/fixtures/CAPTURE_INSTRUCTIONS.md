# Golden Hook Payload Capture Instructions

This directory holds raw JSON payloads captured from real Claude Code hook invocations.
These serve as golden fixtures for parser and integration tests.

## How the capture mechanism works

`bin/sonara-hook` reads the env var `SONARA_CAPTURE`. When set to a directory path, the hook
dumps the raw stdin bytes it receives to `${SONARA_CAPTURE}/<event>-<pid>.json` BEFORE any
other processing, so even a crash in downstream code leaves the payload on disk.

The relevant code is in `bin/sonara-hook`:

```python
if os.environ.get("SONARA_CAPTURE"):
    try:
        cap_dir = os.environ["SONARA_CAPTURE"]
        os.makedirs(cap_dir, exist_ok=True)
        cap_path = os.path.join(cap_dir, f"{event}-{os.getpid()}.json")
        with open(cap_path, "wb") as fh:
            fh.write(raw)
    except Exception:
        pass
```

## Steps to capture real payloads

1. Create a capture directory and export the env var in the same shell you will launch Claude:

```bash
mkdir -p /tmp/sonara-capture
export SONARA_CAPTURE=/tmp/sonara-capture
```

2. Ensure `hooks/hooks.json` is installed/linked so `${CLAUDE_PLUGIN_ROOT}/bin/sonara-hook <Event>`
   fires for each event. Launch `claude` in the same shell so hook subprocesses inherit `SONARA_CAPTURE`.

3. Trigger each event exactly once:
   - **MessageDisplay**: let Claude stream any normal prose reply
     (e.g. "say hello in one sentence").
   - **PreToolUse · Bash**: ask Claude to run a shell command that requires approval
     (e.g. "run `git status` for me").
   - **PreToolUse · AskUserQuestion**: prompt Claude to ask you a multiple-choice question
     (e.g. "ask me which color I prefer between red and blue").
   - **PreToolUse · ExitPlanMode**: enter plan mode (Shift+Tab to planning), have Claude
     produce a plan, and approve/reject it.
   - **Notification · permission_prompt**: the permission approval prompt triggered by the
     Bash tool-use above.
   - **Notification · idle_prompt**: leave the session idle until Claude emits the idle
     notification.

4. Inspect and copy the captured payloads into stable fixture names:

```bash
ls -la /tmp/sonara-capture

mkdir -p tests/fixtures

# Copy and rename each file (inspect contents to disambiguate PreToolUse variants):
cp /tmp/sonara-capture/MessageDisplay-*.json        tests/fixtures/MessageDisplay.json

# For PreToolUse: inspect tool_name inside the JSON to pick the right file:
#   tool_name == "AskUserQuestion" -> PreToolUse-AskUserQuestion.json
#   tool_name == "ExitPlanMode"    -> PreToolUse-ExitPlanMode.json
#   tool_name == "Bash"            -> PreToolUse-Bash.json

# For Notification: inspect notification_type:
#   notification_type == "permission_prompt" -> Notification-permission_prompt.json
#   notification_type == "idle_prompt"       -> Notification-idle_prompt.json
```

5. Commit the captured fixtures:

```bash
git add tests/fixtures
git commit -m "chore: capture golden hook payloads from a real Claude session

Co-Authored-By: Claude <noreply@anthropic.com>"
```

## Expected fixture set

After capture, the directory should contain exactly these six JSON files
(plus this instruction file and `.gitkeep`):

```
tests/fixtures/MessageDisplay.json
tests/fixtures/PreToolUse-AskUserQuestion.json
tests/fixtures/PreToolUse-ExitPlanMode.json
tests/fixtures/PreToolUse-Bash.json
tests/fixtures/Notification-permission_prompt.json
tests/fixtures/Notification-idle_prompt.json
```

## Note on representative fixtures

The `seed representative golden payload fixtures` task in the plan creates best-effort
representative payloads from the live schemas. The TDD tasks use those representative
payloads. Once you replace them with real captures, re-run the parser tests - they should
still pass since the representative schemas match the real field shapes.
