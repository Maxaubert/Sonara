# Phase 0 audit (2026-10-01)
Read-only multi-agent audit of main @ 8d90005. Every bug finding was adversarially re-verified; verdicts are included.

## Findings: prior-audit

I checked the current code (HEAD 8d90005) against every finding in AUDIT-2026-07-31.md. Only one is fully fixed. Almost all the rest are still present, mostly at the same lines (daemon.py lines have shifted by about 20 to 40).

FIXED: M12. The two launch_spec copies are now one: supervisor.py:742-758 delegates to supervisor_loop.launch_spec (#123). The daemon also logs which copy it runs, in "[daemon] started pid= root=" at daemon.py:1689-1690. The cross-cutting "log the resolved root" recommendation is done too.

PARTIAL: H1 is now one line to fix, because only one launch_spec is left (supervisor_loop.py:72). The duck_level and summary_settle_ms default drift is unchanged.

SKIPPED, Chatterbox-only (removal PR): M5 (tts.py Chatterbox rc=0 after dropped chunks), L15 (_warm_inflight coalescing, daemon.py:1226-1235) and the ChatterboxClient lock part of theme 6. The Chatterbox removal should also prune the chatterbox_* keys left in persisted config.json, which is blocked by M7.

STILL PRESENT, no change at all: H1, H2, H3, H4, M1-M4, M6-M11, M13, M14, and every LOW except the Chatterbox ones. The theme 1 "_signal_failure" channel does not exist. Only _signal_speak_failure is there (daemon.py:2100), and the swallow sites remain: daemon.py:1278, ducking.py:107, transport.py:106 / daemon.py:2893, settle_fire.

Hard sequencing still applies: M7 then M8 then H2.

Relevant to the user's "always read the last message" request: M1 (a parked reorder slot) and L-settle-fire (a lost seq) can both stop the latest digest from ever speaking. The raw-text fallback at daemon.py:1817-1828 is the existing "never skip the last message" path, and it only runs if the slot lands.

Test that locks in a bug: tests/test_win_earcon.py:50-51 still asserts 6 earcons, so it guards the H2 bug.

Related open issues: #40 (dead-code decisions), #53 (Kokoro download status/caching, overlaps M2), #69 (FLUSH clears a paused foreground voice).

The suite was not run (read-only task).

### H1 (critical, verify: partial) Daemon spawned with no cwd; summarizer resolves `claude` via shutil.which against the inherited project cwd

- **Where:** src/sonara/platform/windows/supervisor_loop.py:72-79 (kwargs, no cwd), src/sonara/daemon.py:2762-2763 (ensure_running Popen), src/sonara/summarizer.py:165
- **Evidence:** launch_spec kwargs = dict(creationflags, stdin, stdout, stderr=err, env=env), no cwd. summarizer.py:165 `exe = shutil.which(argv[0]) or argv[0]` (Windows which() searches cwd first; bare-name fallback). Lazy start from a hook inherits the project dir, which the daemon then pins (can't delete/rename folder).
- **Fix:** Add cwd=str(paths.SONARA_DIR) to the shared launch_spec kwargs (one place now, since #123 unified it). In summarizer use shutil.which(argv[0], path=os.environ.get('PATH')) and fail (return None + log) instead of falling back to the bare name.
- **Verifier:** Confirmed: launch_spec kwargs have no cwd (supervisor_loop.py:72-79) and ensure_running Popens them (daemon.py:2762-2763), so a hook-launched daemon inherits and pins the project dir; no chdir anywhere in src. The summarizer hijack is overstated. summarizer.py:165 uses which(), and an empirical test on Py3.14 shows which('claude') returns .\claude.CMD from cwd ONLY when NoDefaultCurrentDirectoryInExePath is unset. This Claude Code environment sets NDCD=1, which hooks and thus the lazily started daemon likely inherit; the logon task uses work_dir=APP_DIR. Folder pinning is real (medium). Hijack is conditional, not critical.

### H2 (high, verify: confirmed) nav, nav_edge, session_change, summary_failed earcons have no bundled wav: silent no-ops

- **Where:** src/sonara/platform/windows/earcons/generate.py:108-116; callers src/sonara/daemon.py:822,828,831,907,979,1826,2512,2547,2564; test tests/test_win_earcon.py:50-51
- **Evidence:** _EARCON_SPECS still defines only permission, choice, plan, error, turn_done, ready; earcons/ dir ships those 6 wavs only. test_default_earcons_six still asserts len == 6. plan/ready still have no caller.
- **Fix:** Add the 4 specs + wavs, drop plan/ready, replace the count test with set-equality against literals passed to _earcon(). Lands for existing users only after M7/M8.
- **Verifier:** _EARCON_SPECS (generate.py:108-115) and the earcons/ dir hold only the 6 wavs. Speaker.earcon returns silently on a missing key (speaker.py:187-189), so nav/nav_edge/session_change/summary_failed (daemon.py:822,828,831,907,979,1826,2512,2547) are no-ops. plan/ready have no caller: hooks_entry only sends choice/permission/turn_done EARCONs (51,84,99). tests/test_win_earcon.py:50-51 asserts len==6.

### H3 (high, verify: confirmed) `sonara install` from the deployed launcher writes STOPPED sentinel then fails copy, leaving Sonara permanently off

- **Where:** src/sonara/cli.py:639 (repo_root), 646 (stop_sonara), 650-655 (return 1 before), 676 (sentinel clear); src/sonara/paths.py:77-89
- **Evidence:** plugin_root = realpath(paths.repo_root()); repo_root docstring now admits it returns ~/.sonara under APP_DIR, but install() still uses it with no guard; OSError path returns 1 before os.remove(STOPPED_SENTINEL_PATH).
- **Fix:** Before stop_sonara(): verify <plugin_root>/src/sonara and bin/sonara-hook exist, else print and return. Then resolve plugin root from INSTALL_RECORD / CLAUDE_PLUGIN_ROOT. Also clear sentinel (or say 'run sonara start') on the copy-failure path.
- **Verifier:** The deployed launcher sets PYTHONPATH=APP_DIR (supervisor.py:868-871), so paths.repo_root() returns ~/.sonara (paths.py:77-89). install() calls stop_sonara (cli.py:646, writes sentinel at cli.py:530). Then _copy_app copytree's the missing ~/.sonara/src/sonara (cli.py:454,467), which raises FileNotFoundError (OSError) and returns 1 at cli.py:650-655 before the sentinel removal at cli.py:676. Sonara stays off until `sonara start`.

### H4 (high, verify: confirmed) Windows HOOKS_JSON_TEMPLATE still lacks PostToolUse; CHOICE_ANSWERED never fires for settings.json installs

- **Where:** src/sonara/platform/windows/supervisor.py:264-410 (template events: MessageDisplay, PreToolUse, Notification, Stop, UserPromptSubmit, SessionStart, SessionEnd); src/sonara/hooks_entry.py:72
- **Evidence:** hooks/hooks.json has 8 events incl. PostToolUse; template has 7. No test compares the sets.
- **Fix:** Generate the Windows hooks from hooks/hooks.json (or add PostToolUse) plus a set-equality test.
- **Verifier:** HOOKS_JSON_TEMPLATE (supervisor.py:264-410) has 7 events: MessageDisplay, PreToolUse, Notification, Stop, UserPromptSubmit, SessionStart, SessionEnd. hooks/hooks.json also has PostToolUse, which drives CHOICE_ANSWERED (hooks_entry.py:72-77). This only affects the settings.json path (plugin not enabled).

### M1 (medium, verify: partial) A hung summarizer worker parks every later digest forever (no wall-clock bound on reorder slot)

- **Where:** src/sonara/daemon.py:1723-1744 (_land_digest), 1752-1786 (fn() before landing), 1857-1869; src/sonara/summarizer.py:171-180
- **Evidence:** Slot only lands in the worker's finally after fn() returns; subprocess.run(timeout=) can block on Windows when a grandchild holds the pipes. Hold-release cap (#121) bounds the held question only, not the digest seq.
- **Fix:** Watchdog timer per dispatched seq that calls _land_digest(seq, None) after ~2x summary_timeout; worker's later land is already ignored via seq < serve.
- **Verifier:** Confirmed: a slot only lands in the worker's finally after fn() (daemon.py:1772-1779, 1857-1864), _land_digest blocks later seqs (1736-1743), and no watchdog exists. The trigger is weak: summarizer catches timeout (summarizer.py:214), and the grandchild-holds-pipes hang needs a shim. Here `claude` resolves to native claude.EXE. It is real for npm .cmd shims and other providers. Severity is low-medium.

### M2 (medium, verify: confirmed) Kokoro model download has no timeout and runs under KokoroEngine._lock

- **Where:** src/sonara/kokoro.py:163 (urlretrieve), 214-217 (_ensure under _lock)
- **Evidence:** urllib.request.urlretrieve(url, tmp) with default socket timeout None, called from _ensure_loaded while holding self._lock.
- **Fix:** Chunked urlopen(url, timeout=30) copy; surface failure (cf. open issue #53).
- **Verifier:** kokoro.py:163 calls urlretrieve with no timeout, and no setdefaulttimeout exists anywhere in src. _ensure runs inside `with self._lock` (kokoro.py:214-217), so a stalled download blocks every Kokoro synth.

### M3 (medium, verify: confirmed) STOP clears channels only; settle timers, in-flight digests and held decisions keep producing speech

- **Where:** src/sonara/daemon.py:792-798
- **Evidence:** STOP handler: _drop_channel_pending + ch.wipe() + speaker.cancel(); does not touch _settle_gen/_settle_pending/_summary_gen/_held_decision/_digest_parked. _flush_all (1435-1459) does more, teardown lists still diverge.
- **Fix:** STOP calls a shared _silence_everything() (reuse _flush_all + _cancel_settle for every session + bump _summary_gen).
- **Verifier:** The STOP handler (daemon.py:792-798) only does _drop_channel_pending + wipe + speaker.cancel. It leaves _settle_timers, _summary_gen, _held_decision, _digest_parked and _pending_preamble alone, unlike _flush_all (1435-1459). STOP comes from `sonara stop` (cli.py:159).

### M4 (medium, verify: confirmed) Hotkey start failure (e.g. bad keymap.json) swallowed with bare pass

- **Where:** src/sonara/daemon.py:1272-1279
- **Evidence:** try: keymap.migrate_default_chord(); backend.start(...) except Exception: pass -- no log, no cue, unlike _announce_hotkey_collisions.
- **Fix:** Log traceback to stderr and speak/earcon an error cue.
- **Verifier:** daemon.py:1272-1279 wraps migrate_default_chord + backend.start + collisions in `except Exception: pass` with no log or cue.

### M6 (medium, verify: partial) Settings voice preview plays on the single winsound speech channel and cuts live speech (marked heard)

- **Where:** src/sonara/daemon.py:2343-2372 (preview_voice uses tts.run); src/sonara/platform/windows/tts.py:221-225 (_complete sets returncode 0 on timer)
- **Evidence:** Preview runs runner(text, voice, rate) on its own thread; winsound has one channel; the interrupted utterance's Timer still reports rc=0.
- **Fix:** Route preview through the speech queue (CONTROL item) or the earcon helper process; refuse when speaking.
- **Verifier:** The mechanism is confirmed: preview_voice runs tts.run on its own thread (daemon.py:2343-2372). tts plays via winsound SND_ASYNC (tts.py:268), so a new PlaySound cuts the live one, and the cut handle's Timer still sets returncode=0 (tts.py:221-225). Reach is limited: settings.html:745-753 plays the pre-rendered /api/preview-audio file in the browser and only POSTs /api/preview as an onerror fallback when no preview file exists.

### M7 (medium, verify: confirmed) save_config persists fully-merged DEFAULTS, so default changes never reach existing installs

- **Where:** src/sonara/config.py:93-101 (save_config), 72-90 (load_config)
- **Evidence:** json.dump(cfg) of the whole merged dict; no config_version, no diff-vs-DEFAULTS, no pruning of retired keys (chatterbox_* keys will linger after Chatterbox removal).
- **Fix:** Persist only keys differing from DEFAULTS (or add config_version + migration); prune unknown keys on load.
- **Verifier:** save_config dumps the full merged dict (config.py:93-101), and load_config merges persisted over DEFAULTS (72-90). Persisted values always win, there is no version or diff, and retired keys (chatterbox_*) are never pruned.

### M8 (medium, verify: confirmed) Earcon absolute paths frozen into config on first save

- **Where:** src/sonara/daemon.py:2911-2912
- **Evidence:** if 'earcons' not in cfg: cfg['earcons'] = default_earcons() -- persisted by the next save_config; never revalidated, blocks the H2 fix for existing users.
- **Fix:** Always resolve earcons from the package at startup; keep only an optional user override key.
- **Verifier:** daemon.py:2911-2912 injects default_earcons() into cfg, which becomes daemon.config (2924). Any later save_config persists it, and the `'earcons' not in cfg` guard then skips refresh forever, so new earcon kinds never reach existing users.

### M9 (medium, verify: confirmed) Shared WinRT SpeechSynthesizer mutated from multiple threads without a lock

- **Where:** src/sonara/platform/windows/tts.py:447 (_get_synth), 564-575 (synth.voice / opts.speaking_rate)
- **Evidence:** No lock around voice+rate+synthesize; reached from speak loop, preview builder (daemon.py ~2321) and preview_voice thread.
- **Fix:** One lock around the set-voice/rate/synthesize sequence, or a per-call synthesizer.
- **Verifier:** _synthesize_wav_blocking mutates the shared self._synth (tts.py:564-575) with no lock; the file has no Lock. The same backend singleton (platform/__init__.py:11-19) is reached from the speak loop, previews.synth_wav (previews.py:62,72, preview builder thread) and the preview_voice thread.

### M10 (medium, verify: confirmed) Ducking loses its undo record (partial loop failure; finally clears state file on failed restore)

- **Where:** src/sonara/platform/windows/ducking.py:88-108 (duck), 152-183 (restore_from_state_file finally _clear_state)
- **Evidence:** duck(): no per-session try; _saved/_ducked/_write_state committed only after the loop, outer except: pass. restore_from_state_file: finally: _clear_state() even if the sweep failed.
- **Fix:** Per-session try/except with incremental _saved + state write; clear state only on success.
- **Verifier:** duck() (ducking.py:88-108) has no per-session try. An exception mid-loop leaves earlier sessions ducked while _saved, _ducked and the state file stay unset (outer except: pass). restore_from_state_file clears state in finally even when the sweep failed (183).

### M11 (medium, verify: confirmed) Single-instance mutex is machine-wide Global\ and failure path is silent

- **Where:** src/sonara/platform/transport.py:92, 106-108; src/sonara/daemon.py:2892-2893
- **Evidence:** _MUTEX_NAME = 'Global\\Sonara-Daemon-Singleton-v1'; `if not handle: return None` conflates CreateMutexW failure with 'already owned'; main() returns with no log.
- **Fix:** Use Local\ prefix; distinguish failure (log GetLastError) from ERROR_ALREADY_EXISTS; log one line on exit.
- **Verifier:** transport.py:92 uses Global\ (machine-wide), so a second Windows user's CreateMutexW gets access-denied or NULL. Line 106-107 returns None for that too, the same as ALREADY_EXISTS, and daemon.py:2892-2893 returns silently. Impact is limited to multi-user machines.

### M13 (medium, verify: confirmed) daemon writes Router private announce fields and re-arms only two of three

- **Where:** src/sonara/daemon.py:419-420 (_requeue_or_note), 1456-1458 (_flush_all); src/sonara/router.py:25-27,138-140
- **Evidence:** Sets _pending_announce and _pending_announce_replay=False but never _pending_announce_manual, so a paused manual switch resumes down the deferred-preamble path.
- **Fix:** Add Router.rearm_announce(session, replay, manual) / clear_announce() and use them.
- **Verifier:** next_item clears _pending_announce_manual on emission (router.py:224-227). _requeue_or_note re-arms only _pending_announce and replay=False (daemon.py:419-420), ignoring item.manual. A paused manual switch therefore resumes as manual=False, the deferred-preamble path (#111). It also drops the replay flag. _flush_all at 1456-1458 does the same.

### M14 (medium, verify: confirmed) Bootstrap writes interpreter path records as ASCII

- **Where:** bin/sonara-bootstrap.ps1:87-88
- **Evidence:** Set-Content ... -Encoding ASCII for python.path and pythonw.path; paths._read_recorded (src/sonara/paths.py:33-39) reads UTF-8; bin/sonara.cmd:12 and bin/sonara-hook.cmd:20-21 consume them unchecked.
- **Fix:** -Encoding UTF8 (note PS5 adds a BOM; strip it in _read_recorded, or write via [IO.File]::WriteAllText with UTF8Encoding($false) since cmd set /p also chokes on BOM/non-ANSI).
- **Verifier:** sonara-bootstrap.ps1:87-88 writes with -Encoding ASCII, so non-ASCII chars in the interpreter path become '?'. _read_recorded then fails is_file (paths.py:33-39), and sonara.cmd / sonara-hook.cmd `set /p` consumes them unchecked.

### L-settle-default (low, verify: confirmed) summary_settle_ms missing from DEFAULTS; settings page shows null

- **Where:** src/sonara/config.py:9-50; src/sonara/webui.py:23; src/sonara/settings.html:601-602; src/sonara/daemon.py:1584
- **Evidence:** Key in _PAGE_KEYS and settings.html but not DEFAULTS; daemon uses inline .get(...,600).
- **Fix:** DEFAULTS['summary_settle_ms'] = 600; drop inline fallback.
- **Verifier:** summary_settle_ms is absent from DEFAULTS (config.py:9-50) but present in webui _PAGE_KEYS (webui.py:23,40) and settings.html:601-602. The daemon uses an inline .get(...,600) (daemon.py:1584).

### L-duck-level (low, verify: confirmed) _duck_level fallback 20 vs DEFAULTS 30

- **Where:** src/sonara/daemon.py:2255-2259
- **Evidence:** self.config.get('duck_level', 20) and except branch return 20; config.py:17 says 30.
- **Fix:** Fall back to DEFAULTS['duck_level'].
- **Verifier:** daemon.py:2257,2259 fall back to 20 while DEFAULTS is 30 (config.py:17). The .get default is mostly unreachable since load_config merges DEFAULTS; the except path (non-int value) does return 20.

### L-interp-dup (low, verify: confirmed) Duplicate interpreter resolution and _probe_python_version copies

- **Where:** src/sonara/platform/windows/supervisor.py:167 vs 691; supervisor.py:193 (daemon_pythonw) vs src/sonara/cli.py:384 (_daemon_python)
- **Evidence:** Module copy used by spawn, method copy is the one tests monkeypatch.
- **Fix:** One function; method delegates to module function.
- **Verifier:** Module _probe_python_version (supervisor.py:167-178) duplicates the method (supervisor.py:691+). daemon_pythonw (supervisor.py:193+) duplicates cli._daemon_python (cli.py:384-395) logic.

### L-speaker-race (low, verify: confirmed) Speaker.speak cancel/abandon window can leak a playing proc

- **Where:** src/sonara/speaker.py:95-99 vs 108-114
- **Evidence:** Helper reads abandoned=False under lock, releases, then done.set(); caller cancel in between sets abandoned=True and returns False without terminating state['proc'].
- **Fix:** In caller, after setting abandoned under the lock, terminate state['proc'] if already set.
- **Verifier:** The helper reads abandoned=False under the lock and releases (speaker.py:93-95), then calls done.set(). The caller's wait loop can time out in between, set abandoned=True (108-111) and return False (113-114) without checking state['proc']. Neither side terminates proc, so it plays. The comment's 'exactly one side' claim fails.

### L-preamble (low, verify: confirmed) _pending_preamble check-then-act outside the lock

- **Where:** src/sonara/daemon.py:2532-2536, 2573-2575 (on_play at 2547 clears it from another thread)
- **Evidence:** Double read of self._pending_preamble without snapshot; on_play callback sets None concurrently.
- **Fix:** Snapshot into a local (or guard with self._lock).
- **Verifier:** daemon.py:2532-2534 and 2572-2573 read self._pending_preamble twice without a lock. on_play (2546) and _flush_all (1453-1454) set it None from other threads, so a None can land between the check and the [0] subscript (TypeError). The window is narrow.

### L-settle-pop (low, verify: confirmed) _teardown_session pops _settle_gen right after _cancel_settle bumped it

- **Where:** src/sonara/daemon.py:370-371
- **Evidence:** self._cancel_settle(session); self._settle_gen.pop(session, None) resets the monotonic guard so a blocked timer with gen 1 can pass after re-arm.
- **Fix:** Remove the pop (bump only), matching _summary_gen at 369.
- **Verifier:** _cancel_settle bumps the gen (daemon.py:1617), then _teardown_session pops it (371). _arm_settle restarts at gen 1 (1572), so an old fire with gen==1 blocked on the lock passes the guard at 1596. The window is narrow (fire blocked during teardown, then re-arm before it runs).

### L-settle-fire (low, verify: confirmed) _settle_fire has no catch-all

- **Where:** src/sonara/daemon.py:1590-1612
- **Evidence:** Exception after _settle_pending.discard kills the Timer thread; turn lost silently, an allocated seq may never land (M1 wedge).
- **Fix:** try/except around body, log, land any allocated seq.
- **Verifier:** _settle_fire (daemon.py:1590-1612) has no try/except. An exception after discard/pop kills the Timer thread, and if _start_summary_thread fails after _alloc_digest_seq (1564-1566) the seq never lands, wedging later digests.

### L-pause-state (low, verify: confirmed) resume_from_state_file clears state in finally even on failure

- **Where:** src/sonara/platform/windows/pausing.py:153-176
- **Evidence:** finally: _clear_state().
- **Fix:** Clear only on success.
- **Verifier:** pausing.py:174-176 calls `finally: _clear_state()` even when the whole sweep raised (e.g. SMTC manager unavailable), losing the record. Low impact.

### L-replay-decision (low, verify: confirmed) _replay inserts items directly and never sets ch.has_decision

- **Where:** src/sonara/daemon.py:1344-1360
- **Evidence:** ch.items.insert(at, item) with is_decision possibly True; has_decision untouched (cf. 2016 where it is recomputed).
- **Fix:** After insert: ch.has_decision = any(it.is_decision for it in ch.items[ch.cursor:]).
- **Verifier:** _replay inserts items with is_decision possibly True (daemon.py:1347-1355) without updating ch.has_decision, unlike 2016. A replayed decision does not get the router preempt (router.py:168), though turn_done=True still makes it ready. Low impact, arguably benign.

### L-duck-pid (low, verify: confirmed) Duck crash-restore matches by raw PID

- **Where:** src/sonara/platform/windows/ducking.py:172-178
- **Evidence:** by_pid.get(s.ProcessId) tried first; recycled PID gets unrelated volume.
- **Fix:** Require name match too (or PID+start time).
- **Verifier:** ducking.py:174-176 tries by_pid first, so a recycled PID of a different audio app gets the recorded original volume. Low.

### L-log (low, verify: confirmed) speechd.log never rotates and stderr handle leaks per spawn

- **Where:** src/sonara/platform/windows/supervisor_loop.py:71 (open per launch_spec), 105-107 loop; src/sonara/daemon.py:2762-2763
- **Evidence:** err = open(LOG_PATH, 'a') each call, never closed in parent; no size check/rotation anywhere.
- **Fix:** Rotate when > N MB inside launch_spec; close err in parent after Popen.
- **Verifier:** supervisor_loop.py:71 opens LOG_PATH on every launch_spec call. The parent never closes it, which leaks one handle per respawn in the supervisor loop (105-107). No size check or rotation exists for speechd.log; the only 'rotate' at daemon.py:2788 is for the fault file.

### L-xml (low, verify: confirmed) Task Scheduler XML built without escaping

- **Where:** src/sonara/platform/windows/supervisor.py:96-102
- **Evidence:** TASK_XML_TEMPLATE.format(user_id=..., pythonw=..., supervisor_py=..., work_dir=...) raw; '&' in user/path breaks XML; schtasks output discarded (DEVNULL).
- **Fix:** xml.sax.saxutils.escape each value.
- **Verifier:** supervisor.py:98-103 formats user_id/pythonw/supervisor_py/work_dir into XML unescaped, so '&' or '<' in a path or user breaks it. schtasks output goes to DEVNULL (supervisor.py:~113), leaving only the rc.

### L-dead (low, verify: confirmed) Dead code still present

- **Where:** src/sonara/daemon.py:315 (_drop_pending), 2021 (_resume), 2252 (_audio_pause_on); src/sonara/keymap.py:247-260 (write_resolved, called cli.py:664); src/sonara/paths.py:21-22 (HOTKEYD_RESOLVED_PATH, HOTKEYD_BIN_PATH); src/sonara/kokoro.py:18 (unused import os)
- **Evidence:** grep finds zero callers for the three daemon methods in src/tests/bin/hooks/tools; hotkeyd.resolved.json written each install, never read (only cleaned in cli.py:733); os never referenced in kokoro.py.
- **Fix:** Delete; keep cli.py:733 cleanup of the stale file for one release. See open issue #40.
- **Verifier:** _drop_pending (daemon.py:315), _resume (2021) and _audio_pause_on (2252) have zero callers (grep -w over src/tests/bin/hooks/tools). keymap.write_resolved is only called at cli.py:661 (not 664) plus tests, and the file is never read. HOTKEYD_BIN_PATH is used only in tests. `import os` (kokoro.py:18) is unused. Open issue #40 tracks dead code.

### L-voice-routing (low, verify: confirmed) Voice-to-engine routing duplicated (partial: collapses once Chatterbox goes)

- **Where:** src/sonara/platform/windows/tts.py:640,668; src/sonara/daemon.py:1664-1666, 2165; src/sonara/previews.py:65-68
- **Evidence:** is_kokoro_voice/is_chatterbox_voice branching in 4 places.
- **Fix:** voices.engine_for(voice) during Chatterbox removal.
- **Verifier:** Engine branching by is_kokoro_voice/is_chatterbox_voice appears at tts.py:640,668, daemon.py:1247,1664-1666,2165,2200 and previews.py:65-68. It largely collapses once Chatterbox is removed.

## Findings: core-runtime

Scope: I read the core runtime in full (daemon.py, router.py, queue.py, channel.py, speaker.py, history.py, sessions.py, session_prefs.py, digest_store.py, assembler.py, summarizer.py). I did not open cleaner.py, which is only called by the assembler. I made no edits. I confirmed F1, F2 and F3 with scratch probes under $TMP/sonrev (outside the repo); F4 to F8 come from reading the code. The known Up-key nav_edge issue is excluded.

Issue #69: STILL REAL (F1). Its file paths (src/sonari/daemon/features/prose.py) are stale, and the live code is daemon.py:764. It is also worse than the issue says. The issue assumes the #65 foreground gate exists, but SET_FOREGROUND in daemon.py:769-783 is unconditional, so a background session's UserPromptSubmit also takes the foreground. #65 is CLOSED, yet its gate is not in this tree.

Issue #59: STALE, recommend closing. The per-session design is implemented: SessionChannel (channel.py), Router with auto hand-off, manual next_session ring, suppression, CONTROL lane and per-session mute (router.py). Pause is a single speaker-level halt, as the spec intends. No _voice_owner, SpeechQueue or _open_msg remains (grep of src/ is empty). Commits tagged (#59) such as c92cd2d and 94a98b6 landed it. The only leftover is F1's cross-session un-pause, which belongs to #69.

Checked and found no problem: router suppression keyed on gen (#115), the _replay_authorized eviction, the _requeue_or_note pause rewind, settle and hold-release generation guards, the _summary_worker in-flight accounting, the conn semaphore, the hotkey worker locking, and atomic writes in sessions, session_prefs and digest_store. All persistence fsyncs run under the daemon lock but are best-effort and throttled; not a bug.

### F1 (high, verify: confirmed) Issue #69 still real: any session's FLUSH clears the global pause, and SET_FOREGROUND is ungated (the #65 gating is missing in this codebase)

- **Where:** src/sonara/daemon.py:764, src/sonara/daemon.py:769-783, src/sonara/hooks_entry.py:101-106
- **Evidence:** Every UserPromptSubmit sends SET_FOREGROUND and then FLUSH (hooks_entry.py:101-106). The FLUSH handler runs `self._paused.clear()` at daemon.py:764 with no check on who owns the voice. The SET_FOREGROUND handler calls sessions.set_foreground at daemon.py:771 with no gating at all. The #65 fix that issue #69 refers to (the on_set_foreground gate in src/sonari/.../prose.py) does not exist in this tree. Probe run: fg=A, _paused.set(), then SET_FOREGROUND(B) and FLUSH(B) gives paused=False and fg=B. So a background session's prompt both un-pauses A and takes the foreground. The file paths in issue #69 are stale but the bug is live.
- **Fix:** In FLUSH, clear _paused only when the flushing session is the engaged/foreground session (`if self._engaged_session() == session`). Decide separately whether to gate SET_FOREGROUND for programmatic re-invocations; #65 is closed but its gate is absent here. Add a regression test: paused fg A plus FLUSH(B) leaves it paused, and FLUSH(A) still auto-resumes.
- **Verifier:** daemon.py:764 runs self._paused.clear() inside the FLUSH handler for any session, with no foreground or engaged check. hooks_entry.py:100-106 sends SET_FOREGROUND then FLUSH on every UserPromptSubmit. The SET_FOREGROUND handler at daemon.py:769-771 calls sessions.set_foreground, which (sessions.py:80-82) just records the session and sets _foreground, with no gate. Issue #69 is OPEN and describes exactly this, with stale src/sonari paths. The #65 on_set_foreground gate it relies on is not in this tree. One nuance: when the user really types in B, resuming is arguably intended. The bug is the programmatic background re-invocation case (/loop ticks, agent completions). At daemon.py:779-783 that case also replay-authorizes A's pending items to drain, so A resumes on its own.

### F2 (high, verify: confirmed) Parked short background-turn digest has no cancel or teardown guard: it resurrects ended sessions and speaks into a new turn

- **Where:** src/sonara/daemon.py:1545-1546, src/sonara/daemon.py:1885-1902, src/sonara/daemon.py:1723-1743
- **Evidence:** A short non-foreground turn is landed with `self._land_digest(self._alloc_digest_seq(), lambda: self._enqueue_background_digest(session, text))`. If an earlier seq is still cooking, this lambda waits in _digest_parked. Unlike the worker's apply() (daemon.py:1807), it never checks _summary_gen. FLUSH and SESSION_END/_teardown_session do not touch _digest_parked. Probe: A's long digest in flight (seq 0), B's short turn parked (seq 1), SESSION_END(B), then seq 0 lands. Result: router.channels['B'] is recreated holding 'short turn text.', and digest_store['B'] is re-written. The forgotten session is resurrected and will rehydrate after a restart. With FLUSH(B) instead, the stale text is spoken into B's new turn and replaces its seed and _last_digest_text.
- **Fix:** Capture `gen = self._summary_gen.get(session, 0)` at dispatch, and inside the lambda return early if the gen has changed (same guard as apply()). Optionally land the session's parked slots dead in FLUSH and _teardown_session.
- **Verifier:** daemon.py:1545-1546 lands lambda: self._enqueue_background_digest(session, text), and the lambda has no _summary_gen check, unlike the worker's apply() at about daemon.py:1807. _digest_parked is only touched at daemon.py:198, at 1450-1452 (_flush_all) and at 1738-1740 (_land_digest). The FLUSH handler (around 750-766) and _teardown_session (353) do not touch it. When the parked lambda fires, _enqueue_background_digest (daemon.py:1885-1902) calls router.channel(session), which recreates the channel, and digest_store.set re-persists the text. So a session that has ended gets resurrected, and after a FLUSH the stale text plays in the new turn and overwrites _last_digest_text and the seed.

### F3 (medium, verify: confirmed) on_play still fires from an abandoned (cancelled) synthesis: stray 'Session changed' alert, preamble cleared, other apps ducked or media paused

- **Where:** src/sonara/speaker.py:86-114, src/sonara/platform/windows/tts.py:687-692, src/sonara/daemon.py:2541-2556
- **Evidence:** speak() abandons the helper on cancel (#116), but the helper still calls say_runner, and tts.run calls on_play() before returning the proc (tts.py:662-666, 687-691). Probe with a fake runner and a cancel during synth: speak returned False, events ['on_play','terminated']. On_play then runs on the sonara-synth thread with no daemon lock. It sets self._pending_preamble=None, which undoes the 'keep preamble armed on pause' intent at 2542-2544, so the resumed content loses its handoff announcement. It also plays the chime plus speak_cue_untracked('Session changed: X') while the loop is paused or already voicing the next item, and calls _maybe_engage_audio. That is an unlocked check-then-act on ducker/pauser, racing _maybe_restore_audio from the PAUSE handler and the loop. In 'pause' audio mode it toggles the user's media.
- **Fix:** In Speaker.speak, wrap on_play in a guard that, under _current_lock, returns without calling it when state['abandoned'] is set or the epoch has moved. Optionally have the daemon's on_play re-check its item is still _current_item.
- **Verifier:** The speaker.py:84-90 helper calls say_runner(..., on_play). tts.py:687-692 (and 662-666) invokes on_play() inside say_runner before _play_wav_bytes returns the proc. The abandoned flag is only checked after say_runner returns (speaker.py:93-104), so a cancelled synthesis still runs on_play. The daemon's on_play (daemon.py:2541-2554) clears _pending_preamble, which defeats the intent stated at 2542-2544. It also plays the earcon and speak_cue_untracked on the synth thread, then calls _maybe_engage_audio (daemon.py:2392-2399). That method does an unlocked check-then-act on ducker/pauser and can re-duck, or re-pause the user's media, after a PAUSE has restored audio.

### F4 (medium, verify: confirmed) Speaker cancel race can orphan a playing utterance that no later cancel can stop

- **Where:** src/sonara/speaker.py:93-110
- **Evidence:** The helper sets state['proc'] and reads abandoned=False under the lock, releases it, then calls done.set(). If the caller's 50ms poll fires in that gap after a cancel(), it sets abandoned=True and returns False (lines 104-110). The helper already read abandoned=False, so it never terminates. The caller never registers the proc as _current, so the audio plays to the end and cancel()/pause/mute cannot cut it. The window is narrow but real.
- **Fix:** In the caller's abandon branch, under the lock: `if state['proc'] is not None: proc_to_kill = state['proc']`, and terminate it after releasing the lock. Alternatively set abandoned and read proc atomically in one critical section.
- **Verifier:** In speaker.py:93-96 the helper sets state['proc'] and reads abandoned=False under _current_lock, then releases it and calls done.set() at line 97. If the caller's done.wait(0.05) at line 106 has already timed out and the caller is blocked on the lock the helper holds, the caller gets the lock next. It sees the epoch has moved, sets abandoned=True and breaks (lines 107-110), then returns False at 111-112 without looking at state['proc']. The helper does not terminate the proc because it already read False, and the proc is never set as self._current, so cancel() cannot reach it. The window is narrow but reachable, because lock contention makes the caller wait exactly at that point.

### F5 (medium, verify: confirmed) Summarizer timeout is not enforced for a .CMD command (codex): subprocess.run blocks in communicate() and freezes the reorder buffer

- **Where:** src/sonara/summarizer.py:177-185, src/sonara/daemon.py:1773-1780, src/sonara/daemon.py:1736-1743
- **Evidence:** `shutil.which('codex')` gives C:\Users\Admin\AppData\Roaming\npm\codex.CMD here. On a TimeoutExpired on Windows, CPython's run() kills only cmd.exe and then calls process.communicate() with no timeout. The node grandchild keeps the stdout/stderr pipes open, so the worker hangs until node exits. While it hangs, its digest seq never lands, and every later turn-end digest from every session parks in _digest_parked (daemon.py:1739). Only the held question is freed, by the hold-cap timer. claude resolves to claude.EXE, so the default config is unaffected.
- **Fix:** Use Popen with CREATE_NEW_PROCESS_GROUP and, on timeout, kill the process tree (`taskkill /T /F /PID`). Or call communicate(timeout=...) yourself and close the pipes before raising. Add a test with a runner that sleeps past the timeout.
- **Verifier:** summarizer.py:171-179 uses subprocess.run(..., capture_output=True, timeout=...). On Python 3.14.7 on this machine, subprocess.run's TimeoutExpired branch calls process.kill(), then on Windows process.communicate() with no timeout. shutil.which('codex') resolves to ...\npm\codex.CMD, so the kill only hits cmd.exe, and the node grandchild holds the pipes open, so communicate() blocks. build_argv has a codex branch (summarizer.py:~148-151), so this is a supported configuration. The stuck worker never reaches its finally-land, so later seqs park in _digest_parked (daemon.py:1738-1743). The default claude resolves to claude.EXE, so it is unaffected.

### F6 (low, verify: confirmed) Rate, verbosity and nav cues still go through _enqueue into the session channel: they wipe the #118 seed and wait on the minqueue gate

- **Where:** src/sonara/daemon.py:1048, src/sonara/daemon.py:1172, src/sonara/daemon.py:1920, src/sonara/daemon.py:276-279
- **Evidence:** _enqueue wipes a seeded channel (276-279), which also resets turn_done=False. So 'Rate 210.' / 'Verbosity medium.' / 'Nothing to navigate yet.' destroy the persisted-digest placeholder that keeps the session in the manual cycle ring (#117/#118). With minqueue>1 during a live stream they also sit unplayed until the turn flushes. _speak_cue's docstring (2121-2127) says this is exactly why cues must go to CONTROL.
- **Fix:** Use self._speak_cue(...) for these three call sites.
- **Verifier:** daemon.py:1048 ('Rate N.'), daemon.py:1172 ('Verbosity X.') and daemon.py:1920 ('Nothing to navigate yet.') all call self._enqueue(fg, 'prose', ...). _enqueue wipes a seeded channel (daemon.py:276-279), and Channel.wipe (channel.py:83-89) resets turn_done=False and seeded=False. That destroys the #118 seed set at daemon.py:735-744. The NAV case is the worst one, because 'Nothing to navigate yet' fires exactly when history is empty right after FLUSH, which is when the seed exists. The _speak_cue docstring (daemon.py:2119-2125) says cues must use CONTROL to bypass the minqueue ready() gate (daemon.py:592).

### F7 (low, verify: partial) Single-slot _pending_decision is overwritten inside the settle window, which drops a decision and leaks its _pending_heard entry

- **Where:** src/sonara/daemon.py:629, src/sonara/daemon.py:650, src/sonara/daemon.py:679
- **Evidence:** In summary mode each CHOICE, PLAN or PERMISSION does `self._pending_decision[session] = item`. A second decision for the same session inside summary_settle_ms (600ms) replaces the first, so the first is never enqueued, and its id stays in _pending_heard forever (it is popped only on speak or channel drop).
- **Fix:** Store a list per session (append, then enqueue/hold all in _settle_fire), or pop the replaced item's _pending_heard entry and enqueue it before replacing.
- **Verifier:** The mechanism is as described: daemon.py:629, 650 and 679 overwrite _pending_decision[session], and the replaced item's _pending_heard entry (set at 618/645/675) is never popped. FLUSH (daemon.py:757) also pops _pending_decision without popping _pending_heard, which is a second leak path. Reachability within the 600ms settle window is low, though. CHOICE and PLAN come from PreToolUse of blocking tools (hooks_entry.py:49-62). A PERMISSION after a CHOICE is consumed by _await_choice (daemon.py:660-662), and the paired permission notification arrives about 5-6s later (comment at daemon.py:619-621). So this needs a rare parallel or quickly-answered decision. The impact is a small dict leak plus a dropped decision that the user was in practice already prompted for.

### F8 (low, verify: confirmed) STOP wipes the channels but leaves in-flight and parked digests alive

- **Where:** src/sonara/daemon.py:792-798
- **Evidence:** STOP wipes every channel and cancels the speaker. It does not bump _summary_gen, kill _digest_parked slots or cancel settle timers (compare _flush_all at 1430-1459). A digest that lands seconds later starts speaking after the user pressed stop.
- **Fix:** Implement STOP via _flush_all() plus the wipe, or bump _summary_gen for every session and land the parked slots dead.
- **Verifier:** The STOP handler at daemon.py:792-798 only calls _drop_channel_pending, wipes channels and calls speaker.cancel(). It does not bump _summary_gen, does not land the _digest_parked slots dead and does not cancel settle timers or in-flight digests. Compare _flush_all at daemon.py:1430-1459, which does all of that. STOP is reachable through the CLI 'sonara stop' command, 'stop all speech and clear the queue' (cli.py:158-159, 249-250). A digest that lands later passes apply()'s gen check and speaks after the stop.

## Findings: edges

Read-only bug hunt of the edge modules at HEAD 8d90005. Nothing was edited.

Overlap with the existing audit: AUDIT-2026-07-31.md (untracked, repo root) already lists H1, H3, H4, M2, M11 and M14. All six are still open in source (E17, plus E6 and E11, which extend H3 and M2 with new consequences). Everything else above is new.

Verified by running things:
- uv-managed Python carries EXTERNALLY-MANAGED and `pip install --user` is refused (E1).
- The PowerShell 5.1 stderr-plus-Stop abort happens (E5).
- From the deployed copy, repo_root() returns ~/.sonara (E6).
- `which -a python3` resolves to the Store stub (E3, E4).
- ~/.sonara/app differs from src in 10+ files while the version is still 0.5.0 (E15).
- The test suite: 1265 passed, 5 failed in test_win_tts.py, live WinRT (E22).

Highest-impact cluster: E1 to E4. All four are install paths where the user ends up with total silence on a fresh or zero-Python Windows box, while doctor can still show green.

Quick wins: E7 (sentinel check in ensure_daemon, one line), E12 (cue voice fallback gating), E13 (require a modifier), E14 (WorkingDirectory=~/.sonara), E5 (wrap py probe in try).

Proposed design for issue #53: KokoroEngine gets an `on_download` callback that the daemon wires to a cue or earcon. Downloads use chunked `urlopen` with a timeout. A failure stamp at ~/.sonara/kokoro/.download_failed blocks re-downloads for a backoff window (Kokoro falls back to the WinRT voice meanwhile). The prewarm skips when the model files are absent.

Cleanup notes for the broader request:
- The repo has no tracked CLAUDE.md.
- Root has untracked stray files: `.py` (0 bytes), the mis-named `C:UsersAdmin...task-1-report.md`, `.claire/worktrees`, `.claude/worktrees/feat+per-session-channels` (a stale worktree), and AUDIT-2026-07-31.md (should be filed under docs/ or deleted).
- bin/sonara-hook.cmd is dead code (only tests reference it).

Out of scope here: the user's 'queue of one message / always read the last message' request is router/daemon nav logic, not edge code.

### E1 (high, verify: confirmed) Zero-Python install path ends silent: uv-managed Python refuses pip --user (PEP 668)

- **Where:** bin/sonara-bootstrap.ps1:58-74,87-92; src/sonara/cli.py:510-519,637,701-705
- **Evidence:** When no system Python exists, the bootstrap provisions one with `uv python install 3.12` and runs `sonara install` on it. `_ensure_speech_deps` then runs `python -m pip install --user winrt-...`. uv-managed interpreters ship `Lib/EXTERNALLY-MANAGED`. I confirmed this here: C:/Users/Admin/AppData/Roaming/uv/python/cpython-3.13-windows-x86_64-none/Lib/EXTERNALLY-MANAGED exists, and `python -m pip install --user --dry-run six` returns `error: externally-managed-environment`. So PyWinRT and pycaw are never installed, install returns 1 and prints the SILENT banner. The printed manual `pip install` command fails in the same way. The bootstrap exists only for this scenario.
- **Fix:** For a uv-provisioned interpreter, install deps with `uv pip install --python <py> --break-system-packages ...` (or create a dedicated uv venv under ~/.sonara with `--seed` and run the daemon on it). Do not use `--user`. Add a test that the bootstrap path never relies on pip --user.
- **Verifier:** bin/sonara-bootstrap.ps1:62-66 provisions a uv CPython, and :92 runs install on it. cli.py:510-511 runs `python -m pip install --user`. Every uv CPython here has Lib/EXTERNALLY-MANAGED, and `cpython-3.13/python.exe -m pip install --user --dry-run six` fails with the PEP 668 error. The manual hint at cli.py:516-517 fails the same way.

### E2 (high, verify: confirmed) `sonara voices install` moves the daemon onto a venv that has no pip and no PyWinRT/pycaw

- **Where:** src/sonara/kokoro_provision.py:67-74; src/sonara/requirements-kokoro.txt:3-5; src/sonara/cli.py:384-395,625,637,814; src/sonara/platform/windows/supervisor.py:198-202
- **Evidence:** provision() runs `uv venv` (no --seed, so no pip) and installs only kokoro-onnx/onnxruntime/numpy. install() then picks the venv python (_daemon_python, cli.py:390-394) and calls _ensure_speech_deps(venv_py): `python -m pip install --user` fails (no pip module, and --user is also invalid inside a venv). Result: install returns 1, and the Task plus lazy-start both run the daemon on the venv (daemon_pythonw) without winrt or pycaw. Native voices, the Kokoro to Windows fallback, SMTC pause and ducking are all dead. This was not caught here only because this machine runs Kokoro from system Python 3.14 (install.json python = Python314) and has no ~/.sonara/venv.
- **Fix:** Add winrt-runtime, winrt-Windows.Media.SpeechSynthesis, winrt-Windows.Storage.Streams, winrt-Windows.Media.Control and pycaw to requirements-kokoro.txt (or call `uv pip install --python venv_py` for _WINRT_PACKAGES). Make _ensure_speech_deps use `uv pip` when the target is a uv venv.
- **Verifier:** kokoro_provision.py:72 runs `uv venv` with no --seed, so the venv has no pip. requirements-kokoro.txt:3-5 has no winrt or pycaw. cli.py:390-394 picks the venv python, and _ensure_speech_deps (cli.py:510) then runs pip --user, which fails. sup.install (cli.py:670) has already wired the Task to the venv before the `return 1` at cli.py:700-705.

### E3 (high, verify: confirmed) Plugin hooks rely on a `python3` shebang; the recorded-interpreter fallback lives only in the dead sonara-hook.cmd

- **Where:** hooks/hooks.json:9 (all 12 entries); bin/sonara-hook:1; bin/sonara-hook.cmd:1-24; src/sonara/platform/windows/supervisor.py:828-835
- **Evidence:** hooks.json runs `${CLAUDE_PLUGIN_ROOT}/bin/sonara-hook <Event>` under Git Bash, so `#!/usr/bin/env python3` resolves. The python.org installers up to 3.13 ship no python3.exe, so `python3` resolves to the WindowsApps Store stub (present here: `which -a python3` lists WindowsApps/python3). The stub runs nothing, so no hook ever fires. The pythonw/pyw/pythonw.path fallback exists only in bin/sonara-hook.cmd, and nothing references that file (only tests/test_bin_shims.py), so it is dead code. Because install() writes NO settings.json hooks when the plugin is enabled (supervisor.py:828-835), a plugin user with only python.org <=3.13 or zero-Python gets no hooks at all. Doctor's hooks row still shows green 'via the sonara plugin' (supervisor.py:924-925).
- **Fix:** Point hooks.json at a launcher that resolves the interpreter itself, e.g. `bash ${CLAUDE_PLUGIN_ROOT}/bin/sonara-hook-launch`, with the logic of sonara-hook.cmd: read ~/.sonara/pythonw.path first, then pythonw/py. Delete sonara-hook.cmd or wire it in. Make doctor's plugin-hooks row actually test-run the hook interpreter.
- **Verifier:** hooks.json runs `${CLAUDE_PLUGIN_ROOT}/bin/sonara-hook`, and bin/sonara-hook:1 is `#!/usr/bin/env python3`. In Git Bash, `which -a python3` lists the WindowsApps stub. bin/sonara-hook.cmd is referenced only by tests/test_bin_shims.py:29,40, so nothing runs it. When the plugin is enabled, supervisor.py:828-835 writes no settings hooks, and hooks_doctor_row (supervisor.py:924-925) reports green 'via the sonara plugin'. This box has Python314/python3 first on PATH, so it is unaffected.

### E4 (high, verify: confirmed) CLI shims prefer the Windows Store `python` stub over the recorded interpreter

- **Where:** bin/sonara:16-19; bin/sonara.cmd:11-19
- **Evidence:** bin/sonara does `py=$(command -v python || command -v python3)` and reads ~/.sonara/python.path only when nothing is found. On Windows 11 the WindowsApps `python` alias is on PATH by default (present here), so in the zero-Python case the stub is chosen and exits 9009. bin/sonara.cmd `where python && (python -m ...)` has the same problem. Every slash command except install uses bin/sonara (commands/doctor.md, start.md, settings.md, uninstall.md), so they all fail for exactly the users the bootstrap provisioned.
- **Fix:** Check ~/.sonara/python.path FIRST (it is what install validated), then a PATH python that is not under WindowsApps (reuse the Store-stub test from supervisor._is_store_stub).
- **Verifier:** bin/sonara:16-19 reads python.path only when `command -v python` finds nothing, and the WindowsApps `python` alias is on PATH here. bin/sonara.cmd:11 `where python` matches the same stub. On a zero-Python box the stub wins.

### E5 (medium, verify: confirmed) Bootstrap aborts under PowerShell 5.1 when the py launcher exists without a Python 3

- **Where:** bin/sonara-bootstrap.ps1:8,30
- **Evidence:** `$ErrorActionPreference = "Stop"` combined with `$real = & py -3 -c ... 2>$null`, outside try. In Windows PowerShell 5.1, redirected native stderr becomes an ErrorRecord, and Stop makes it terminating. I verified this: `powershell -Command "$ErrorActionPreference='Stop'; $x = & cmd /c 'echo err 1>&2' 2>$null; 'survived'"` throws NativeCommandError and never prints 'survived'. A leftover py.exe after Python was uninstalled prints 'No installed Python found' to stderr, which kills the script before the uv provisioning it exists for. /sonara:install invokes `powershell` (5.1).
- **Fix:** Wrap line 30 in try/catch like Test-RealPython does, or set `$ErrorActionPreference='Continue'` around native probes.
- **Verifier:** bootstrap.ps1:8 sets EAP=Stop, and :30 `& py -3 ... 2>$null` sits outside any try. I reproduced it in powershell 5.1: `$x = & cmd /c 'echo err 1>&2' 2>$null` throws NativeCommandError, and 'survived' never prints. commands/install.md invokes `powershell` (5.1).

### E6 (medium, verify: confirmed) Any install() failure after stop_sonara leaves the STOPPED sentinel, so Sonara stays off with no cue (H3 is still open and wider than the audit stated)

- **Where:** src/sonara/cli.py:639-679,804,814,840; src/sonara/paths.py:77-89; src/sonara/keymap.py:250-252; src/sonara/platform/windows/supervisor.py:837,570-572
- **Evidence:** stop_sonara writes the sentinel (cli.py:646), which is cleared only at 5b (cli.py:675-678). Every escape between those points leaves the daemon permanently down, because hooks return early on the sentinel (daemon.py ensure_running). The escapes are: (a) _copy_app OSError returns 1 (cli.py:652-655), which always happens from the ~/.local/bin launcher because repo_root() resolves to ~/.sonara there (verified: PYTHONPATH=~/.sonara/app gives repo_root C:\Users\Admin\.sonara); (b) keymap.write_resolved() raises ValueError on a hand-edited bad keymap.json (cli.py:661), an uncaught traceback; (c) sup.install -> merge_hooks_into_settings raises ValueError on invalid ~/.claude/settings.json, also uncaught; (d) the rename fails with PermissionError (see E14). Separately, `sonara voices install` from the launcher passes repo_root()/src = ~/.sonara/src as PYTHONPATH to predownload (cli.py:804; kokoro_provision.py:91 replaces PYTHONPATH outright). That raises, and the handler runs uninstall_kokoro, so it always fails. `voices uninstall` reaches install() with the same H3 failure.
- **Fix:** Use try/finally in install() to clear the sentinel (or restart the old daemon) on every failure path, and print 'run sonara start' on failure. Validate plugin_root/src/sonara and bin/sonara-hook before stop_sonara. Resolve the plugin root from install.json/CLAUDE_PLUGIN_ROOT instead of __file__.
- **Verifier:** stop_sonara writes the sentinel (cli.py:530-531), and only cli.py:675-678 clears it. The escapes all hold: (a) the _copy_app OSError returns 1 at cli.py:652-655, and repo_root() resolves to ~/.sonara from APP_DIR (paths.py:83-86), so src/sonara is missing. (b) write_resolved -> resolve_keymap raises ValueError on an unknown key (keymap.py:97). (c) sup.install -> merge_hooks -> _load_settings raises ValueError (supervisor.py:567-572), uncaught. voices install passes repo_root()/src (cli.py:804), and predownload replaces PYTHONPATH outright (kokoro_provision.py:91).

### E7 (medium, verify: confirmed) After `sonara shutdown`, every hook event blocks about 3 s

- **Where:** src/sonara/client.py:43-51; bin/sonara-hook:87-90; hooks/hooks.json:39-47
- **Evidence:** ensure_daemon(): when not connectable it calls ensure_running(), which returns immediately on the sentinel, and then still spins `while time.time() < deadline` for the full 3.0 s timeout. The hook calls ensure_daemon unconditionally. The PreToolUse matcher "" fires on EVERY tool call, and MessageDisplay fires per streamed message, so a user who shut Sonara down sees every Claude tool call delayed by about 3 s for the rest of the session.
- **Fix:** In ensure_daemon, return immediately when STOPPED_SENTINEL_PATH exists. Better: have the hook skip ensure_daemon and send nothing when the sentinel exists.
- **Verifier:** client.py:43-51: ensure_running returns at once on the sentinel (daemon.py:2759-2760), but the loop still spins for the full 3.0 s. bin/sonara-hook:86-89 calls ensure_daemon whenever msgs is non-empty, and hooks_entry.py:63-70 returns a TOOL msg for every PreToolUse. hooks.json sets no async or timeout.

### E8 (medium, verify: confirmed) Uninstall with the plugin still enabled is undone by the next hook

- **Where:** src/sonara/cli.py:720,735,765; src/sonara/daemon.py:2757-2765 (ensure_running)
- **Evidence:** uninstall() deletes STOPPED_SENTINEL_PATH as a 'clean slate' (cli.py:735) and only then tells the user to disable the plugin (cli.py:765). /sonara:uninstall runs inside a live session, so the very next Stop or MessageDisplay hook calls ensure_running(). No sentinel and no socket means a daemon is spawned from the plugin's src. That daemon re-creates daemon.lock, speechd.log, faulthandler.log and webui.token, re-ducks audio, and re-registers global hotkeys, right after an 'uninstall'.
- **Fix:** Keep the sentinel after uninstall (or write an 'uninstalled' marker that ensure_running honors), and clear it in install().
- **Verifier:** cli.py:735 deletes STOPPED_SENTINEL_PATH, and cli.py:765 only tells the user to disable the plugin. The next hook reaches ensure_running (daemon.py:2757-2765): no sentinel and no socket, so it Popens launch_spec from the plugin's code.

### E9 (medium, verify: confirmed) Lazy-start interpreter probes flash console windows (#125 class) and add hook latency

- **Where:** src/sonara/platform/windows/supervisor.py:146-150,171-174,184-187,230-233,198-203,758
- **Evidence:** WinSupervisorBackend.launch_spec() calls daemon_pythonw(), which calls resolve_python_windows(). That runs `py -3 -c`, `python -c` (x2 for the stub check plus sys.executable) and the version probes, up to about 8 subprocess.run/check_output calls, none with CREATE_NO_WINDOW. Callers are the hook (pythonw in settings.json installs) and webui._spawn_respawner (DETACHED pythonw, webui.py:65-74). Both are consoleless, so each console child pops a window. This happens on every lazy start after a crash or at logon before the Task runs, and adds about 0.5-1 s to the hook.
- **Fix:** Pass creationflags=0x08000000 to every probe, or better, have launch_spec read the interpreter from install.json/pythonw.path instead of re-resolving on the hook path.
- **Verifier:** launch_spec (supervisor.py:758) -> daemon_pythonw -> resolve_python_windows runs _probe_version_via_launcher, _is_store_stub, check_output and _probe_python_version (supervisor.py:146-233). None of them passes creationflags. Callers can be consoleless: settings.json hooks run under pythonw, and webui._spawn_respawner (webui.py:65-74) is a DETACHED sys.executable. Console children of those pop windows.

### E10 (medium, verify: confirmed) `voices install` runs against a live daemon and deletes a working venv on any failure

- **Where:** src/sonara/cli.py:798-813; src/sonara/kokoro_provision.py:67-74,119-123
- **Evidence:** Unlike voices uninstall (cli.py:838), voices install does not call stop_sonara. A re-run or upgrade while the daemon runs on venv\Scripts\pythonw.exe makes `uv venv` try to replace a venv whose python.exe and .pyd files are locked. That fails, and the except branch calls uninstall_kokoro(), an rmtree with no onerror. The rmtree raises PermissionError inside the handler, masking the original error and leaving a half-deleted venv. Separately, any transient failure (for example a network error during predownload) deletes a previously healthy venv.
- **Fix:** Call stop_sonara() first. Provision into venv.new and swap only on success (mirror _copy_app). Never delete the existing venv on failure.
- **Verifier:** cli.py:798-813 has no stop_sonara, unlike cli.py:838. The except path calls kp.uninstall_kokoro (kokoro_provision.py:119-123), a bare rmtree with no onerror. Any failure, including a predownload network error (kokoro_provision.py:116), deletes the previously healthy venv.

### E11 (medium, verify: confirmed) Issue #53 is still open, and the default fast-cue voice makes it reachable at every daemon start

- **Where:** src/sonara/kokoro.py:159-168,171-181,211-218; src/sonara/daemon.py:2193-2212; src/sonara/config.py:29
- **Evidence:** There is still no 'downloading' cue, no memo of failures, and urlretrieve has no timeout (audit M2). cue_voice defaults to af_heart, so _maybe_prewarm_cue_voice starts _kokoro_wav at daemon startup whenever the kokoro extra is importable. If the model files are missing, a silent 316 MB download starts under KokoroEngine._lock and every Kokoro utterance waits behind it. A network failure re-attempts the multi-minute fetch on every later Kokoro utterance.
- **Fix:** Add a callback seam: KokoroEngine(model_dir, on_download=cb), where the cb is supplied by the daemon and speaks or earcons 'Downloading neural voice'. Wrap urlopen(timeout=30) with a chunked copy. On failure, write ~/.sonara/kokoro/.download_failed with an epoch and skip downloads (falling back to WinRT) until a backoff window passes (for example 30 min). Have the prewarm skip when the model files are absent.
- **Verifier:** `gh issue view 53` shows OPEN. kokoro.py:162 calls urlretrieve with no timeout. _ensure_loaded downloads under self._lock (kokoro.py:211-218). _maybe_prewarm_cue_voice (daemon.py:2193-2212) gates only on kokoro.is_installed(), not on the model files existing, and cue_voice defaults to af_heart (config.py:29). There is no failure memo.

### E12 (medium, verify: confirmed) Default install (no Kokoro) speaks a bogus 'Kokoro unavailable, using Windows voice.' every daemon run

- **Where:** src/sonara/config.py:29; src/sonara/platform/windows/tts.py:668-685; src/sonara/daemon.py:2154-2169,2229-2243
- **Evidence:** fast_cues=True and cue_voice='af_heart' by default. _cue_voice returns af_heart regardless of whether Kokoro is installed. tts.run then takes the Kokoro branch, require_installed() raises, and the code calls kokoro._set_fallback_notice() (tts.py:681), so the first cue of each run announces a Kokoro failure to a user who never installed Kokoro. It also logs a fallback traceback on every cue.
- **Fix:** In _cue_voice, return None (native voice) when the cue voice is a Kokoro voice and neither kokoro.is_installed() nor neural_enabled() is true. Arm the notice only when Kokoro was actually provisioned.
- **Verifier:** _cue_voice (daemon.py:2154-2169) returns af_heart without checking that Kokoro is installed. tts.py:668-681 takes the Kokoro branch, require_installed raises, and _set_fallback_notice is armed. daemon.py:2240-2243 then speaks 'Kokoro unavailable, using Windows voice.' once per run, to a user who never installed Kokoro.

### E13 (medium, verify: confirmed) Settings page can bind a modifier-less global hotkey that swallows a letter system-wide

- **Where:** src/sonara/settings.html:821-844; src/sonara/keymap.py:161-180
- **Evidence:** The capture handler accepts any key with mods=[] (only Ctrl/Alt/Shift are read, and e.metaKey is ignored). bind_action validates names only. A user who clicks a row and presses 'm' registers RegisterHotKey(0|MOD_NOREPEAT, VK_M), and the letter m stops typing in every app until the binding is cleared. Win+X captures as a bare 'x'.
- **Fix:** Reject an empty mods list in bind_action (and the page), or require Ctrl or Alt. Map e.metaKey to 'win'.
- **Verifier:** settings.html:828-831 builds mods from ctrl, alt and shift only. metaKey is ignored and an empty mods list is accepted. keymap.py:161-180 validates only the names, so a bare-letter global hotkey can be persisted.

### E14 (medium, verify: partial) Task WorkingDirectory sits inside the tree that install swaps and uninstall deletes

- **Where:** src/sonara/platform/windows/supervisor.py:102,840-848; src/sonara/cli.py:565,646,469,750
- **Evidence:** work_dir = dirname(supervisor_py), which is ~/.sonara/app/sonara/platform/windows. The supervisor process pins that directory. stop_sonara relies on an async `schtasks /end`, and _kill_stray_daemons only matches 'sonara[.]daemon', never supervisor_loop.py. install ignores stop_sonara's False return (cli.py:646). If the supervisor lingers, os.rename(dst_pkg, old_pkg) (cli.py:469) raises PermissionError, which leads into E6's stuck sentinel, and uninstall's rmtree silently half-fails (cli.py:750-753 swallow OSError).
- **Fix:** Set the Task WorkingDirectory to ~/.sonara. Extend the stray sweep to supervisor_loop. Abort install before mutating if stop_sonara returns False.
- **Verifier:** The mechanism is real: work_dir is Path(supervisor_py).parent (supervisor.py:102), the cwd sits inside app/sonara, _kill_stray_daemons matches only 'sonara[.]daemon' (cli.py:565), and install ignores stop_sonara's return (cli.py:646). But stop_sonara both ends the Task and writes the sentinel that the supervisor loop honors, so the rename only fails if the supervisor survives /end. That is a race, not the normal path.

### E15 (medium, verify: confirmed) Version never bumped, so the update notice is dead and the deployed runtime is stale

- **Where:** .claude-plugin/plugin.json:4; pyproject.toml:7; src/sonara/daemon.py:538-541
- **Evidence:** Version is 0.5.0 in both files and has not changed since 2026-06-20 (git log on plugin.json), across about 25 fix and feature PRs. _setup_health flags drift only when versions differ, so 'Sonara was updated, run /sonara:install' never fires. Right now `diff -rq src/sonara ~/.sonara/app/sonara` shows 10+ differing files (client, daemon, keymap, transport, hotkeys, earcon...). This is the runtime-deploy-drift trap.
- **Fix:** Bump the patch version in every PR, per the global shipping rule. Better: stamp a content hash of src/sonara into install.json and compare it at SESSION_START instead of the version string. Keep a single version source.
- **Verifier:** pyproject.toml:7 and plugin.json:4 are both 0.5.0, and the last change to plugin.json was 2026-06-20 (git log). The drift check at daemon.py:538-541 compares only the version strings. `diff -rq src/sonara ~/.sonara/app/sonara` shows 15 differing entries (excluding pycache).

### E16 (low, verify: confirmed) Ctrl+Alt default chord is AltGr on European layouts

- **Where:** src/sonara/platform/windows/keytables.py:35-36 (DEFAULT_MODS + comment); src/sonara/keymap.py:45-49
- **Evidence:** The comment claims Ctrl+Alt 'clears AltGr' collisions, but AltGr is delivered as LCtrl+RAlt, so RegisterHotKey(MOD_CONTROL|MOD_ALT) matches it. On a German layout AltGr+M (µ) triggers mute and the character is eaten, and Polish and other layouts lose AltGr letters. Ctrl+Alt+arrows also collide with Intel display rotation.
- **Fix:** Fix the comment. Consider Ctrl+Alt+Shift or Win+Alt defaults, or detect an AltGr layout (GetKeyboardLayout) and warn in doctor.
- **Verifier:** The comment at keytables.py:35 claims Ctrl+Alt clears AltGr collisions, but AltGr is LCtrl+RAlt, so MOD_CONTROL|MOD_ALT hotkeys match it. The default 'm' (keymap.py:48) eats AltGr+M (µ) on German layouts. Low severity.

### E17 (low, verify: confirmed) Prior-audit edge items still open in source

- **Where:** src/sonara/platform/windows/supervisor.py:264-411 (no PostToolUse, H4); src/sonara/platform/windows/supervisor_loop.py:72-79 (no cwd, H1); bin/sonara-bootstrap.ps1:87-88 (-Encoding ASCII, M14); src/sonara/platform/transport.py:92,107-108 (Global\ mutex; CreateMutexW failure treated as 'owned', M11)
- **Evidence:** Each of these was re-checked against the current tree (HEAD 8d90005) and is unchanged. AUDIT-2026-07-31.md H1/H3/H4/M11/M14 are not yet fixed. Also the exec-form `args` field in the settings.json hooks has never been verified against Claude Code (docs/superpowers/M2-WINDOWS-ACCEPTANCE.md:277).
- **Fix:** Land the AUDIT wave-0 items. Generate the Windows hook set from hooks/hooks.json, and add a set-equality test.
- **Verifier:** supervisor.py:322-404 has Notification, Stop, UserPromptSubmit, SessionStart and SessionEnd but no PostToolUse or MessageDisplay, while hooks.json has PostToolUse. supervisor_loop.py:72-79 sets no cwd. bootstrap.ps1:87-88 uses -Encoding ASCII. transport.py:92 uses a Global\ mutex, and :107-108 returns None (treated as another owner) when CreateMutexW fails.

### E18 (low, verify: confirmed) CLI tracebacks on a hung daemon, and a misleading settings error

- **Where:** src/sonara/client.py:22-35; src/sonara/cli.py:69-71,899-906
- **Evidence:** Only connect() errors become DaemonNotRunning. A recv/sendall timeout (TimeoutError, an OSError subclass) propagates, and main() re-raises it, so `sonara status` against a daemon stuck under its lock prints a traceback. KeyError from a lockfile missing keys also escapes. `sonara settings` reports 'daemon is not running' when the daemon runs but the webui failed to bind (http_port None, daemon.py run()). transport.connect leaks the socket when connect fails (transport.py:42).
- **Fix:** Map socket.timeout and KeyError in client.send to a distinct DaemonUnresponsive error handled in main. Distinguish 'no settings page' from 'not running'. Close the socket on connect failure.
- **Verifier:** client.py:22-35 wraps only connect. A recv timeout raises TimeoutError, which main() re-raises (cli.py:899-906) because it is not DaemonNotRunning. transport.connect:42 never closes the socket on failure, and info['host'] raises KeyError. _cmd_settings (cli.py:69-71) prints 'not running' when http_port is None.

### E19 (low, verify: confirmed) Uninstall leaves GBs behind and crashes on a malformed settings.json

- **Where:** src/sonara/cli.py:730-763; src/sonara/platform/windows/supervisor.py:880,570-572
- **Evidence:** The artifact list omits venv/, chatterbox-venv/, kokoro/ (316 MB), chatterbox/hf-cache, previews/, tools/uv.exe, python.path/pythonw.path, duck_state.json, hotkeys.state.json, webui.token, session*.json, speechd.old*.log and daemon.singleton, yet it prints 'Removed Sonara runtime files'. remove_hooks_from_settings raises ValueError on invalid JSON after the task is already deleted, leaving a half-uninstalled state with a traceback.
- **Fix:** Offer `uninstall --purge`, or at least list what remains along with its size. Catch ValueError from the hooks removal and continue.
- **Verifier:** The artifact list at cli.py:727-735 omits the kokoro, venv, previews and tools dirs, the *.path files, the state files and more, yet cli.py:763 says 'Removed Sonara runtime files'. sup.uninstall deletes the task first and then calls remove_hooks_from_settings -> _load_settings, which raises ValueError on invalid JSON (supervisor.py:606, 567-572). Neither caller catches it.

### E20 (low, verify: confirmed) Lockfile replace races concurrent hook readers on Windows

- **Where:** src/sonara/platform/transport.py:24; src/sonara/client.py:49
- **Evidence:** os.replace fails with access denied when another process holds the target open without FILE_SHARE_DELETE, which is how Python's open() opens it. At lazy start, several hook processes poll read_lockfile every 50 ms (client.ensure_daemon) against a stale lockfile while the new daemon calls write_lockfile outside any try (daemon.py run()). That raises PermissionError, the startup dies, and a respawn follows.
- **Fix:** Retry os.replace a few times on PermissionError (with a short sleep).
- **Verifier:** transport.py:24 calls os.replace with no retry, and Python's open() on Windows does not share delete, so a concurrent reader blocks the replace. daemon.py:2720-2722 calls write_lockfile outside any try. Hook pollers read the lockfile through client.ensure_daemon (client.py:49) -> socket_connectable. The window is narrow (low severity), but the race is real.

### E21 (low, verify: confirmed) Small Windows playback and UX issues

- **Where:** src/sonara/platform/windows/tts.py:147-155,268; src/sonara/platform/windows/hotkeys.py:21-26,209
- **Evidence:** (a) _scale_wav loops over each sample in pure Python whenever volume >100, so a 30 s Kokoro clip at 24 kHz (720k samples) adds roughly 0.5-1 s of dead air before playback. (b) PlaySound lacks SND_NODEFAULT, so a missing or locked temp WAV plays the Windows default 'ding' instead of failing. (c) `sonara keymap` prints 'Ctrl+Alt+key65' for any letter or digit outside the 9-entry _VK_LABELS.
- **Fix:** Use array multiply via memoryview/numpy when available, or clamp to session volume only. Add winsound.SND_NODEFAULT. Derive labels from keytables (chr(vk) for A-Z/0-9).
- **Verifier:** (a) tts.py:148-155 loops over every sample in pure Python. (b) tts.py:268 calls PlaySound with SND_FILENAME|SND_ASYNC and no SND_NODEFAULT. (c) hotkeys.py:21-26 maps only a few letters, and display_combo (hotkeys.py:209) falls back to 'key{n}', so A prints as key65.

### E22 (low, verify: confirmed) Test suite is not hermetic on Windows

- **Where:** tests/test_win_tts.py (test_run_completes_returns_zero and 4 others)
- **Evidence:** On this box `python -m pytest -q` gives 1265 passed and 5 failed: real WinRT synthesize raises FileNotFoundError for all three installed OneCore voices (David, Zira, Mark). The probe ran inside this session's sandbox, so it may be environmental. The _winfakes shim only applies off win32, so these tests exercise live OS state rather than code.
- **Fix:** Mark real-WinRT tests @pytest.mark.windows_live (opt-in), and fake winrt by default on all platforms so CI is deterministic.
- **Verifier:** `python -m pytest -q tests/test_win_tts.py` gives 5 failed and 11 passed, with FileNotFoundError at tts.py:586. The failing tests are the ones named. The test file header says it is mock-tested through _winfakes, yet on win32 it exercises live WinRT. The root cause may be the sandbox, but the suite is non-hermetic either way.

## Findings: dead-code

Read-only audit. I edited, committed and switched nothing; vulture was installed into the scratchpad only. Method: per-symbol grep across src/, bin/ (sonara, sonara-hook, sonara.cmd, bootstrap.ps1), hooks/ and settings.html, plus vulture at 60% confidence (Chatterbox excluded). vulture's other hits are false positives: hooks_entry.handle_event and client.ensure_daemon are called from bin/sonara-hook:53,82,87; webui do_GET/do_POST/log_message and daemon_threads are framework overrides; tts.py:454-455 are WinRT option setattrs; transport.release_singleton_mutex is a reasonable test seam (test_singleton_mutex.py). Every protocol MsgType besides the 4 in DC1 has a producer (CLI, keymap, hooks_entry or webui). CLI verbs stop/skip/repeat are live via the CLI. Status of #40: section 1 is still valid except the 'error' earcon, which is now live (daemon.py:2109); jump_to_decision in queue.py is already gone; sections 2 and 3 are stale except nth_last_message (see DC4/DC6). macOS is not supported (platform/__init__.py:15). For phase zero ('always one message'), the multi-message backlog code to remove is DC1 (CATCH_UP/unheard/other_session_with_unheard) and nth_last_message; history.message_ids/entries_for_message power within-turn nav and stay live. Baseline pytest: 1279 passed, 2 skipped, 5 failed. The failures are tests/test_win_tts.py (e.g. test_terminate_sets_returncode_one, test_wait_timeout_raises), all raising FileNotFoundError at src/sonara/platform/windows/tts.py:586 when a real WinRT call runs on this box. That looks environment-dependent and is not caused by dead code, but the bug-review pass should check it.

### DC1 (high, verify: confirmed) Four orphaned protocol features: JUMP_DECISION, CATCH_UP, REREAD_OPTIONS, CYCLE_VERBOSITY (issue #40 section 1)

- **Where:** src/sonara/protocol.py:31,32,45,46; src/sonara/daemon.py:938-947 (REREAD), 949-966 (JUMP), 990-1023 (CATCH_UP), 1161-1173 (CYCLE), 79-81 (_DEBOUNCED_HOTKEYS lists CYCLE_VERBOSITY); _options store daemon.py:177,361,581,613,640,670,766; history.py:31-32,43-44,106-108,115,117-130 (_touch/_tick/unheard/other_session_with_unheard)
- **Evidence:** A grep for each wire string and MsgType constant across src/ and bin/ finds no producer. jump_decision only appears at daemon.py:949, catch_up at 990, reread_options at 938, and cycle_verbosity at daemon.py:80 and 1161. keymap.ACTION_MESSAGES (keymap.py:26-37) has only nav, flush_session, pause, mute, next_session and set_rate. cli.py has no matching verb (the parser at 205-255 and 860-887 lists status, verbosity, rate, voice, minqueue, audio-control, duck-level, audio-mode, summary, repeat, stop, skip, start, doctor, install, uninstall, daemon, keymap and voices). hooks_entry.py emits none of the four, and webui/settings.html send none. queue.jump_to_decision is already gone. The only reader of self._options is daemon.py:942 (REREAD). history.unheard is called only at daemon.py:998/1005 and other_session_with_unheard only at 1001, both inside the CATCH_UP branch. _touch/_tick exist only to feed other_session_with_unheard. README.md:168 says 'jump-to-decision, catch-up, re-read live in the CLI', which is false, and README.md:176/226/235 promise 'recoverable via catch-up'. Pinning tests: test_hotkeyd_contract.py:74,80,87,93; test_protocol.py (11 refs); test_daemon_phase2.py (19); test_daemon_phase21.py (21); test_daemon_control.py (4); test_daemon_decisions.py (4); test_daemon_flush_session.py (2); test_daemon_summary_mode.py (1). These features replay a multi-message backlog and old messages, which goes against the user's direction to 'always read the last message, never go back'.
- **Fix:** DELETE, not wire up. In one atomic commit, remove the 4 MsgType constants, the 4 daemon branches, CYCLE_VERBOSITY from _DEBOUNCED_HOTKEYS, the _options dict and all its writes, history.unheard/other_session_with_unheard/_touch/_tick, and their pinning tests. Fix the comments that mention CATCH_UP (daemon.py:952,977,1337) and the README lines 168/176/226/235. Close #40 section 1 and close #33 (two-level nav into previous messages) since it conflicts with the 'one message' direction.
- **Verifier:** Grep of src/bin/hooks: the jump_decision/catch_up/reread_options/cycle_verbosity strings appear only at protocol.py:31,32,45,46. The MsgType constants are referenced only at daemon.py:938,949,990,1161 and :80. keymap.ACTION_MESSAGES (keymap.py:26-37) has no producer for them, and settings.html/cli.py send none. The sole reader of _options is daemon.py:942, and history.unheard/other_session_with_unheard are called only at daemon.py:998,1001,1005. _touch/_tick (history.py:31-44,115,122-129) feed only other_session_with_unheard. queue.jump_to_decision is gone, but is_decision is still live (channel.py:43). The README lines at 168/176/226/235 are confirmed. Caveat: _replay_authorized and the comments at daemon.py:569,1336-1371 and router.py:29,201 are shared with nav and repeat. Edit the comments but keep that mechanism. 'high' severity is overstated, since this is dead code and not a user-facing bug.

### DC2 (medium, verify: confirmed) macOS hotkeyd leftovers: hotkeyd.resolved.json is written on every install but nothing reads it; HOTKEYD_BIN_PATH is unused

- **Where:** src/sonara/paths.py:21-22; src/sonara/keymap.py:14,247-260 (write_resolved), 79 ('Swift-facing array'), 39-40 (macOS Ctrl+Cmd comment); src/sonara/cli.py:661, 733, 736; src/sonara/daemon.py:1262, 1310-1311 (docstrings about macOS hotkeyd)
- **Evidence:** The Windows listener resolves the keymap in-process (platform/windows/hotkeys.py:114-115: keymap.resolve_keymap(keymap.load_keymap())) and never opens HOTKEYD_RESOLVED_PATH. Its only readers are write_resolved itself and the uninstall artifact list at cli.py:733. HOTKEYD_BIN_PATH has 0 references in src/ outside its definition and is only asserted in test_paths.py:46,53 and conftest.py:64. The stale file is present on this machine at ~/.sonara/hotkeyd.resolved.json. Tests that pin it: test_keymap.py:178-195, test_paths.py:45-53, conftest.py:61-64,103, test_cli_install.py:34,216 and test_cli_lifecycle.py:152 (both monkeypatch it away), test_cli_uninstall.py:13-31.
- **Fix:** DELETE write_resolved, both path constants, the cli.py:661 call and its tests. Keep a one-off removal of the legacy hotkeyd.resolved.json/hotkeyd.log in uninstall (or drop it). Separately, uninstall does not remove hotkeys.state.json (hotkeys.py:157), so add it to the cli.py:730-738 artifact list. Also simplify HotkeyBackend.install/uninstall (base.py:46-55): the Windows install() is a no-op (hotkeys.py:215-217) and cli.py:682 discards its return tuple.
- **Verifier:** write_resolved (keymap.py:247-260) is called only at cli.py:661. HOTKEYD_RESOLVED_PATH is read nowhere and appears only in the uninstall list at cli.py:733. The Windows listener resolves in-process (hotkeys.py:115). HOTKEYD_BIN_PATH (paths.py:22) has no reference in src. ~/.sonara/hotkeyd.resolved.json exists. hotkeys.state.json (hotkeys.py:157) is missing from the uninstall artifacts at cli.py:730-738. Windows install() is a no-op (hotkeys.py:215-217) and cli.py:682 discards its return value.

### DC3 (medium, verify: confirmed) Earcon set out of sync both ways: plan/ready wavs are never played, while nav/nav_edge/session_change/summary_failed are played but have no bundled wav (silent on a fresh install)

- **Where:** src/sonara/platform/windows/earcons/generate.py:108-116, earcons/plan.wav, earcons/ready.wav; src/sonara/daemon.py:822,828,831,907,979,1826,2512,2547; src/sonara/daemon.py:2911-2912; src/sonara/speaker.py:187-189
- **Evidence:** The kinds that are actually produced are choice/permission/turn_done (hooks_entry.py:51,84,99) and error (daemon.py:2109). The 'error' earcon is now LIVE, so that #40 item is stale. 'plan' was removed at hooks_entry.py:59 and 'ready' at 92-93, and nothing else calls _earcon('plan'|'ready'). The daemon fires 'nav', 'nav_edge', 'session_change' and 'summary_failed', but _EARCON_SPECS has only 6 kinds and Speaker.earcon returns silently when a kind is missing (speaker.py:187-189). The result is that the 'every nav press chimes' behaviour (daemon.py:824) and the #111 instant switch chime (daemon.py:902-907) are silent unless the user supplied their own wavs. This machine's ~/.sonara/config.json has a custom 'earcons' map covering nav/nav_edge/session_change but not summary_failed. On top of that, daemon.py:2911 applies the defaults only when 'earcons' is absent, so any user map permanently freezes the earcon set. README.md:20 advertises a plan earcon.
- **Fix:** DELETE the plan/ready specs and wavs (and update test_win_earcon.py's count==6 and test_earcon_generator). WIRE UP nav/nav_edge/session_change/summary_failed as generated specs, or remove the calls. Change daemon.py:2911 to merge bundled defaults under the user's map, e.g. {**defaults, **cfg.get('earcons', {})}.
- **Verifier:** _EARCON_SPECS (generate.py:108-116) has only permission/choice/plan/error/turn_done/ready. default_earcons derives from it (earcons/__init__.py). hooks_entry no longer emits plan (59) or ready (92-93). The daemon fires nav/nav_edge (822,828,831,979), session_change (907,2512,2547) and summary_failed (1826). Speaker.earcon returns silently when a kind is missing (speaker.py:187-189). daemon.py:2911-2912 sets defaults only when 'earcons' is absent. The user's config map has nav/nav_edge/session_change but no summary_failed. README.md:20 advertises a plan earcon. 'error' is live at daemon.py:2109.

### DC4 (medium, verify: confirmed) Unused methods, functions and aliases with no production caller

- **Where:** src/sonara/daemon.py:315 (_drop_pending), 2021 (_resume), 2252 (_audio_pause_on); src/sonara/history.py:89-104 (nth_last_message); src/sonara/paths.py:42-44 (recorded_python); src/sonara/summarizer.py:118-120 (INSTRUCTION alias); src/sonara/platform/windows/tts.py:126-127 (get_volume)
- **Evidence:** vulture (60%) plus a grep of src/ and bin/ give one occurrence each, the def itself, for _drop_pending, _resume and _audio_pause_on. None of the three is referenced in tests either: the pause path uses _audio_mode() inline, and _drop_channel_pending (daemon.py:319) replaced _drop_pending. Test-only callers: nth_last_message is called only in test_history.py:102-116; recorded_python only in test_paths.py:144-158, because the bin shims read python.path directly (bin/sonara:17, bin/sonara.cmd:12) and only recorded_pythonw is used (supervisor.py:252); INSTRUCTION only in test_summarizer.py:31-39, and the comment says 'back-compat alias'; get_volume only in test_daemon_volume.py:43.
- **Fix:** DELETE _drop_pending, _resume, _audio_pause_on, nth_last_message (and its test), recorded_python (keep _read_recorded and recorded_pythonw), and INSTRUCTION (point the test at default_instruction('natural')). get_volume is a cheap test seam, so keep it or replace it with a _VOLUME[0] read in the test.
- **Verifier:** _drop_pending (daemon.py:315), _resume (2021) and _audio_pause_on (2252) each appear only at their own def, in src and in tests. nth_last_message is called only in test_history.py:102-116. recorded_python() is called only in test_paths.py:144-158. test_bin_shims.py:25,31 only uses the name in test titles, because the shims read python.path directly. INSTRUCTION (summarizer.py:120) is used only in test_summarizer.py:31,38. get_volume is used only in test_daemon_volume.py:43.

### DC5 (low, verify: confirmed) Pre-#92 audio_control compat shim (CLI verb, protocol message, webui keys) duplicates audio_mode

- **Where:** src/sonara/protocol.py:37; src/sonara/daemon.py:1110-1115; src/sonara/cli.py:99-102,230-232; src/sonara/webui.py:23,34; src/sonara/config.py:16
- **Evidence:** daemon.py:1111 says 'Pre-#92 compat shim'. settings.html has no audio_control reference (grep finds none), so the webui _PAGE_KEYS/_MSG_KEYS entries are unreachable from the page. config.py:88 migration reads persisted['audio_control'], not DEFAULTS, so the DEFAULTS key does nothing. The audio-mode CLI (cli.py:238-240) covers everything this does. Pinned in test_cli_ducking.py (4), test_daemon_ducking.py (12), test_config.py (4), test_protocol.py (2), test_webui.py (1), test_daemon_audio_mode.py (2).
- **Fix:** DELETE the audio-control CLI verb, SET_AUDIO_CONTROL and its daemon branch, the webui keys and the DEFAULTS key. KEEP the config.py:86-89 load-time migration so old configs still map to duck.
- **Verifier:** SET_AUDIO_CONTROL is at protocol.py:37 and daemon.py:1110, the CLI verb at cli.py:99-101 and 232, and the webui keys at webui.py:23 and 34. settings.html has 0 audio_control references. The migration at config.py:88 reads persisted, not DEFAULTS, so the DEFAULTS key at config.py:16 is inert. Note that the webui would still echo audio_control in state() via _PAGE_KEYS, and nothing reads it.

### DC6 (low, verify: confirmed) Issue #40 sections 2 and 3 are stale: background_policy and should_speak are now live; only nth_last_message is still dead

- **Where:** src/sonara/sessions.py:23,106-113; src/sonara/router.py:166-174,207; src/sonara/config.py:13; src/sonara/daemon.py:2920
- **Evidence:** should_speak reads background_policy (sessions.py:111), and router._pick calls should_speak (router.py:166, 174, 207), so both are wired. router.py:166 uses getattr(self.sessions, 'should_speak', None), a duck-typing shim for test doubles: every real SessionManager has the method.
- **Fix:** Update or close #40 sections 2 and 3 (nth_last_message is handled in DC4). Optionally drop the getattr shim at router.py:166 and call self.sessions.should_speak directly.
- **Verifier:** should_speak reads background_policy (sessions.py:111) and router.py:166/174 calls it through a getattr shim. #40 sections 2 and 3 say both are unread or uncalled, and the code no longer matches: the issue body cites the stale sessions.py:28 and daemon 1030 lines.

### DC7 (low, verify: confirmed) macOS is unsupported but leftovers remain in comments and tests; macOS issues #21 and #27 are obsolete

- **Where:** src/sonara/platform/__init__.py:15 (raises 'Sonara is Windows-only'); comments at platform/windows/earcon.py:4,13; supervisor.py:6,262 ('the macOS hooks file'); supervisor_loop.py:4,21,66,124; tts.py:14; speaker.py:46,130 ('say'); keymap.py:39-40; daemon.py:1262,1310-1311; tests/test_no_os_branch_in_core.py:24-28; tests/test_cli_doctor.py:10-11,55,77-89 (say/afplay rows); tests/test_win_autostart.py:52; tests/test_win_supervisor.py:164; tests/test_bin_sonara.py:24
- **Evidence:** There is no platform/macos package in git ls-files. README.md:12 says this is the Windows line forked from the macOS sonari. #21 asks for a test of MacSupervisorBackend.launchagent_plist and #27 for macOS message parity, and neither class nor code path exists. test_core_modules_do_not_import_macos_backend guards an import that cannot exist. The non-win32 guards at transport.py:101,118, daemon.py:2817,2840, supervisor_loop.py:121 and cli.py:561, plus tests/_winfakes.py, exist only so tests can run off-Windows, and there is no CI (no .github/ directory) that does that.
- **Fix:** Close #21 and #27 as wontfix ('macOS line lives in nimkimi/sonari'). DELETE test_core_modules_do_not_import_macos_backend, swap the say/afplay fixtures for Windows rows, and reword the macOS comments. Keep the non-win32 guards and _winfakes only if a Linux CI runner is planned; otherwise they are removable (that decision belongs to the maintainer).
- **Verifier:** platform/__init__.py:15 raises 'Sonara is Windows-only'. git ls-files has no platform/macos files. test_no_os_branch_in_core.py:24-28 guards 'platform.macos' imports that cannot exist. #21 and #27 are open macOS issues, and #27 is already labelled wontfix. The macOS comments are confirmed (earcon.py:4,13; keymap.py:39-40). There is no .github/ directory. Removing the non-win32 guards is rightly left to the maintainer.

### DC8 (low, verify: confirmed) Test module named for the removed Swift hotkeyd pins dead features

- **Where:** tests/test_hotkeyd_contract.py (whole file; dead-feature tests at :74,:80,:87,:93)
- **Evidence:** The docstring (lines 1-9) says it proves 'the bytes the hotkeyd / CLI send' and admits cycle_verbosity/reread_options/jump_decision/catch_up are no longer hotkey actions. The hotkeyd no longer exists (see DC2).
- **Fix:** Rename it to test_protocol_contract.py, keep the ACTION_MESSAGES / stop / skip / repeat / faster / slower tests, and delete the 4 dead-feature tests together with DC1.
- **Verifier:** The docstring at tests/test_hotkeyd_contract.py:1-9 references 'the hotkeyd / CLI' and admits that cycle_verbosity/reread_options/jump_decision/catch_up are no longer hotkey actions. The rename and the 4-test deletion are coupled with DC1.

### DC9 (low, verify: confirmed) Hook registrations that do nothing: Notification idle_prompt, and redundant PreToolUse matchers

- **Where:** hooks/hooks.json:64-72 (idle_prompt), 15-32 (AskUserQuestion/ExitPlanMode matchers); src/sonara/platform/windows/supervisor.py:~337 (same in HOOKS_JSON_TEMPLATE)
- **Evidence:** hooks_entry.py:92-94 returns [] for idle_prompt ('No ready chime ... removed'), yet every idle still spawns a python hook process. The PreToolUse '' catch-all (hooks.json:33-41) already routes AskUserQuestion and ExitPlanMode to the same command, and hooks_entry branches on tool_name itself (hooks_entry.py:49,58). This relies on Claude Code deduplicating identical commands to avoid a double fire.
- **Fix:** DELETE the idle_prompt entry and the two tool-specific PreToolUse matchers from both hooks.json and HOOKS_JSON_TEMPLATE, then update the settings-hooks tests.
- **Verifier:** hooks.json:64-72 registers idle_prompt, and hooks_entry.py:92-94 returns [] for it, so each idle event spawns a no-op process. The '' catch-all at hooks.json:33-41 runs the same command as the AskUserQuestion/ExitPlanMode matchers at 15-32, and hooks_entry branches on tool_name (49,58). Claude Code dedupes identical commands, so there is no double fire, but the entries are redundant. They are mirrored in supervisor.py:283,296,337.

### DC10 (low, verify: confirmed) summary_settle_ms is a live setting that is missing from config DEFAULTS, so a fresh install shows 'null ms'

- **Where:** src/sonara/config.py:9-51; src/sonara/daemon.py:1584; src/sonara/webui.py:220; src/sonara/settings.html:601-602
- **Evidence:** daemon.py:1584 reads config.get('summary_settle_ms', 600), but DEFAULTS has no such key. webui.py:220 builds the page config with config.get(k), which returns None, and settings.html:602 renders s.config.summary_settle_ms + ' ms', which gives 'null ms' with an empty slider until the user sets it.
- **Fix:** Add 'summary_settle_ms': 600 to DEFAULTS and drop the inline default.
- **Verifier:** config.DEFAULTS (config.py:9-51) has no summary_settle_ms. load_config merges persisted over DEFAULTS (config.py:85), so the key is absent on a fresh install. webui.py:220 calls config.get(k), which returns None, and settings.html:602 then renders 'null ms'. daemon.py:1584 masks this with an inline default of 600.

### DC11 (low, verify: confirmed) Repo hygiene: tracked agent artifact, untracked junk, a missing referenced audit doc, no repo CLAUDE.md or CI

- **Where:** .superpowers/sdd/final-fix-report.md (tracked); repo root untracked: '.py', 'CUsersAdmin...task-1-report.md', AUDIT-2026-07-31.md, .claire/; tests/_fakeclient/sonari/ (empty pre-rename dir); DEAD_CODE_AUDIT.md (referenced by #40, absent)
- **Evidence:** git ls-files lists .superpowers/sdd/final-fix-report.md. git status shows the untracked files. ls finds tests/_fakeclient/sonari empty, and DEAD_CODE_AUDIT.md and CLAUDE.md do not exist at the repo root. There is no .github/ directory, so no ci.yml.
- **Fix:** git rm the .superpowers report and delete the untracked junk and the empty sonari dir. Edit #40 to stop pointing at DEAD_CODE_AUDIT.md. Adding a repo CLAUDE.md and ci.yml belongs to the docs/structure pass.
- **Verifier:** .superpowers/sdd/final-fix-report.md is tracked (git ls-files). tests/_fakeclient/sonari exists and is empty. DEAD_CODE_AUDIT.md, CLAUDE.md and .github/ are absent at the repo root. Issue #40 still says 'Full analysis + line references: DEAD_CODE_AUDIT.md'. The untracked junk matches the git status snapshot.

## Architecture review

Repo: `C:/Users/Admin/Documents/Claude/Github/Sonara`, branch main at `8d90005`. Nothing was edited. The two probe scripts are in the scratchpad (`probe_seed.py`, `pytest.txt`).

## 0. Findings to fix before or alongside the split

These are bugs, not just structure problems.

1. **Confirmed bug: a cue can wipe an unread short turn.** I reproduced it in `scratchpad/probe_seed.py`.
   - Sequence in summary mode: FLUSH re-seeds the channel (`ch.seeded=True`, daemon.py:739-744). A short foreground turn is delivered through `_replay(append=True)` (daemon.py:1533). That inserts with `ch.items.insert` (daemon.py:1355) and never clears `seeded`.
   - Any later `_enqueue` then calls `ch.wipe()` (daemon.py:276-279), for example a rate hotkey (daemon.py:1048), a verbosity cycle (daemon.py:1172), setup guidance (daemon.py:313) or "Nothing to navigate" (daemon.py:1920).
   - Probe output: pending was `['Short answer here.']`, and after rate-up it was `['Rate 210.']`. The last message is lost, which directly breaks the "always read the last message" rule.
   - Root cause: the daemon changes `ch.items` directly in 6 places (daemon.py:281-289, 1019-1021, 1355, 1956-1958, 1997-2016) and so skips `SessionChannel.append`'s rules for `seeded`, `gen` and `has_decision` (channel.py:298-305).
2. **Open issue #69 is still live.** FLUSH from any session clears the global pause unconditionally (daemon.py:764).
3. **Hook definitions have drifted.** The `settings.json` exec-form template (`supervisor.py` `HOOKS_JSON_TEMPLATE`, around lines 262-411) has no `PostToolUse/AskUserQuestion` entry, while `hooks/hooks.json` has it. Users who installed hooks through `settings.json` never send `CHOICE_ANSWERED` (#83). I checked this by diffing `build_hooks_json()` against `hooks.json`.
4. **Two control cues skip the control channel.** "Rate N." and "Verbosity X." go to the session channel via `_enqueue` (daemon.py:1048, 1172) instead of `_speak_cue`. They are not mute-exempt, they can wait behind minqueue, and they trigger bug 1.
5. **Order is not guaranteed within one hook event.** `bin/sonara-hook` sends each message on a new TCP connection (`client.send` per message, `bin/sonara-hook` final loop; `client.py:15-40`). The daemon gives each connection its own thread (daemon.py:2647-2668), so SET_FOREGROUND/FLUSH and EARCON/CHOICE can be applied in either order.
6. **The "queue of one" rule makes several features dead.** The user's rule is: always read only the last message, never go back.
   - These have no producer in `keymap.ACTION_MESSAGES` (keymap.py:26-37) and are already listed in #40: `JUMP_DECISION` (daemon.py:949-968), `CATCH_UP` (990-1025), `REREAD_OPTIONS` (938-947) plus the `_options` store, and `CYCLE_VERBOSITY` (1161-1173).
   - `nav_prev`/`nav_next` (daemon.py:1904-1966) contradict the rule.
   - Dead helpers: `_resume` (2021), `_drop_pending` (315), `_audio_pause_on` (2252), `history.nth_last_message` (history.py:89), `paths.HOTKEYD_BIN_PATH` (paths.py:22). `keymap.write_resolved` (keymap.py:247) is called only by install (cli.py:661), and nothing on Windows reads its output.
   - Removing all this first shrinks the split a lot.

## 1. Splitting `daemon.py` (2934 lines)

### Where the size is
- `handle_message` is one 660-line if-chain (daemon.py:545-1205).
- The summary pipeline is about 450 lines (1479-1902).
- The speak loop is about 150 lines (2425-2575).
- Chatterbox code is about 120 lines (1222-1258, 1652-1676, 2214-2227, chatterbox keys at 2275-2278). The Chatterbox-removal PR takes these out.

### Target layout
`git mv daemon.py daemon/__init__.py`, so the import path `sonara.daemon` stays the same module object.

| New module | Moves there (daemon.py lines) | State it owns | Interface |
|---|---|---|---|
| `daemon/__init__.py` (facade, about 250 lines) | `SpeechDaemon.__init__`, `handle_message` (now a table dispatch), `run`, `stop`, re-exports | wiring only | `SpeechDaemon(speaker, sessions, config, ...)`, `handle_message(msg)`, `main` |
| `daemon/core.py` | `_alloc_id`, `_muted`, `_earcon`, `_assembler`, `_enqueue`, `_minqueue`, `_teardown_session`, `note_spoken`, `_requeue_or_note` (238-426) | `_lock`, `_wake`, `_running`, `_paused`, `_mute_level`, `_current_item`, `_next_id`, `_pending_heard`, `_assemblers`, plus a **per-session registry** that features register their dicts into | `Core.forget_session(sid)` replaces the 20 hand-written pops in `_teardown_session` (353-387) |
| `daemon/decision_text.py` (pure) | `_choice_text`, `_plan_text`, `_permission_text`, `_choice_notes` (428-502) | none | pure functions |
| `daemon/ingest.py` | handlers for PROSE, CHOICE, PLAN, PERMISSION, TOOL, EARCON, FLUSH, SESSION_START/END, SET_FOREGROUND, CHOICE_ANSWERED, FORGET_SESSION (556-790, 982-988, 1075-1085), `_selection_cue` (479) | `_await_choice`, `_warned_immediate` | `register(dispatch_table)` |
| `daemon/summary/reorder.py` (pure) | `_alloc_digest_seq`, `_land_digest` (1716-1743) | `_digest_seq_next`, `_digest_seq_serve`, `_digest_parked`, `_digest_release_counter` | `DigestReorderBuffer.alloc()`, `.land(seq, fn)`, `.kill_parked()`, `.next_release_stamp()` |
| `daemon/summary/pipeline.py` | `_maybe_summarize`, settle (1569-1619), hold (1621-1650, 1692-1714), worker (1745-1902), `_decision_hold_max_s`, `_SUMMARY_MIN_CHARS` | `_summary_gen`, `_settle_*`, `_pending_decision`, `_held_decision`, `_summary_token`, `_last_dispatch_token`, `_inflight_digests`, `_voiced_upto`, `_summarize_fn` | `on_turn_done(sid)`, `on_decision(sid, item)`, `cancel(sid)` (FLUSH/teardown), `caught_up(sid)` |
| `daemon/playback.py` | `_speak_loop`, `_speak_loop_once`, `_signal_speak_failure`, preamble logic (2085-2117, 2425-2575) | `_pending_preamble`, `_poll_interval` | `SpeakLoop.run()` |
| `daemon/cues.py` | `_speak_cue`, `_cue_voice`, `_cue_voice_override`, `_voice_override`, `_maybe_prewarm_cue_voice`, Kokoro fallback notice (2119-2243) | `_kokoro_fallback_announced` | `cue(text, exempt_mute, pause_exempt, key)` |
| `daemon/controls.py` | PAUSE, MUTE, SKIP, STOP, NEXT_SESSION, FLUSH_SESSION, `_user_caught_up`, `_flush_all`, `_engaged_session`, `_reread_last` (792-911, 970-980, 1377-1477, 1968-2019) | none beyond core | handlers |
| `daemon/settings.py` | SET_RATE, SET_VOICE, SET_VERBOSITY, SET_MINQUEUE, SET_SUMMARY_MODE, SET_SESSION_PREF, STATUS, `set_config_value`, `set_summary_prompt` (1027-1183, 2261-2318) | none (config) | handlers plus `persist()` |
| `daemon/audio.py` | `_audio_mode`, `_duck_level`, `_duck_exclude_pids`, `_apply_volume`, `_maybe_engage_audio`, `_maybe_restore_audio`, `_apply_audio_mode`, SET_AUDIO_*, SET_DUCK_LEVEL, SET_VOLUME (1103-1146, 2245-2423) | ducker, pauser | `engage()`, `restore()`, `set_mode()` |
| `daemon/hotkeys.py` | `_dispatch_hotkey`, `_hotkey_worker`, `_process_hotkey`, `_debounce_suppress`, `_start/_stop/_reload_hotkeys`, `_announce_hotkey_collisions`, RELOAD_KEYMAP (2028-2083, 1260-1321, 913-924) | `_hotkey_q`, `_hotkey_last`, `_reload_lock` | `start()`, `stop()`, `reload()` |
| `daemon/server.py` | `_handle_conn*`, `_spawn_conn_handler`, `_accept_loop`, token helpers (27-61, 2577-2694), `_MAX_CONN_THREADS` | `_server`, `_token`, `_conn_sem` | `serve(on_message)`; it will host the SUBSCRIBE broadcaster (section 3) |
| `daemon/setup_health.py` | `_maybe_guide_setup`, `_setup_health`, `_launcher_present` (298-313, 515-543) | `_guided_sessions` | `maybe_guide(sid, version)` |
| `sonara/install_record.py` | `_read_install_record` (daemon.py:505 and the duplicate at cli.py:417) | none | `read()`, `write()` |
| `sonara/lifecycle.py` | `ensure_running` (2757-2765) | none | used by `client.py:8` and `webui._spawn_respawner` (webui.py:66), so clients stop importing the daemon |
| `platform/windows/process.py` | `_harden_process`, `_preload_vc_runtime`, `_arm_faulthandler` (2771-2873) | none | `harden()`, `preload_vc()` |
| `daemon/previews` → existing `previews.py` | `_start_preview_builder`, `preview_voice` (2320-2373) | `_preview_busy` | also fixes the reversed import of `webui._installed_voices` at daemon.py:2330 |

Router and channel API to add first, so the daemon stops reaching into private fields:
- Router: `authorize_replay(sid)`, `rearm_announce(sid)`, `clear_pending_announce()`, `last_active`. This replaces the private access at daemon.py:419-420, 783, 1368, 1374, 1456-1458, 1476, 1858 and 1901.
- `SessionChannel`: `insert_at(i, items)`, `truncate_pending() -> dropped`, `skip_to_end()`. All of them must keep the `seeded`, `gen` and `has_decision` rules. This is what fixes bug 1.

### Extraction order (suite green after every step)

Each step is a pure move plus test repointing, with no logic change. Run the full suite each time.

0. **Prep** (separate PRs):
   - Chatterbox removal, plus deleting the dead features from section 0.6 together with the tests that pin them (#40).
   - Add `self._persist()` and repoint the 19 test patches of `sonara.daemon.save_config` to it.
   - Add the Router and channel methods above, which fixes bug 1, and the `_paused` scoping fix for #69.
1. `git mv daemon.py daemon/__init__.py`. Nothing else changes, so every `monkeypatch.setattr(sonara.daemon, ...)` keeps working.
2. Pure, low-risk modules: `decision_text`, `platform/windows/process`, `install_record`, `lifecycle`, tokens.
3. `DigestReorderBuffer`. Tests that touch `_digest_seq_serve` and `_digest_seq_next` (2 places) need updating.
4. `setup_health`. Seven tests patch `sonara.daemon.INSTALL_RECORD_PATH` and need repointing.
5. `hotkeys`, then `audio`, then `cues`.
6. `server`. This is the prerequisite for SUBSCRIBE. Retarget the `socket_connectable` and `transport` patches (6 and 5 places).
7. `summary/pipeline`. This is the largest step: `_summarize_fn` is patched 46 times, `_settle_schedule` 9 times, `_schedule_hold_release` 3 times. Keep forwarding properties on `SpeechDaemon` until the tests move.
8. `playback`. `_current_item` is touched 13 times and `_pending_preamble` twice.
9. Turn `handle_message` into table dispatch. Each feature module registers its `MsgType`s; unknown types return None, as they do now.

### Risks
- **Patches tied to module globals.** conftest.py:112-114 and many tests patch names on `sonara.daemon`. Once code moves, those patches silently stop applying, and tests could write to real paths. Repoint the patches in the same commit as each move.
- **Lock discipline is implicit.** The rule is "caller holds `_lock`" (daemon.py:2057-2062, 1388, 1438). Add a debug-only `assert lock held` helper in `core` before moving any code.
- **Shared reorder state.** `_last_digest_text` is shared by the summary pipeline, `note_spoken` and `_reread_last` (daemon.py:180, 403-407, 1975). Put it in `core`, not in summary.
- **Ordering drift.** Moving `_wake.set()` calls can change ordering. Keep each call at its exact current position.

## 2. Other structural issues

- **No project CLAUDE.md is tracked** (`git ls-files` shows none) and **there is no CI**: `.github/` does not exist, even though the global rules require `ci.yml`. Root clutter:
  - Untracked: `.py` (0 bytes), `C:UsersAdmin...task-1-report.md`, `AUDIT-2026-07-31.md`, `.claire/`.
  - Tracked: `.superpowers/sdd/final-fix-report.md`, about 90 historical plans and specs under `docs/superpowers/`, and `tests/_fakeclient/sonari/`, which is left over from the rename.
- **Platform seam leaks:**
  - The daemon imports `platform.windows.ducking` and `pausing` directly (daemon.py:140, 144, 2906, 2908) instead of going through `get_platform()`.
  - It calls the private `tts._kokoro_wav` (daemon.py:2209).
  - It branches on `sys.platform` (daemon.py:2817, 2840). The docstring of `test_no_os_branch_in_core.py` claims the only such branch is in `platform/__init__.py`; the test only checks a short CORE list.
  - `platform/transport.py:59-84, 99-117` contains `msvcrt` and Win32 mutex code but lives outside `platform/windows/`.
  - macOS remnants: `keymap` hotkeyd output, `HOTKEYD_*` paths, daemon docstrings at 1262 and 1310, and open issues #21 and #27, which can be closed.
- **Config has no single source of truth:**
  - Clamps are spread over `handle_message` (1031-1041, 1096, 1119, 1136), `set_config_value` (2265-2279) and `webui._MSG_KEYS`/`_CONFIG_KEYS` (webui.py:29-41).
  - Defaults disagree: `duck_level` is 30 in config.py:17 but falls back to 20 at daemon.py:2257-2259.
  - The legacy `audio_control` is still in DEFAULTS (config.py:16), with a compat message at daemon.py:1110 and a webui key.
  - Proposal: `config_schema.py` with one table of key → (default, validator, live-apply hook), used by the daemon, webui and CLI.
- **Hook management is in the wrong place.**
  - The `settings.json` hook writer (about 400 lines, `supervisor.py:262-670`) manages Claude Code settings, not the Windows supervisor. It should move to `sonara/install/claude_hooks.py` and generate its entries from `hooks/hooks.json`, so section 0.3 cannot recur.
  - `cli.py` install, uninstall, copy-app and dependency code (cli.py:379-770) should move to `sonara/install/`, leaving `cli.py` as argparse only.
- **hooks.json has redundant entries.** The `AskUserQuestion` and `ExitPlanMode` matchers duplicate the `""` matcher; this only works because Claude Code de-duplicates identical commands. `idle_prompt` starts a hook process that returns `[]` (hooks_entry.py:326-328).
- **The hot-key kill switch has duplicate path logic** (daemon.py:1268, 1313). It uses `expanduser` instead of `paths.SONARA_DIR`, so conftest does not isolate it.
- **The protocol docstrings are stale.** The module says "Unix stream socket" (protocol.py:1), MUTE says "per-session" (protocol.py:26), and FLUSH_SESSION says "engaged session" (protocol.py:24). The `"v"` field is stamped on messages but never checked by the daemon.
- **The client imports the daemon** (client.py:8). Every hook process loads `sonara.daemon` (about 12 ms of a 36 ms import, measured with `-X importtime`). `sonara/lifecycle.py` fixes this.

## 3. Protocol additions for an embedded player

### SUBSCRIBE event stream (plugs into `daemon/server.py`)
- Request: `{"v":1,"type":"subscribe","events":["state"]}`. The connection stays open after it.
- Daemon pushes: `{"type":"state","seq":N,"now_playing":{"session":sid,"tab":tab|null,"kind":"summary","text":"…"}|null,"queue":int,"paused":bool,"mute_level":0|1|2,"volume":int,"summary_mode":bool}`
  - `queue` is the sum of `pending()` over all non-CONTROL channels.
  - `paused` comes from `_paused`, `mute_level` from `_mute_level`, `volume` from `config.volume`.
- Emit points: compare a snapshot after each `handle_message` (daemon.py:2622-2633, 2063-2065) and each `note_spoken` or speak start (daemon.py:2457-2458, 389-407), and send only when it changed. Changing the snapshot is done under the lock; writing to the socket happens from a per-subscriber queue, never under the lock.
- Required changes:
  - `_handle_conn` sets a 5 s timeout and drops the connection on `socket.timeout` (daemon.py:2581, 2612-2615). Subscribers need `settimeout(None)` or a heartbeat.
  - Subscribers should not take slots from the 32-connection cap (`_MAX_CONN_THREADS`, daemon.py:124); give them a separate cap of about 4.
- Extend STATUS (daemon.py:1175-1183) to return the same snapshot shape. The webui can later replace its polling (webui.py:219-237) with this stream.

### SPEAK message
- Shape: `{"v":1,"type":"speak","text":str,"source":"prism","tab":str|null,"label":str|null,"interrupt":bool=false}`
- Session id: `f"{source}:{tab or 'default'}"`. Register it through `sessions.register(sid, cwd=None)` and set its label with `session_prefs` name (so the router announcement uses it, router.py:218-221).
- Queue-of-one semantics: wipe the pending items for that session, then `_enqueue(sid, "summary", normalize_for_speech(text), False, entry=history.record(...))`. Skip assembly and summarization. Keep the global pause; this avoids the #69 behaviour.
- Handler goes in `daemon/ingest.py`. Add `MsgType.SPEAK` in protocol.py and `client.speak(text, source, tab)`.

### PRISM_TAB_ID through the hook
- `hooks_entry.handle_event(event, payload, env=os.environ)`: inject `env` so the function stays pure. It already reads `os.environ` directly at hooks_entry.py:116, which breaks the "PURE" docstring.
- Add `"tab": env.get("PRISM_TAB_ID") or None` to SESSION_START and SET_FOREGROUND.
- In the daemon, store `tab` in `SessionManager._record` (sessions.py:67) and expose it in subscribe events and `webui._sessions`.
- Use a general key name, `host_tab`, inside the protocol, so other hosts can use it later.
- Also batch: add `client.send_many(msgs)` on a single connection and use it from `bin/sonara-hook`. This fixes section 0.5.

## 4. Test suite

- `python -m pytest -q`: **95.1 s, 1279 passed, 5 failed, 2 skipped** (1286 collected, Python 3.14).
- **The 5 failures are all in `tests/test_win_tts.py`**: `test_run_completes_returns_zero`, `test_terminate_sets_returncode_one`, `test_wait_timeout_raises`, `test_run_falls_back_when_voice_name_unknown` and `test_terminate_issues_a_real_stop_playsound_call`.
  - They call real OneCore through `tts.py:586` and raise `FileNotFoundError [WinError -2147024894]`.
  - My probe showed every OneCore voice (David, Zira, Mark) fails to synthesize on this box. So the native-voice fallback (daemon.py:2242, "using Windows voice") is broken on the user's own machine. This is an environment problem, but it matters to the user.
  - These tests are live-hardware tests sitting in the unit suite. Mark them opt-in, e.g. `@pytest.mark.live_winrt`.
- **Slow tests:**
  - Playwright e2e: about 25 s (`tests/e2e/test_settings_page_e2e.py`, 7.9 s and 6.3 s for the top two).
  - Chatterbox: about 13.7 s (`test_cli_voices` 5.6 s, `test_chatterbox_worker` 5.1 s, `test_chatterbox_handle` 3.0 s). This goes away with the removal.
  - Multi-session daemon tests: about 1 s each, because they use the real-time speak loop (`test_daemon_multisession`, `test_daemon_session_change`).
  - The top 25 tests take 51 s of the 95 s total.
- **Gaps:**
  - No CI.
  - No test that `hooks/hooks.json` matches `build_hooks_json` (that is how section 0.3 slipped in).
  - No test of the seeded-channel rules on `_replay` or `_enqueue` (section 0.1).
  - No test that a background FLUSH keeps the pause (#69).
  - No ordering test for the two messages of one hook event.
  - `test_no_os_branch_in_core` leaves out `daemon.py`, `webui.py` and `cli.py`.
  - 100+ tests reach into private daemon fields (`_summarize_fn` 46 times, `_current_item` 13 times). The split should give them public seams.
  - `tests/daemon_helpers.py:68-77` writes `router._replay_authorized` directly.

## Docs and repo hygiene review

**The most important finding:** `origin` points at the upstream repo, `https://github.com/nimkimi/sonari`, and `main` tracks `sonara/main`. Plain `gh` commands therefore resolve to **nimkimi/sonari**. Running `gh issue list` without `-R` returned upstream's issues (#69, #59, #53, #40, #33, #27, #21), not this fork's. The fork's issues are at `-R Maxaubert/sonara` (triaged in section 6). A `git push origin` would push to upstream. Fix this before any PR work: run `gh repo set-default Maxaubert/Sonara` and rename the remotes (`origin` → `upstream`, `sonara` → `origin`).

## 0. Test baseline (relevant to CI and the CLAUDE.md test command)

- **Repo `.venv` (Python 3.14):** `python -m pytest -q` gives **10 failed** / 1264 passed. The failures are in `tests/test_win_tts.py` and `tests/test_winfakes.py::test_winfakes_make_winrt_and_winsound_importable`.
- **System Python 3.14 (has winrt):** 5 failed / 1279 passed. All 5 fail with `FileNotFoundError` at `src/sonara/platform/windows/tts.py:586`.
- **Cause:** the fakes in `tests/_winfakes.py` do nothing on real Windows (`tests/conftest.py:12-15`). So `test_win_tts.py` calls real OneCore and depends on what is installed on the machine. The suite is not hermetic on Windows, which contradicts the green claim from #119.
- **Recommendation:** in Phase 0, force the fakes for `test_win_tts.py`, or mark the real-OneCore tests with a `real_windows` marker that is skipped by default. Until then a `windows-latest` CI job would be red. An `ubuntu-latest` CI job with the fakes is the cheap hard gate.

## 1. Draft CLAUDE.md (lean, every line prevents a mistake)

```markdown
# Sonara – eyes-free TTS for Claude Code (Windows). Python >=3.9, src/sonara.

## Remotes (read first)
- origin = UPSTREAM nimkimi/sonari. Fork = Maxaubert/Sonara. Never push to upstream.
- Always `gh ... -R Maxaubert/sonara` (or `gh repo set-default Maxaubert/Sonara`).

## Commands
- Tests: `python -m pytest -q` (from repo root; conftest adds src/ to sys.path).
  Use an interpreter with `.[dev,windows]`. Real-OneCore tests in test_win_tts.py are machine-dependent.
- E2E (settings page): `pip install playwright && playwright install chromium`, then `python -m pytest tests/e2e -q`.
- Lint: `ruff check src tests` (once added).
- Doctor: `PYTHONPATH=src python -m sonara.cli doctor` (or `bash bin/sonara doctor`).
- Plugin-dir dev: `claude --plugin-dir <repo>`.

## Runtime deploy drift (biggest gotcha)
- The daemon runs ~/.sonara/app/sonara, NOT the repo. Before diagnosing behaviour:
  `diff -rq ~/.sonara/app/sonara src/sonara`. A redeploy needs a daemon restart.
- Safe redeploy:
  1. `PYTHONPATH=src python -m sonara.cli shutdown`; wait until no pythonw.exe remains.
  2. `PYTHONPATH=src python -c "from sonara.cli import _copy_app; _copy_app(r'<repo>')"`
  3. If only sonara.old/sonara.new remain (#127), rename sonara.new -> sonara.
  4. `PYTHONPATH=~/.sonara/app python -m sonara.cli start`
     (starting with PYTHONPATH=src runs the REPO copy).
- Hooks run via Git Bash -> bin/sonara-hook under console python.exe; bin/sonara-hook.cmd is unused by hooks.json.
- Logs: ~/.sonara/speechd.log, faulthandler.log. Live config: POST /api/set on 127.0.0.1:27431 with ~/.sonara/webui.token.

## Architecture map
- hooks/hooks.json -> bin/sonara-hook -> hooks_entry.py -> client.py --TCP (daemon.lock)--> daemon.py
- daemon.py: message handling; router.py + channel.py (per-session channels; summary mode = one item per channel)
- speaker.py (speak loop, cancel epoch); assembler.py / cleaner.py (text -> items); summarizer.py (claude -p / codex digests)
- history.py, digest_store.py, sessions.py, session_prefs.py (persisted under ~/.sonara)
- platform/ seam: base.py + windows/ (tts, hotkeys, earcon, ducking, pausing, supervisor = install/autostart/hooks)
- webui.py + settings.html (token-protected settings page); cli.py (all CLI verbs + install/doctor)
- kokoro*.py (optional neural voices, uv venv at ~/.sonara/venv)

## Conventions
- Core stays OS-free: no win32 imports outside platform/windows (test_no_os_branch_in_core.py).
- Python 3.9 syntax only (test_py39_compat.py); `from __future__ import annotations`.
- Every ~/.sonara path goes through paths.py (conftest isolates it per test).
- Slash commands are only doctor/settings/start/uninstall/install (test_commands.py pins this).
- Bump the version in pyproject.toml, .claude-plugin/plugin.json and marketplace.json together (test_manifests.py).
- No em-dashes in user-facing text (#45).
```

**Lint and typecheck tooling:** none exists. There is no ruff, mypy, flake8 or pyright in `.venv/Scripts`, and `pyproject.toml` has no tool config. Proposal:

- Add `ruff` to the `dev` extra.
- Add `[tool.ruff] target-version="py39"` and `lint.select=["E9","F","B","UP"]`, and fix what that surfaces.
- Skip mypy for now. `daemon.py` is 2934 lines and untyped, so typechecking would be mostly noise. Revisit pyright basic later.
- Add `[tool.pytest.ini_options] testpaths=["tests"]`.
- CI (`ci.yml`): ruff + pytest on `ubuntu-latest` (fakes path), Python 3.9 and 3.12.

## 2. README.md: stale or wrong statements

| Line | Problem | Fix |
|---|---|---|
| README.md:99-153 | Chatterbox section | Removed by the Chatterbox PR; also drop it from PRIVACY.md:58-64 and pyproject comments |
| README.md:168 | "jump-to-decision, catch-up, re-read live in the CLI" | No such CLI verbs exist (cli.py:50-845). Those message types are hotkey/protocol-only (protocol.py:31-32,45) |
| README.md:205-207 | CLI-only list | Omits `audio-control` (cli.py:99) and `start`; check the list against argparse |
| README.md:225-226, 235 | "catch-up can still read everything" | Catch-up has no user-reachable producer. Reword to "navigate with Ctrl+Alt+arrows" |
| README.md:249-255 | "Only the foreground session is spoken… background earcons only… flushes the queue" | Predates per-session channels and auto hand-off (router.py:186-189, daemon.py:158). Rewrite around channels and the "last message only" model the user just set |
| README.md:267-270, 280 | "socket", `speechd.sock` | The transport is a TCP lockfile, `daemon.lock` (paths.py:9, 71-74) |
| README.md:29-37 | "Python 3.9+ on PATH is required" | `/sonara:install` provisions a uv Python when none exists (commands/install.md:9-10). Pick one story |
| README.md:282-297 | Uninstall described twice, inconsistently | 291 says autostart + launcher, 295 adds `~/.sonara/app`. Merge into one paragraph |
| README.md:22, 171-178 | Hotkey table | Matches keymap.py:45-48. Keep |
| (missing) | Speech volume, session manager, Codex summary engine, audio mode | Add a short "Settings page" section instead of per-CLI prose |

## 3. CONTRIBUTING.md, PRIVACY.md, docs/

**CONTRIBUTING.md**
- :30-31 uses POSIX `.venv/bin/...` for a Windows-only tool. Use `.venv\Scripts\...`.
- :12-15 branch naming `area/short-desc` conflicts with the standing `type/issue-slug` convention.
- :53 says "(and, soon, in CI)", but there is no CI.
- :16-18 "three PRs" should allow version bump and tooling to ride inside the feature PR.
- Add the test interpreter note and the e2e note.

**PRIVACY.md (substantive inaccuracies)**
- :3 "Last updated 2026-06-05" is stale.
- :21-23 says text is "not stored". In fact each session's last digest text is persisted to `~/.sonara/session_digests.json` (paths.py:27, daemon.py:2927, #118). Session folder names and prefs are also persisted (paths.py:24-26).
- :30-37 file list is incomplete. Missing: sessions.json, session_prefs.json, session_seen.json, session_digests.json, webui.token, previews/, venv/, duck_state.json, pause_state.json, hotkeys.state.json, faulthandler.log.
- :70-77 mentions only `claude -p`. Summary mode can also use **`codex exec`** (summarizer.py:139-147), which sends message text to OpenAI under the user's Codex login. This must be disclosed.
- :85 contact email is the upstream author's. Use the fork maintainer or the issues page.

**docs/: current vs historical**
- Everything is historical: 25 plans and 33 specs under `docs/superpowers/{plans,specs}`, plus audits, acceptance checklists, `mockups/*.html`, and the Echo-era `docs/phase1-verification-checklist.md` and `docs/phase-2.1-eyes-free-prompts-spec.md`. Several are macOS-heavy (e.g. m2-windows-api-reference.md has 59 macOS/sonari hits).
- Proposed structure:
  ```
  docs/architecture.md        (new: data flow hook->daemon->router/channel->speaker; threads; persisted state; platform seam)
  docs/development.md         (test/redeploy/debug; or keep it in CLAUDE.md)
  docs/acceptance/windows.md  (one consolidated, current hardware checklist from M2/M3 + friend test)
  docs/history/{specs,plans,audits,spikes,mockups}/  (git mv of docs/superpowers/* + the two docs/phase* files)
  ```
- **Caveat:** 20 references to `docs/superpowers/...` live in src (e.g. summarizer.py:140, supervisor.py, supervisor_loop.py, settings.html, requirements-kokoro.txt). Update them in the same PR, or drop them, since code comments should not depend on spec paths.

## 4. pyproject.toml, .gitignore, .gitattributes, plugin manifests

**pyproject.toml**
- Version is stuck at `0.5.0` since 2026-07-16 (`git log -- pyproject.toml`), despite about 20 feature/fix PRs since. This has a real consequence: the installed marketplace cache `~/.claude/plugins/cache/sonara/sonara/0.5.0/commands/` still ships the old keymap/minqueue/rate/status/verbosity/voice/voices commands that 3e1daad removed. Plugin updates are keyed on version, so users never refresh. Bump to 0.6.0 in pyproject, plugin.json and marketplace.json together. There are no fork releases or tags; the only tags are upstream's `sonari--v0.3.0` to `v0.5.0`.
- `authors` lists only the upstream author. Add `maintainers=[Max Aubert]`, plus `readme="README.md"` and `[project.urls]`.
- `package-data` (pyproject.toml:39-40) omits `requirements-kokoro.txt`, which kokoro_provision.py:62-64 reads next to the module. This works today only because deploy uses `copytree` (cli.py:467); a wheel install would break.
- `_copy_app` copies `__pycache__` into the app dir (cli.py:467, no `ignore=`). Add `shutil.ignore_patterns("__pycache__")`.
- `dev` extra should be `["pytest>=7","ruff"]`. Add an `e2e = ["playwright"]` extra (tests/e2e/test_settings_page_e2e.py:9 uses importorskip).
- Remove the chatterbox requirements file and references in the Chatterbox PR.

**.gitignore:** add `.pytest_cache/`, `.superpowers/`, `.claude/worktrees/`, `.claude/settings.local.json`, `.claire/`. Then untrack `.superpowers/sdd/final-fix-report.md`, which is the only tracked file there, via `git rm --cached` or a move to docs/history.

**.gitattributes**
- `hooks/*.py text eol=lf` is stale: there are no .py files in hooks/.
- The bash/python shims `bin/sonara` and `bin/sonara-hook` are checked out as CRLF here (`core.autocrlf=true`, `git ls-files --eol`), and so is the installed plugin cache. A probe showed Git Bash tolerates the CRLF shebang and script today, but pin it anyway:
  ```
  bin/sonara text eol=lf
  bin/sonara-hook text eol=lf
  *.cmd text eol=crlf
  *.ps1 text eol=crlf
  ```

**Plugin manifests, commands, hooks**
- `marketplace.json` description contains an em-dash (`\u2014`).
- `commands/settings.md:12` says "run `sonara start`". Point to `/sonara:start` instead.
- `hooks/hooks.json` is current: its event set matches the daemon (PostToolUse AskUserQuestion is present at :43-52).
- **Stale:** the settings.json fallback template `HOOKS_JSON_TEMPLATE` (supervisor.py:264) has **no PostToolUse** (grep count 0), so `CHOICE_ANSWERED` never fires on settings.json installs. This is AUDIT H4 and still valid.
- The comment at supervisor.py:262, "hooks/hooks.json (the macOS hooks file)", is stale.
- `HOTKEYD_BIN_PATH` (paths.py:22) is a macOS Swift relic with no users.
- The macOS mentions in daemon.py:1262 and 1310 and keymap.py:39 and 79 are dead-branch commentary to trim.

## 5. Root clutter

| Item | What it is | Action |
|---|---|---|
| `.py` (0 bytes) | Accidental shell-redirect artifact | Delete |
| `C:UsersAdmin…sonari.superpowerssddtask-1-report.md` | Subagent report for the settings_port task (Jul 15, settings-page work #35) written with a mangled path | Delete; the content is in git history |
| `AUDIT-2026-07-31.md` | 33-entry audit. H3 fixed (#123/#124, paths.py:92-101). **H1 still open:** summarizer.py:165 `shutil.which(argv[0])` resolves in the daemon's cwd, and launch_spec sets no `cwd` (supervisor.py:742-760, supervisor_loop.py:51-72). H4 still open | Move to `docs/history/audits/2026-07-31.md`; fold H1, H4 and the unverified MEDIUM/LOW items into Phase 0 |
| `.claire/worktrees/feat+per-session-channels/tests/test_e2e_pipeline.py` | Stray copy from a typo'd worktree dir; that feature has landed | Delete the dir |
| `.claude/` | `settings.local.json` (machine-local permissions, already globally ignored) plus `worktrees/feat+per-session-channels`, a **live git worktree** on branch `refactor/windows-only-remove-macos` (`git worktree list`) | Keep `settings.local.json`. Use `git worktree remove`, not `rm`, once that branch is merged or abandoned. Gitignore `.claude/worktrees/` |
| `.superpowers/sdd/` (2.2 MB of diffs and reports) | SDD scratch | Gitignore it and untrack the one tracked file |
| Local branches | 70 branches, many tracking `origin/*` (upstream), some `[gone]` | Prune after the remote rename: `git fetch --prune`, then delete merged and gone branches |

## 6. Open issues on Maxaubert/sonara (`gh issue list -R Maxaubert/sonara`)

| # | Title (short) | Status / evidence | Action |
|---|---|---|---|
| 128 | Up plays nav_edge on repeat presses | Valid: daemon.py:1944 `moved = new != cur`, chimed at daemon.py:831. A fix is in progress in worktree `Sonara-wt-upkey` (`fix/128-up-always-restart`) | Phase 0; it ties directly to the "one message, always the last" model |
| 127 | `_copy_app` leaves no live package on a failed rename | Valid: cli.py:463-470 has no retry or rollback | Phase 0 |
| 118 | Cycle loses sessions after restart | Fixed in code: `SESSION_DIGESTS_PATH` (paths.py:27, daemon.py:2927) | Verify, then close |
| 117 | Second uncuttable "Session changed" | Fixed in code: `_cue_current` (speaker.py:30, 144-169) | Verify, then close |
| 116 | Speak loop blocks inside cancelled synthesis | Fixed in code: speaker.py:67 (#116 comment) | Verify, then close |
| 115 | Force-switched sessions stuck suppressed | Fixed in code: `SessionChannel.gen` (channel.py:19) | Verify, then close |
| 21 | Audit fix wave 2 | Landed (805f871, 1411df7); item 7 is Chatterbox | Close |
| 19 | Audit fix wave 1 | Landed (f2b4381, 8265374) | Close |
| 17 | Short turn overtakes queued question | Landed (5322153) | Close |
| 16 | Question lead-in skipped | Landed (cd6eacc) | Close |
| 15 | "Session X:" digest prefix | Landed (ad60cce) | Close |
| 14 | turn_done races final prose | Landed (51374c1) | Close |
| 10 | Chatterbox streaming | Chatterbox is being removed | Close as won't-do, referencing the removal PR |

**Upstream issues the plain `gh` call showed (not ours, do not act on them):** #69 (FLUSH clears pause) **does reproduce in the fork**. The FLUSH handler runs `self._paused.clear()` unconditionally (daemon.py:764), so a background session's prompt un-pauses the foreground voice. File it on the fork and fold it into Phase 0. #33 (two-level nav across past responses) runs against the user's "always just the last message" direction; do not port it.

**New issues worth filing in Phase 0:**
- AUDIT H1: daemon cwd / `which claude`.
- AUDIT H4: settings.json template missing PostToolUse.
- The non-hermetic `test_win_tts.py`.
- The version bump plus stale cache commands.
- The PRIVACY inaccuracies (Codex egress, persisted digests).
- The remote/gh default fix.
