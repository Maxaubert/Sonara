# Daemon Lifecycle: Shutdown/Start, Stop-Then-Swap Install — Design

**Issue:** Maxaubert/Sonara#23
**Status:** Approved (user-approved scope: "add a real sonara stop, make install
stop-then-swap atomically, and fix uninstall ordering")
**Date:** 2026-07-13

## Problem

There is no way to stop Sonara. The consequences (all confirmed in the stage 3
audit, `docs/superpowers/audits/2026-07-13-audit-stage3.md`):

- `sonara install` rmtree's `~/.sonara/app` while the daemon and supervisor run;
  the scheduled task's working directory sits INSIDE the tree being deleted, so
  the delete half-completes and leaves a gutted app the respawn loop cannot run
  (the documented "gutted app" runbook failure). [critical, cli.py:424]
- `uninstall` deletes the task definition but never ends the running processes;
  the system keeps respawning from a deleted install. [supervisor.py:874]
- The lazy-start path (`ensure_running`, fired on every hook event) resurrects
  the daemon no matter what, so even a manual kill does not stick.
- `voices uninstall` rmtree's a venv the running daemon executes from: raw
  PermissionError traceback, half-deleted venv. [cli.py:658]
- Doctor tells the user to run `sonara start`, which does not exist.
  [supervisor.py:815]

Constraint discovered in code: `sonara stop` ALREADY exists and means "stop
speech, clear the queue" (MsgType.STOP). The lifecycle command needs a
different name.

## Design

### Stop sentinel: `~/.sonara/stopped`

Existence = "Sonara must not run." One mechanism covers every respawn path:

- `supervisor_loop.run_supervisor_loop` checks it at the top of each iteration
  and EXITS instead of (re)spawning while it exists.
- `daemon.ensure_running` (the per-hook-event lazy start) refuses to spawn while
  it exists — a shutdown actually stays shut down.
- `sonara start` and `install()` remove it (an explicit start action).

### `MsgType.SHUTDOWN` (daemon)

New protocol message. Handler replies `{"ok": true}` and arms a short
`threading.Timer` (0.2 s) that calls `self.stop()` — the reply must reach the
socket before the daemon tears down. Existing `run()`/`stop()` cleanup already
restores ducked audio, closes the server, and unlinks the lockfile; the
singleton mutex is released by the OS at process death.

### `sonara shutdown` (CLI)

1. Write the stop sentinel.
2. `schtasks /end` the scheduled task (new `WinSupervisorBackend.end_task()`;
   best-effort — ends the task-launched supervisor).
3. Send SHUTDOWN to the daemon (tolerate "not running").
4. Wait up to ~5 s for the daemon to be gone (lockfile removed or socket no
   longer connectable), plus a short grace for process exit (mutex release).
5. Report what happened.

The shared implementation lives in `cli.stop_sonara()` so install/uninstall
reuse it.

### `sonara start` (CLI)

Remove the sentinel, then `ensure_running()` and wait briefly for the socket.
(The scheduled task still autostarts the supervisor at logon; `start` covers
the immediate case, matching what doctor tells users.)

### Install: stop, then copy-then-swap

`install()` calls `stop_sonara()` BEFORE touching `APP_DIR` and removes the
sentinel at the end (install leaves Sonara startable; the next hook event or
`sonara start` brings it up on the fresh code).

`_copy_app` becomes crash-safe:

1. `copytree(src, APP_DIR/sonara.new)` (fails -> live app untouched),
2. `rename(APP_DIR/sonara -> APP_DIR/sonara.old)`,
3. `rename(APP_DIR/sonara.new -> APP_DIR/sonara)`,
4. best-effort `rmtree(sonara.old)` (leftover cleaned next install).

Stale `.new`/`.old` from a previous crash are removed up front.

### Uninstall ordering

`uninstall()` and `_cmd_voices_uninstall` call `stop_sonara()` FIRST, then
delete. `uninstall()` also removes the sentinel at the end (nothing left to
guard; a reinstall starts clean).

## Out of scope

- Detecting/recovering a stuck mutex-holding non-serving daemon (needs a
  liveness probe redesign).
- The supervisor/CLI structural splits, log rotation, and the other stage 3
  findings.

## Test scenarios (acceptance)

1. SHUTDOWN message: daemon replies ok and stop() runs (timer armed).
2. Supervisor loop exits without spawning when the sentinel exists.
3. `ensure_running` refuses to spawn while the sentinel exists.
4. `sonara shutdown`: writes sentinel, ends task, sends SHUTDOWN, waits for
   not-connectable; tolerates a daemon that is not running.
5. `sonara start`: removes sentinel and triggers a spawn.
6. `_copy_app`: a copytree failure leaves the existing app intact; success
   replaces it and leaves no `.new`/`.old` residue.
7. `install()` stops before copying and clears the sentinel at the end.
8. `uninstall()` and `voices uninstall` stop before deleting.
