# Daemon Lifecycle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development or superpowers:executing-plans. Design/spec: `docs/superpowers/specs/2026-07-13-daemon-lifecycle-design.md` (all behavior questions resolve there).

**Goal:** A real shutdown/start lifecycle; install and uninstall never mutate files under a running daemon.

**Architecture:** A stop sentinel (`~/.sonara/stopped`) gates every respawn path (supervisor loop, lazy start). A new SHUTDOWN protocol message exits the daemon cleanly. `cli.stop_sonara()` composes sentinel + task-end + SHUTDOWN + wait, and install/uninstall call it before touching files. `_copy_app` becomes copy-then-swap.

**Tech Stack:** Python 3.14 stdlib, pytest.

## Global Constraints

- `sonara stop` keeps its current meaning (stop speech). The lifecycle commands are `sonara shutdown` and `sonara start`.
- The SHUTDOWN reply must be sent before teardown (timer-deferred stop).
- Every step best-effort: a missing daemon/task never fails shutdown.
- No em-dashes.

---

### Task 1: Sentinel + SHUTDOWN + respawn-path gating

**Files:** Modify `src/sonara/paths.py` (STOPPED_SENTINEL_PATH), `src/sonara/protocol.py` (MsgType.SHUTDOWN), `src/sonara/daemon.py` (handler + ensure_running gate), `src/sonara/platform/windows/supervisor_loop.py` (loop gate). Tests: `tests/test_daemon_lifecycle.py` (new).

**Interfaces:** Produces `paths.STOPPED_SENTINEL_PATH`, `MsgType.SHUTDOWN = "shutdown"`, daemon handler (replies `{"ok": True}`, arms `threading.Timer(0.2, self.stop)`), `supervisor_loop._stop_requested() -> bool`, sentinel check in `ensure_running`.

Steps: failing tests (scenarios 1-3 of the spec) -> implement -> green -> commit.

### Task 2: `sonara shutdown` / `sonara start` + `end_task()`

**Files:** Modify `src/sonara/cli.py` (stop_sonara(), start_sonara(), subcommands), `src/sonara/platform/windows/supervisor.py` (`end_task()`). Tests: `tests/test_cli_lifecycle.py` (new).

**Interfaces:** `cli.stop_sonara(sup) -> bool` (True if the daemon is gone), `cli.start_sonara() -> int`; `WinSupervisorBackend.end_task()` wraps `self._schtasks(["/end", "/tn", TASK_NAME])`.

Steps: failing tests (scenarios 4-5) -> implement -> green -> commit.

### Task 3: Install stop-then-swap + uninstall ordering

**Files:** Modify `src/sonara/cli.py` (`install()` stops first + clears sentinel at end; `_copy_app` swap; `uninstall()` and `_cmd_voices_uninstall` stop first). Tests: extend `tests/test_cli_lifecycle.py` + `tests/test_cli_install.py`/`test_cli_voices.py` order assertions.

Steps: failing tests (scenarios 6-8) -> implement -> green -> full suite -> commit.
