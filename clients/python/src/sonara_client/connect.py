"""``connect()``: find or start the shared runtime (spec section 3)."""
from __future__ import annotations

import os
import time
from typing import Optional, Sequence

from .client import Client
from .connection import Connection
from .discovery import live_runtime, resolve_home, start_runtime, wait_exit
from .errors import E_CLOSED, E_NOT_RUNNING, SonaraError
from .version import PROTOCOL, __version__

CONNECT_TIMEOUT = 5.0
EXIT_WAIT = 5.0


class _Incompatible(Exception):
    """The instance answers but cannot serve this client."""


def connect(
    client_name: str,
    *,
    runtime_path: Optional[str] = None,
    home: Optional[str] = None,
    autostart: bool = True,
    require: Sequence[str] = (),
    extensions: Sequence[str] = (),
    client_version: Optional[str] = None,
    keep_alive: bool = False,
    runtime_args: Sequence[str] = (),
    start_timeout: float = 5.0,
    takeover_timeout: float = 30.0,
    takeover_retry: float = 0.25,
    timeout: float = 10.0,
) -> Client:
    """Connect to the shared Sonara runtime.

    1. Read ``runtime.json`` in the home (``home``, else ``SONARA_HOME``,
       else ``%LOCALAPPDATA%\\Sonara``); when its pid is alive, connect and
       send ``hello``.
    2. Use it when it speaks protocol 1 and offers everything in ``require``.
    3. Otherwise (with ``autostart``), start ``runtime_path --home <home>``
       (default: ``SONARA_RUNTIME``), wait up to ``start_timeout`` seconds
       for its ``runtime.json``, then ``hello``.
    4. An incompatible running instance is first asked to step down
       (``hello`` with ``takeover: true``); while it is busy the takeover is
       retried for up to ``takeover_timeout`` seconds, then
       ``E_INCOMPATIBLE``.

    ``runtime_args`` are extra arguments for a started runtime (tests:
    ``["--engine", "fake"]``); ``timeout`` bounds each request.
    """
    if not isinstance(client_name, str) or not client_name:
        raise SonaraError("E_BAD_REQUEST", "connect() needs a client_name")
    home = resolve_home(home)
    runtime_path = runtime_path or os.environ.get("SONARA_RUNTIME")
    can_start = bool(autostart and runtime_path)
    hello = {
        "client": {"name": client_name, "version": client_version or __version__},
        "protocol": dict(PROTOCOL),
        "require": list(require),
        "extensions": list(extensions),
    }
    if keep_alive:
        hello["keep_alive"] = True

    running = live_runtime(home)
    if running:
        try:
            client = _greet(running, hello, list(require), timeout)
            if client:
                return client
        except _Incompatible as e:
            if not can_start:
                raise SonaraError(
                    "E_INCOMPATIBLE",
                    f"the running Sonara {running.get('version')} cannot serve this client ({e}) "
                    "and there is no runtime to start in its place",
                ) from None
            _take_over(home, client_name, client_version, str(e), takeover_timeout, takeover_retry)

    if not can_start:
        why = "autostart is off" if runtime_path else "no runtime_path to start one"
        raise SonaraError(E_NOT_RUNNING, f"no Sonara runtime is running and {why}")
    started = start_runtime(runtime_path, home, list(runtime_args), start_timeout)
    try:
        client = _greet(started, hello, list(require), timeout)
    except _Incompatible as e:
        raise SonaraError(
            "E_INCOMPATIBLE", f"the bundled Sonara {started.get('version')} cannot serve this client: {e}"
        ) from None
    if client is None:
        raise SonaraError(E_CLOSED, "the started runtime closed the connection")
    return client


def _greet(info: dict, hello: dict, require: list, timeout: float) -> Optional[Client]:
    """Connect and ``hello``. None when the instance cannot be reached;
    _Incompatible when it answers but cannot serve this client."""

    def dial() -> Connection:
        conn = Connection(info["port"], timeout)
        try:
            conn.request("hello", {**hello, "token": info["token"]})
        except SonaraError:
            conn.close()
            raise
        return conn

    try:
        conn = Connection(info["port"], min(timeout, CONNECT_TIMEOUT))
    except SonaraError:
        return None
    try:
        reply = conn.request("hello", {**hello, "token": info["token"]})
    except SonaraError as e:
        conn.close()
        if e.code in ("E_INCOMPATIBLE", "E_UNSUPPORTED"):
            raise _Incompatible(e.message) from None
        if e.code == E_CLOSED:
            return None
        raise
    offered = set(reply.get("capabilities") or []) | set(reply.get("extensions") or [])
    missing = [r for r in require if r not in offered]
    major = (reply.get("protocol") or {}).get("major")
    if major != PROTOCOL["major"] or missing:
        conn.close()
        raise _Incompatible(f"it does not offer {', '.join(missing)}" if missing else f"it speaks protocol {major}")
    return Client(conn, info, reply, dial)


def _take_over(home: str, name: str, version: Optional[str], why: str, limit: float, retry: float) -> None:
    """Ask the running instance to exit so the bundled runtime can start. It
    accepts only when idle; while busy, retry until ``limit`` seconds. Each
    attempt uses a fresh connection: the runtime closes one that has no
    successful ``hello`` within 5 s."""
    deadline = time.monotonic() + limit
    client = {"name": name, "version": version or __version__}
    while True:
        cur = live_runtime(home)
        if not cur:
            return
        try:
            conn = Connection(cur["port"], CONNECT_TIMEOUT)
            try:
                conn.request("hello", {"token": cur["token"], "client": client, "takeover": True})
            finally:
                conn.close()
            if wait_exit(cur["pid"], EXIT_WAIT):
                return
        except SonaraError as e:
            if e.code not in ("E_BUSY", E_CLOSED):
                raise SonaraError(
                    "E_INCOMPATIBLE",
                    f"the running Sonara cannot serve this client ({why}) and refused a takeover: {e.message}",
                ) from None
        if time.monotonic() >= deadline:
            raise SonaraError(
                "E_INCOMPATIBLE",
                f"the running Sonara cannot serve this client ({why}) and stayed busy; gave up the takeover",
            )
        time.sleep(retry)
