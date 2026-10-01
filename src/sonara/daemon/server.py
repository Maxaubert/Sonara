"""The daemon's loopback socket: accept loop, per-connection handler threads
under a concurrency cap, the token handshake, and newline-delimited messages
applied in order under the daemon lock.

client.send_many writes every message of one hook event on ONE connection
(#137); they are applied sequentially on that connection's thread, in the
order sent."""
from __future__ import annotations

import socket
import sys
import threading

from sonara.protocol import encode, decode

# Cap on concurrent connection-handler threads. Legitimate clients are short-lived
# (one request each), so this bound is generous; it just stops a misbehaving or
# hostile peer from leaking unbounded threads by opening many connections.
MAX_CONN_THREADS = 32


class ConnectionServer:
    """Serves the listening socket the daemon binds. Given the daemon's
    shared state explicitly: *running* is the daemon's running event, *lock*
    the daemon lock, *handle_message* applies one message (called under
    *lock*) and returns the reply or None.

    The daemon sets *sock* (the listening socket) and *token* (the session
    token every connection must send first) before it starts accept_loop."""

    def __init__(self, running, lock, handle_message) -> None:
        self._running = running
        self._lock = lock
        self._handle_message = handle_message
        self.sock = None
        self.token = None
        self.conn_sem = threading.BoundedSemaphore(MAX_CONN_THREADS)

    def close(self) -> None:
        """Close the listening socket, which ends accept_loop."""
        srv = self.sock
        if srv is not None:
            try:
                srv.close()
            except OSError:
                pass

    def handle_conn(self, conn) -> None:
        try:
            buf = b""
            with conn:
                conn.settimeout(5.0)
                # --- token handshake: the first newline-terminated line must
                # equal the daemon's session token, or the peer is dropped. ---
                while b"\n" not in buf:
                    try:
                        data = conn.recv(4096)
                    except (OSError, socket.timeout):
                        return
                    if not data:
                        return
                    buf += data
                token_line, buf = buf.split(b"\n", 1)
                if token_line.decode("utf-8", "replace") != self.token:
                    return  # reject unauthenticated peer
                while self._running.is_set():
                    # Process any complete messages already buffered (e.g. a
                    # message that arrived in the same packet as the token).
                    while b"\n" in buf:
                        line, buf = buf.split(b"\n", 1)
                        if not line.strip():
                            continue
                        try:
                            msg = decode(line)
                        except (ValueError, UnicodeDecodeError):
                            continue
                        reply = self.handle_message_guarded(msg)
                        if reply is not None:
                            try:
                                conn.sendall(encode(reply))
                            except OSError:
                                return
                    try:
                        data = conn.recv(4096)
                    except (OSError, socket.timeout):
                        return
                    if not data:
                        return
                    buf += data
        except OSError:
            return

    def handle_message_guarded(self, msg):
        """Dispatch one socket message under the lock, contained so a malformed or
        buggy message logs a traceback instead of silently killing the connection
        thread (mirrors the hotkey worker's guard). Returns the reply or None."""
        try:
            with self._lock:
                return self._handle_message(msg)
        except Exception:  # noqa: BLE001 - one bad message must not drop the connection
            import traceback
            traceback.print_exc(file=sys.stderr)
            return None

    def handle_conn_guarded(self, conn) -> None:
        """Run handle_conn, contain any crash (log it, don't die silently), and
        always release the concurrency permit so capacity recovers."""
        try:
            self.handle_conn(conn)
        except Exception:  # noqa: BLE001 - a handler crash must be logged, not silent
            import traceback
            traceback.print_exc(file=sys.stderr)
        finally:
            self.conn_sem.release()

    def spawn_conn_handler(self, conn) -> bool:
        """Spawn a handler thread for *conn* if under the concurrency cap; else
        drop (close) the connection. Returns True iff a handler was spawned."""
        if not self.conn_sem.acquire(blocking=False):
            try:
                conn.close()
            except OSError:
                pass
            return False
        try:
            th = threading.Thread(target=self.handle_conn_guarded, args=(conn,), daemon=True)
            th.start()
        except Exception:  # noqa: BLE001 - thread creation can fail (resource limits)
            # The handler that would release the permit never ran: release it here
            # and drop the connection, else this slot leaks forever (M8).
            self.conn_sem.release()
            try:
                conn.close()
            except OSError:
                pass
            return False
        return True

    def accept_loop(self) -> None:
        import time
        srv = self.sock
        failures = 0
        while self._running.is_set():
            try:
                conn, _ = srv.accept()
            except OSError:
                if not self._running.is_set():
                    return                    # shutdown closed the socket
                # A transient accept failure (WSAECONNRESET burst etc.) used to
                # kill the WHOLE daemon, which the hooks then silently respawned
                # with fresh state - one of the mute-reset triggers (#65). Retry;
                # a genuinely dead socket exhausts the cap and exits as before.
                failures += 1
                if failures > 20:
                    print("[daemon] accept failing persistently; exiting",
                          file=sys.stderr, flush=True)
                    return
                print("[daemon] transient accept error; retrying",
                      file=sys.stderr, flush=True)
                time.sleep(0.2)
                continue
            failures = 0
            self.spawn_conn_handler(conn)
