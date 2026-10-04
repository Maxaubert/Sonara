"""Helpers for the plugin suite (#202): the bash launcher and wrapper in
``bin/``, the PowerShell bootstrap, the built ``sonara.exe`` and the release
zip, driven as Claude Code drives them (Git Bash, payload on stdin), with
temporary ``LOCALAPPDATA``, ``SONARA_HOME`` and ``USERPROFILE`` folders and
a local HTTP server standing in for GitHub releases. Nothing touches this
PC's Sonara, Claude Code settings or audio: every runtime started here runs
``--engine fake --system fake`` on a temporary home."""
from __future__ import annotations

import hashlib
import http.server
import importlib.util
import json
import os
import shutil
import socket
import subprocess
import threading
import time
import zipfile
from pathlib import Path

import harness

REPO = harness.REPO
BIN = REPO / "bin"
VERSION = (BIN / "runtime-version").read_text(encoding="utf-8").strip()
ZIP_NAME = f"sonara-runtime-win-x64-{VERSION}.zip"
EXES = ("sonarad.exe", "sonara-hook.exe", "sonara.exe")
FAKE = "--engine fake --system fake --keys fake"


def find_bash() -> Path | None:
    for root in (os.environ.get("ProgramFiles", r"C:\Program Files"),
                 os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)")):
        cand = Path(root) / "Git" / "bin" / "bash.exe"
        if cand.is_file():
            return cand
    found = shutil.which("bash")
    return Path(found) if found else None


def build_dir() -> Path | None:
    """The target folder holding all three executables (the newest)."""
    exe = harness.find_sonarad()
    if exe is None:
        return None
    d = exe.parent
    return d if all((d / e).is_file() for e in EXES) else None


def posix(p) -> str:
    return str(p).replace("\\", "/")


def release_zip_module():
    spec = importlib.util.spec_from_file_location(
        "release_zip", REPO / "packaging" / "release_zip.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def build_release(exes: Path, out: Path) -> tuple[bytes, bytes]:
    """A release built by packaging/release_zip.py from the built
    executables: (zip bytes, SHA256SUMS bytes). The fake engine never loads
    ONNX Runtime, so placeholders stand in when it is not staged."""
    stage = out / "stage"
    stage.mkdir(parents=True)
    for e in EXES:
        shutil.copy2(exes / e, stage / e)
    for f in ("onnxruntime.dll", "onnxruntime-LICENSE.txt"):
        src = exes / f
        if src.is_file():
            shutil.copy2(src, stage / f)
        else:
            (stage / f).write_bytes(b"placeholder: the fake engine never loads it")
    rz = release_zip_module()
    path = rz.build_zip(stage / "sonarad.exe", out / "dist", VERSION)
    sums = rz.write_sums([path], out / "dist")
    return path.read_bytes(), sums.read_bytes()


def fake_zip(exes: Path) -> bytes:
    """A small release zip: the three executables under the release folder."""
    import io
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", compression=zipfile.ZIP_DEFLATED) as z:
        for e in EXES:
            z.write(exes / e, f"sonara-runtime-win-x64-{VERSION}/{e}")
    return buf.getvalue()


def sums_for(data: bytes, name: str = ZIP_NAME) -> bytes:
    return f"{hashlib.sha256(data).hexdigest()}  {name}\n".encode()


class Releases:
    """A local stand-in for github.com/<repo>/releases/download: serves
    ``/v<version>/<file>`` from ``files`` and counts the requests."""

    def __init__(self, files: dict[str, bytes], delay: float = 0.0):
        self.files = files
        self.hits: dict[str, int] = {}
        self.delay = delay
        outer = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):  # noqa: N802
                name = self.path.rsplit("/", 1)[-1]
                outer.hits[name] = outer.hits.get(name, 0) + 1
                if outer.delay:
                    time.sleep(outer.delay)
                body = outer.files.get(name) if self.path.startswith(f"/v{VERSION}/") else None
                if body is None:
                    self.send_response(404)
                    self.send_header("Content-Length", "0")
                    self.end_headers()
                    return
                self.send_response(200)
                self.send_header("Content-Type", "application/octet-stream")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *args):
                pass

        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}"
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def close(self):
        self.server.shutdown()
        self.server.server_close()


def closed_port_url() -> str:
    """A loopback URL nothing listens on (offline)."""
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return f"http://127.0.0.1:{port}"


class Box:
    """Temporary LOCALAPPDATA, home and profile, and the environment a hook
    gets from Claude Code."""

    def __init__(self, tmp: Path, base_url: str | None = None):
        self.lad = tmp / "lad"
        self.home = tmp / "home"
        self.profile = tmp / "profile"
        for d in (self.lad, self.profile):
            d.mkdir(parents=True, exist_ok=True)
        self.root = self.lad / "Sonara" / "runtime"
        self.dest = self.root / VERSION
        env = dict(os.environ)
        for k in ("SONARA_SUMMARIZER", "SONARA_CAPTURE", "SONARA_HOST_TAB", "PRISM_TAB_ID",
                  "SONARA_NO_START", "SONARA_RUNTIME_ARGS", "SONARA_BOOTSTRAP_START",
                  "SONARA_RELEASE_BASE_URL", "CLAUDE_PLUGIN_ROOT"):
            env.pop(k, None)
        bash = find_bash()
        if bash is not None:
            tools = bash.parent.parent / "usr" / "bin"
            env["PATH"] = os.pathsep.join([str(tools), env.get("PATH", "")])
        env.update({
            "LOCALAPPDATA": str(self.lad),
            "SONARA_HOME": str(self.home),
            "USERPROFILE": str(self.profile),
            "SONARA_RUNTIME_ARGS": FAKE,
            "SONARA_NO_BROWSER": "1",
            "CLAUDE_PLUGIN_ROOT": str(REPO),
        })
        if base_url:
            env["SONARA_RELEASE_BASE_URL"] = base_url
        self.env = env

    def launch(self, event: str, payload: dict | None = None, extra: dict | None = None,
               timeout: float = 30.0) -> tuple[int, float]:
        """Run the hook launcher as Claude Code does: (exit code, seconds).
        Output is captured through pipes, so a background process holding
        them would show up as a slow hook."""
        env = dict(self.env)
        env.update(extra or {})
        data = json.dumps(payload or {}).encode()
        t0 = time.monotonic()
        p = subprocess.run([str(find_bash()), posix(BIN / "sonara-hook-launch"), event],
                           input=data, capture_output=True, env=env, timeout=timeout,
                           creationflags=harness.CREATE_NO_WINDOW)
        return p.returncode, time.monotonic() - t0

    def wrapper(self, *args: str, timeout: float = 180.0) -> subprocess.CompletedProcess:
        """``bash bin/sonara <args>``, as the slash commands run it."""
        return subprocess.run([str(find_bash()), posix(BIN / "sonara"), *args],
                              capture_output=True, text=True, env=self.env, timeout=timeout,
                              creationflags=harness.CREATE_NO_WINDOW)

    def cli(self, exe: Path, *args: str, timeout: float = 60.0) -> subprocess.CompletedProcess:
        return subprocess.run([str(exe), *args], capture_output=True, text=True,
                              env=self.env, timeout=timeout,
                              creationflags=harness.CREATE_NO_WINDOW)

    def install(self, exes: Path, version: str = VERSION) -> Path:
        """The runtime as the bootstrap leaves it."""
        dest = self.root / version
        dest.mkdir(parents=True, exist_ok=True)
        for e in EXES:
            shutil.copy2(exes / e, dest / e)
        return dest

    def wait_bootstrap(self, timeout: float = 120.0) -> None:
        """Until no bootstrap holds the lock (it removes it when done)."""
        lock = self.root / ".bootstrap.lock"
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            if self.root.is_dir() and not lock.exists():
                return
            time.sleep(0.2)
        raise AssertionError(f"the bootstrap still runs after {timeout} s; log: {self.log()}")

    def wait_for(self, cond, timeout: float = 120.0, what: str = "condition") -> None:
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            if cond():
                return
            time.sleep(0.2)
        raise AssertionError(f"timed out waiting for {what}; bootstrap log: {self.log()}")

    def log(self) -> str:
        p = self.home / "logs" / "bootstrap.log"
        return p.read_text(encoding="utf-8", errors="replace") if p.is_file() else "(none)"

    def runtime_info(self) -> dict | None:
        try:
            return json.loads((self.home / "runtime.json").read_text(encoding="utf-8"))
        except (OSError, ValueError):
            return None

    def stop_runtime(self, exes: Path | None = None) -> None:
        """End a runtime started on this home (teardown)."""
        exe = (self.dest / "sonara.exe") if (self.dest / "sonara.exe").is_file() else (
            exes / "sonara.exe" if exes else None)
        if exe is not None and exe.is_file():
            try:
                self.cli(exe, "stop", timeout=30)
            except subprocess.TimeoutExpired:
                pass
        info = self.runtime_info()
        if info and info.get("pid"):
            subprocess.run(["taskkill", "/F", "/PID", str(info["pid"])],
                           capture_output=True, creationflags=harness.CREATE_NO_WINDOW)
