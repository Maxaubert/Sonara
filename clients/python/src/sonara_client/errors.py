"""Errors of the Sonara client."""
from __future__ import annotations

from typing import Optional

# Client-side codes, next to the protocol's E_* codes that come from the runtime.
E_NOT_RUNNING = "E_NOT_RUNNING"  # nothing running, nothing to start
E_START_FAILED = "E_START_FAILED"  # the bundled runtime did not come up in time
E_CLOSED = "E_CLOSED"  # the connection closed before the reply


class SonaraError(Exception):
    """A coded failure: a protocol error (``E_BUSY``, ``E_NOT_FOUND``...) or
    one of the client's own (``E_NOT_RUNNING``, ``E_START_FAILED``,
    ``E_CLOSED``)."""

    def __init__(self, code: str, message: str, reason: Optional[str] = None):
        super().__init__(f"{code}: {message}")
        self.code = code
        self.message = message
        #: Protocol 1.2: why an external engine failed (``auth``, ``quota``,
        #: ``network``...), on an ``E_ENGINE`` of ``engine_test``.
        self.reason = reason
