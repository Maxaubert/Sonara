# Sonara summary mode (out-of-band spoken recap)

Date: 2026-07-11

## Summary

Add an opt-in **summary mode**: when a Claude assistant turn finishes, Sonara reads
aloud a short AI-generated summary of that turn instead of the full narration. The
summary is produced by a **separate, throwaway `claude -p` call** so the user's main
Claude session is completely untouched (no hooks inject anything into its context).
Off by default.

## Motivation

Full narration of a long response is slow to listen to. The user wants the gist,
eyes-free, once a message finishes. Steering the main Claude to self-summarize was
rejected: Claude Code hooks can only inject a **per-turn** `additionalContext`
system-reminder (no standing instruction), it must be re-injected every turn, and
Claude may ignore it, so it would clutter context and risk degrading the real work
for a feature that adds nothing to the work itself. Producing the summary
out-of-band keeps the main session pristine.

## Non-goals (YAGNI / scope boundaries)

- No change to the main Claude session: no hook injection, no context edits, no
  prompt steering.
- v1 summarizes only the **foreground** session's turns (the session you hear).
  Background-session summaries are out of scope.
- No local/offline summarization; the summary requires the `claude -p` call.
- No configurable summarizer model command surface beyond a config key default
  (Haiku); no per-length tuning UI.

## Behavior

When `summary_mode` is ON:

1. **Streaming prose is not spoken.** It is still recorded to history (exactly the
   existing `quiet` behavior: `daemon.py` PROSE handler records the chunk and skips
   the speech enqueue). So catch-up / re-read still work.
2. **Decisions are unchanged.** Questions, plans, and permission prompts are still
   spoken in full and all earcons still fire. These are time-critical and cannot be
   summarized (you must act on the actual options).
3. **At turn end** (the `Stop` hook -> `EARCON turn_done` the daemon already
   receives), for the foreground session, Sonara gathers that turn's prose text from
   `history` and spawns a background summarizer:
   - `claude -p --model <haiku>` with a fixed instruction to return a 1-2 sentence
     plain-text spoken summary; the turn's prose is passed on stdin.
   - Runs in a worker thread with a timeout so it never blocks the speak loop or the
     daemon lock.
4. **On success**, the returned summary is enqueued as a speech item for that
   session and spoken through the normal loop.
5. **On failure or timeout** (offline, non-zero exit, empty output, or exceeded
   `summary_timeout`), Sonara fires a brief `summary_failed` earcon and speaks
   nothing. The full prose remains in history, so `sonara catch_up` / re-read can
   still retrieve it.

When `summary_mode` is OFF, nothing changes anywhere (default).

## The main session is untouched

There is no `UserPromptSubmit` / `SessionStart` context injection in this feature.
The summarizer is a separate OS process reading only the assistant text that already
exists. The main Claude's context window, system prompt, and behavior are identical
to summary mode being off.

## Components / files

1. **`src/sonara/summarizer.py`** (new): pure-ish module.
   - `build_prompt() -> str`: the fixed summarizer instruction.
   - `summarize(text, *, model, command, timeout, runner=None) -> str | None`:
     builds argv (`[command, "-p", "--model", model]`), feeds `text` on stdin via
     `runner` (an injectable callable defaulting to a `subprocess.run` wrapper with
     `CREATE_NO_WINDOW` on Windows and the timeout), returns the trimmed stdout, or
     `None` on any failure (non-zero exit, timeout, empty, exception). The injectable
     `runner` keeps tests from spawning `claude`.

2. **`src/sonara/daemon.py`**:
   - Turn-end trigger: in the `EARCON` handler for `turn_done` (or the turn-end
     path), when `config["summary_mode"]` is truthy and the session is the
     foreground, collect the turn's prose from `history` and dispatch a worker
     thread that calls `summarizer.summarize(...)`; on success `_enqueue` a summary
     speech item for the session and `_wake`; on failure `_earcon("summary_failed")`.
     The worker acquires `self._lock` only to enqueue the result (mirrors the hotkey
     worker pattern), never during the subprocess call.
   - PROSE gate: extend the existing `verbosity != "quiet"` speech gate so prose is
     also suppressed when `summary_mode` is on (record-to-history only).
   - `SET_SUMMARY_MODE` message handler to toggle `config["summary_mode"]` and
     `save_config`.

3. **`src/sonara/protocol.py`**: add `MsgType.SET_SUMMARY_MODE = "set_summary_mode"`.

4. **`src/sonara/config.py`**: defaults `summary_mode=False`,
   `summary_model="claude-haiku-4-5-20251001"`, `summary_command="claude"`,
   `summary_timeout=20`.

5. **`src/sonara/cli.py`** + **`commands/summary.md`**: `sonara summary on|off`
   (and bare `sonara summary` prints state) sending `SET_SUMMARY_MODE`; a
   `/sonara:summary` slash command.

6. **Earcon**: a new optional earcon kind `summary_failed`, silent no-op if the user
   has supplied no wav (matches Sonara's user-supplied-earcon convention; no shipped
   asset required).

7. **Doctor**: a check that `summary_command` resolves on PATH when `summary_mode` is
   on (so an unreachable `claude` is reported, not silently failing every turn).

8. **Docs**: `README.md` (controls + a summary-mode section) and **`PRIVACY.md`**
   (summary mode sends the assistant message text to a separate `claude -p` call;
   opt-in, off by default).

## Data flow

```
assistant streams prose -> PROSE msgs -> history.record (NOT spoken, summary_mode)
Stop hook -> EARCON turn_done -> daemon:
    if summary_mode and foreground:
        text = history prose for this turn
        thread: summary = summarizer.summarize(text, model, command, timeout)
            success -> lock -> _enqueue(session, summary) -> _wake
            failure -> _earcon("summary_failed")
```

## Latency and cost

A Haiku call adds roughly 1-5s after a message before the summary is spoken (or the
failure cue fires). One Haiku call per foreground turn. Both are accepted tradeoffs
for a real summary; documented in the summary-mode README section.

## Error handling

- Subprocess: non-zero exit, timeout, empty stdout, or spawn failure all map to
  `None` -> `summary_failed` earcon, no crash. Every failure is swallowed (the
  daemon must keep running).
- `claude` not on PATH: `summarize` returns `None` (failure cue each turn); the
  doctor check surfaces the root cause.
- Concurrency: at most one in-flight summarizer per session; a new turn-end while one
  is running supersedes/deduplicates (drop the older, keep the latest turn) to avoid
  piling up processes.

## Testing

- **`summarizer`**: with an injected fake `runner`:
  - success returns trimmed stdout;
  - non-zero exit / timeout / empty output / raised exception all return `None`;
  - argv contains `-p` and the configured model; text is passed on stdin.
- **daemon**:
  - summary_mode ON suppresses prose speech but still records history;
  - turn_done with summary_mode + foreground dispatches the summarizer (injected
    fake) and enqueues the returned summary as a spoken item;
  - summarizer failure fires the `summary_failed` earcon and enqueues nothing;
  - decisions (choice/plan/permission) are still spoken with summary_mode ON;
  - summary_mode OFF changes nothing (prose spoken as today);
  - a second turn-end while one summarizer is in flight does not double-enqueue.
- **protocol**: `SET_SUMMARY_MODE` snapshot.
- **config**: new defaults present.
- **cli**: `sonara summary on|off` sends `SET_SUMMARY_MODE`; bare prints state.

## Global constraints

- Python 3.9+, stdlib only (subprocess is stdlib); no new dependencies.
- No em-dashes in code/docs.
- Off by default; the main Claude session is never modified.
- Persistence/config via existing `save_config` / `load_config`.
- The summarizer subprocess must never block the speak loop or be held under
  `self._lock`; only the result enqueue takes the lock.
