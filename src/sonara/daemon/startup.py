"""The daemon process entry point (#141): the single-instance guard (named
mutex, then the lock-file byte-lock), process hardening, crash recovery of
ducked or paused audio, and wiring the platform backends into a
SpeechDaemon. `python -m sonara.daemon` and `sonara` CLI verbs call main()
through the sonara.daemon package, which re-exports it."""
from __future__ import annotations

import sys

from sonara import config_schema
from sonara.config import load_config
from sonara.paths import (
    SESSION_DIGESTS_PATH, SESSION_PREFS_PATH, SESSION_SEEN_PATH, SESSIONS_PATH,
    SINGLETON_PATH, ensure_sonara_dir, socket_connectable,
)

# Holds the single-instance flock for this process's lifetime (see main()).
_SINGLETON = None
_MUTEX = None       # process-lifetime handle to the named single-instance mutex


def resolve_earcons(bundled: dict, overrides) -> dict:
    """The earcon map the speaker plays: the bundled set, resolved from the
    running package on every start, with the user's own wavs (config
    "earcons") on top. Never stored back into config, so new bundled kinds
    reach every install (#136, audit M8)."""
    out = dict(bundled)
    if isinstance(overrides, dict):
        out.update({k: v for k, v in overrides.items()
                    if isinstance(v, str) and v})
    return out


def main() -> None:
    from sonara.platform import daemon_process
    _process = daemon_process()
    _process.arm_faulthandler()
    # Single-instance guard. The fast path avoids work when a daemon is clearly
    # already serving. The AUTHORITATIVE guard is the exclusive flock below:
    # with an ephemeral TCP port, bind() never collides (unlike the old fixed
    # AF_UNIX path), so socket_connectable() alone is racy and lets concurrent
    # lazy-starts each bind their own port -> a daemon explosion. The flock lets
    # exactly one process win; the rest exit. The lock auto-releases on death.
    global _SINGLETON, _MUTEX
    if socket_connectable():
        return
    ensure_sonara_dir()
    # AUTHORITATIVE single-instance guard: a named kernel mutex. The byte-lock
    # below is tied to the lock FILE's inode, so a deleted/recreated file or two
    # daemons racing to create it stop excluding -> a daemon explosion (observed
    # live). The mutex is keyed by name, immune to that, and frees on death.
    try:
        _MUTEX = _process.acquire_singleton_mutex()
    except OSError as exc:
        # M11: a mutex that cannot be created is not "another daemon owns
        # it". Log it and let the lock-file byte-lock below decide.
        print("[singleton] {0}; using the lock file instead".format(exc),
              file=sys.stderr, flush=True)
        _MUTEX = False
    if _MUTEX is None:
        print("[singleton] another Sonara daemon is already running for this "
              "user; exiting", file=sys.stderr, flush=True)
        return
    _SINGLETON = _process.acquire_singleton(SINGLETON_PATH)  # pid record (best-effort)
    if _MUTEX is False and _SINGLETON is None:
        print("[singleton] the lock file is held by another daemon; exiting",
              file=sys.stderr, flush=True)
        return

    _process.harden_process()   # win32: opt out of EcoQoS throttling + raise
                                # priority so global hotkeys stay responsive
                                # after long idle
    _process.preload_vc_runtime()   # win32: system VC runtime first, before any engine (#29)

    from sonara.speaker import Speaker
    from sonara.sessions import SessionManager
    from sonara.platform import get_platform

    _backend = get_platform()
    # Un-duck and resume anything a crashed prior daemon left down or paused.
    _backend.recover_audio()
    cfg = load_config()
    speaker = Speaker(
        voice=cfg.get("voice"),
        rate=config_schema.get(cfg, "rate"),
        say_runner=_backend.tts.run,
        earcon_player=_backend.earcon.play,
        earcons=resolve_earcons(_backend.earcon.default_earcons(),
                                cfg.get("earcons")),
    )
    sessions = SessionManager(background_policy=config_schema.get(cfg, "background_policy"),
                              store_path=SESSIONS_PATH, seen_path=SESSION_SEEN_PATH)
    from sonara.session_prefs import SessionPrefs
    from sonara.digest_store import DigestStore
    from sonara.daemon import SpeechDaemon
    daemon = SpeechDaemon(speaker, sessions, cfg,
                          ducker=_backend.ducker, pauser=_backend.pauser,
                          prefs=SessionPrefs(store_path=SESSION_PREFS_PATH),
                          digests=DigestStore(store_path=SESSION_DIGESTS_PATH))
    daemon._audio.apply_volume(config_schema.get(cfg, "volume"))   # restore persisted speech gain
    daemon.run()
