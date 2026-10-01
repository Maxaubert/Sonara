"""The daemon's auth token: clients send it with every message and the
settings page carries it in its URL. It is reused across restarts (#34) so the
page and bookmarks keep working."""
from __future__ import annotations

import os
import secrets

from sonara.platform import transport


def wellformed_token(tok) -> bool:
    return (isinstance(tok, str) and len(tok) == 64
            and all(c in "0123456789abcdef" for c in tok))


def select_token(prior_lock: dict) -> str:
    """Reuse a well-formed prior lockfile token (settings-page restart
    reconnect + durable bookmarks, #34); otherwise mint a fresh one."""
    tok = (prior_lock or {}).get("token")
    if wellformed_token(tok):
        return tok
    return secrets.token_hex(32)


def persistent_token() -> str:
    """The daemon token, durable across CLEAN restarts (#34 follow-up): the
    lockfile is unlinked on exit, so lockfile-based reuse only covered crashes
    -- live-verified when the page's Restart button reconnected to a 403 wall.
    Priority: token file, then a stale lockfile (crash case), else mint. The
    chosen token is (re)written to the file so the NEXT start reuses it."""
    from sonara import paths as _paths
    tok = None
    try:
        tok = _paths.WEBUI_TOKEN_PATH.read_text(encoding="utf-8").strip()
    except OSError:
        pass
    if not wellformed_token(tok):
        tok = select_token(transport.read_lockfile(_paths.LOCK_PATH) or {})
    try:
        _paths.ensure_sonara_dir()
        _paths.WEBUI_TOKEN_PATH.write_text(tok, encoding="utf-8")
        os.chmod(_paths.WEBUI_TOKEN_PATH, 0o600)
    except OSError:
        pass                     # unwritable dir: token still valid this run
    return tok
