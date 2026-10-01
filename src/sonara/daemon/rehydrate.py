"""Startup channel rehydration (#118). Channels are in-memory, so a restart
emptied every queue and the manual session cycle could only reach sessions
that spoke SINCE the restart. Recently-seen sessions get their persisted last
digest re-seeded as an already-heard channel."""
from __future__ import annotations

import time

from sonara.queue import SpeechItem

# Sessions seen within this window get their persisted last digest re-seeded
# as a replayable channel, so the manual cycle reaches them across daemon
# restarts. Matches the settings page's "recent sessions" threshold.
REHYDRATE_WINDOW_S = 3 * 3600


def rehydrate_channels(daemon) -> None:
    """Re-seed each recently-seen session's channel with its persisted last
    digest as an already-heard item. Rehydrated channels are caught up
    (pending 0): never auto-spoken, but landing on them replays the digest
    like any read session, and Up re-reads it. Takes the daemon lock."""
    now = time.time()
    with daemon._lock:
        for sid, text in daemon.digest_store.items():
            if not text or sid in daemon.router.channels:
                continue
            seen = daemon.sessions.last_seen(sid)
            if seen is None or (now - seen) > REHYDRATE_WINDOW_S:
                continue
            # Heard, replay-only (no auto-speak); real content replaces it.
            daemon.router.channel(sid).seed(SpeechItem(
                id=daemon._alloc_id(), session=sid, kind="summary",
                text=text, is_decision=False))
            daemon._last_digest_text[sid] = text   # Up re-read parity
