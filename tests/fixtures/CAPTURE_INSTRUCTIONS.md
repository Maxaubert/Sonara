# Golden Hook Payload Capture Instructions

This directory holds raw JSON payloads captured from real Claude Code hook invocations.
These serve as golden fixtures for parser and integration tests.

## How the capture mechanism works

`sonara-hook.exe` (`crates/sonara-hook/src/main.rs`) reads the env var `SONARA_CAPTURE`. When
set to a directory path, the hook dumps the raw stdin bytes it receives to
`${SONARA_CAPTURE}/<event>-<pid>.json` before any other work (hooks of the summarizer's own session,
`SONARA_SUMMARIZER`, are skipped), so even a crash in
downstream code leaves the payload on disk.

Each payload here is used by a golden case in `crates/sonara-hook/tests/golden/` (its
`fixture` field); `crates/sonara-hook/tests/golden.rs` checks that none is left without one.

## Steps to capture real payloads

1. Create a capture directory and export the env var in the same shell you will launch Claude:

```bash
mkdir -p /tmp/sonara-capture
export SONARA_CAPTURE=/tmp/sonara-capture
```

2. Ensure `hooks/hooks.json` is installed/linked so `${CLAUDE_PLUGIN_ROOT}/bin/sonara-hook-launch <Event>`
   (which runs the runtime's `sonara-hook.exe`) fires for each event. Launch `claude` in the same shell so hook subprocesses inherit `SONARA_CAPTURE`.

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
(plus this instruction file):

```
tests/fixtures/MessageDisplay.json
tests/fixtures/PreToolUse-AskUserQuestion.json
tests/fixtures/PreToolUse-ExitPlanMode.json
tests/fixtures/PreToolUse-Bash.json
tests/fixtures/Notification-permission_prompt.json
tests/fixtures/Notification-idle_prompt.json
```

## After a new capture

Run `cargo test -p sonara-hook --test golden`: a new payload needs a golden case, and a
replaced one must still map to the messages its case lists.
