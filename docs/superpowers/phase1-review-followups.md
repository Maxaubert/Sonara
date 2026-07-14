# Echo Phase 1 - Review Follow-ups

Durable backlog from subagent-driven execution + the final whole-implementation review
(`wf_dc5c640f-7c3`) and the fix pass (`wf_c6dc752d-76d`). Everything the review flagged as
fix-now is **Resolved**. "Deferred" items are real-but-lower-priority, logged for a later pass.

Suite after the fix pass: **242 passed, 0 warnings.**

---

## Deferred - real but lower priority (next pass / Phase 1.x)

- [ ] Stop-hook transcript reconciliation (spec §5.2 safety net for a dropped final delta).
- [ ] `background_policy` is dead config (should_speak ignores it) - wire it or remove it.
- [ ] STOP vs CATCH_UP are currently identical - differentiate (catch-up = jump to newest) or document.
- [ ] SET_RATE range validation; Unix socket mode 0600; LaunchAgent plist XML-escaping + ThrottleInterval;
  install verifies the daemon shim is executable; unbounded per-connection recv buffer.
- [ ] Add missing slash commands (`/echo:voice`, `/echo:rate`, `/echo:skip`) or trim the README table.
- [ ] Minor clarity nits: `clean_markdown` double-call in assembler; mid-file imports in cli;
  `chr(10)`→`"\n"`; `_deep_merge(DEFAULTS, {})`→`copy.deepcopy`; unused `SpeechItem.id`; split the
  SET_FOREGROUND/SESSION_START branch; strengthen `test_commands.py` assertions.

## Resolved

**Final-review fix pass (`wf_c6dc752d-76d`) - all verified with tests + a re-gate:**
- [x] **REPEAT control** implemented (`a2f51fb`) - daemon tracks `_last_spoken`; REPEAT re-enqueues it. (probe + 4 tests)
- [x] **Assembler reset on FLUSH** (`a81b84e`) - no stale prose leaks across turns. (cross-turn test)
- [x] **`afplay` earcons reaped** (`e2b9a8a`) - no zombie accumulation. (2 tests)
- [x] **Distinct verbosity levels** (`d2b6425`) - quiet drops prose; medium drops tools; README aligned. (7 tests; probe confirms quiet keeps only decisions)
- [x] **Friendly daemon-down error** (`5a2993f`) - `DaemonNotRunning`; CLI prints a message, exit 1, no traceback. (9 tests)
- [x] **Speaker thread-safety + wait timeout** (`192c12e`) - `_current` lock; 120s `wait` bound; robust cancel. (2 tests)
- [x] **Atomic user-file writes** (`0f46f10`) - `_clean_zshrc`/`_clean_settings_json` use tmp+`os.replace`. (tests)
- [x] **Absolute launchd interpreter** (`8651cc3`) - plist uses `sys.executable -m echo.daemon`. (2 tests)
- [x] **DRY** (`69067ae`) - `socket_connectable()` + `repo_root()` consolidated into `paths.py`. (6 tests)
- [x] **Test-coverage gaps** (`8217372`) - multi-item FIFO loop + wake path; real `_handle_conn` socket round-trip; assembler multi-feed fence.

**From the original backlog:**
- [x] **`load_config` deep-copy** - VERIFIED already correct (deep-copies via `_deep_merge`; regression test exists).
- [x] **Test-suite thread-exception warning** - FIXED (`d5c5d2d`); suite is 0 warnings.
- [x] **§3 `specPass:false` (5 tasks)** - audited & cleared (assembler verified correct by probe).
- [x] **§1 egg-info / clean tree** - verified clean after editable install.
