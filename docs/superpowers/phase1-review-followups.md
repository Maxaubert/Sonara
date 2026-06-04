# Echo Phase 1 — Review Follow-ups

Durable backlog from subagent-driven execution + the final whole-implementation review
(run `wf_dc5c640f-7c3`). The **fix pass** (run after the review) resolves the "Open — fixing
now" items below. Items under "Deferred" are real-but-lower-priority and are intentionally
left for a follow-up pass (logged so nothing is lost).

---

## Open — fixing now (the final-review fix pass)

- [ ] **CRITICAL: `/echo repeat` is a no-op.** `MsgType.REPEAT` is sent by the CLI but the daemon
  has no handler and nothing tracks the last-spoken text. A key eyes-free control silently does
  nothing. **Fix:** track last-spoken text in the daemon/speaker; add a REPEAT handler that
  re-speaks it; test it. (`daemon.py`, `speaker.py`)
- [ ] **HIGH: ProseAssembler not reset on FLUSH** → stale/garbled prose leaks into the next turn.
  **Fix:** drop/reset the session's assembler in the FLUSH handler; test cross-turn. (`daemon.py`)
- [ ] **HIGH: `afplay` earcon processes never reaped** (zombie leak). **Fix:** reap earcon
  subprocesses; test no accumulation. (`speaker.py`)
- [ ] **HIGH: verbosity `medium` behaves identically to `quiet`** (README mismatch). **Fix:** make
  the three levels distinct — everything = prose+tools+decisions; medium = prose+decisions (no
  routine tool announcements); quiet = decisions only (no prose). Update daemon + README + tests.
- [ ] **HIGH: control commands raw-traceback when the daemon is down.** **Fix:** catch the
  connection error in `client.send`/`cli` and print a short friendly message (non-zero exit).
  test. (`client.py`, `cli.py`)
- [ ] **HIGH: `Speaker._current` data race + `say` `proc.wait()` has no timeout** (a hung `say`
  can stall the speak loop forever). **Fix:** lock around `_current` set/read in speak/cancel;
  add a wait timeout / robust cancel. test. (`speaker.py`)
- [ ] **HIGH: install interpreter** — `bin/echo` / `bin/echo-daemon` use bare `python3` via env,
  which won't resolve the `echo` package under launchd's minimal env, so the daemon won't start
  on install. **Fix:** make the shims / install use a correct absolute interpreter (the venv
  python). Verify via `doctor`. (`bin/*`, `cli.install`)
- [ ] **MEDIUM: `_clean_zshrc` / `_clean_settings_json` write your REAL files non-atomically**
  (corruption risk if killed mid-write). **Fix:** tmp-file + `os.replace`, like `config.save_config`.
  test. (`cli.py`)
- [ ] **HIGH (design): DRY** — `_connectable`/`_socket_connectable` duplicated across client+daemon;
  repo-root computation duplicated in 3 places. **Fix:** consolidate into one helper each.
- [ ] **Test coverage gaps** (add the high-value ones): multi-item speak-loop FIFO + wake path;
  real `_handle_conn` socket round-trip; REPEAT contract; client error paths; assembler fence
  spanning multiple `feed()` calls.

## Deferred — real but lower priority (next pass)

- [ ] Stop-hook transcript reconciliation (spec §5.2 safety net for a dropped final delta).
- [ ] `background_policy` is dead config (should_speak ignores it) — wire it or remove it.
- [ ] STOP vs CATCH_UP are currently identical — differentiate (catch-up = jump to newest) or document.
- [ ] SET_RATE range validation; Unix socket mode 0600; LaunchAgent plist XML-escaping + ThrottleInterval;
  install verifies the daemon shim is executable; unbounded per-connection recv buffer.
- [ ] Add missing slash commands (`/echo:voice`, `/echo:rate`, `/echo:skip`) or trim the README table.
- [ ] Minor clarity nits: `clean_markdown` double-call in assembler; mid-file imports in cli;
  `chr(10)`→`"\n"`; `_deep_merge(DEFAULTS, {})`→`copy.deepcopy`; unused `SpeechItem.id`; split the
  SET_FOREGROUND/SESSION_START branch; strengthen `test_commands.py` assertions.

## Resolved

- [x] **`load_config` deep-copy** — VERIFIED already correct: `load_config` deep-copies via
  `_deep_merge(DEFAULTS, {})`, and `tests/test_config.py` already has the exact regression test
  (mutate `cfg["earcons"]` → `DEFAULTS` unchanged). No change needed.
- [x] **Test-suite thread-exception warning** — FIXED (`d5c5d2d`): the `_echo_server` test helper
  now wraps recv/send in `try/except OSError`; suite is **0 warnings**.
- [x] **§3 `specPass:false` (5 tasks)** — audited & cleared (assembler verified correct by probe).
- [x] **§1 egg-info / clean tree** — verified: tree stays clean after editable install; egg-info ignored.
