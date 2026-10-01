"""Guard: the portable core must never branch on the OS or reach a concrete
backend. The ONLY sys.platform branch in Sonara lives in platform/__init__.py
(_require_windows, behind get_platform() and daemon_process()); everything
OS-specific lives under platform/windows and is reached through that seam.

keymap.py is intentionally NOT covered: it re-exports keytables from the
active backend, so it is a documented platform-coupled module.
"""
import pathlib
import re

SRC = pathlib.Path(__file__).resolve().parents[1] / "src" / "sonara"

CORE = [
    "assembler.py", "cleaner.py", "queue.py", "history.py", "sessions.py",
    "protocol.py", "hooks_entry.py", "speaker.py", "config.py",
    "webui.py", "cli.py",
]
# Whole packages held to the same rule (#142): the daemon, and the installer
# cli.py hands its install, uninstall and doctor work to.
CORE_PACKAGES = ["daemon", "install"]

# An OS branch or a direct reach into a concrete backend or an OS-only module.
FORBIDDEN = [
    (re.compile(r"\bsys\.platform\b"), "branches on sys.platform"),
    (re.compile(r"\bos\.name\b"), "branches on os.name"),
    (re.compile(r"\bplatform\.windows\b"), "imports the Windows backend directly"),
    (re.compile(r"^\s*(import|from)\s+(ctypes|msvcrt|winreg|winsound)\b", re.M),
     "imports an OS-only module"),
]


def _core_files():
    files = [SRC / name for name in CORE]
    for pkg in CORE_PACKAGES:
        files.extend(sorted((SRC / pkg).rglob("*.py")))
    return files


def _code(path):
    """Source without comments, so a comment may still name what it avoids."""
    text = path.read_text(encoding="utf-8")
    return "\n".join(line.split("#", 1)[0] for line in text.splitlines())


def test_core_covers_the_daemon_webui_cli_and_installer():
    rel = {p.relative_to(SRC).as_posix() for p in _core_files()}
    for must in ("webui.py", "cli.py", "daemon/__init__.py", "daemon/startup.py",
                 "daemon/hotkeys.py", "install/installer.py"):
        assert must in rel


def test_core_modules_do_not_branch_on_the_os_or_reach_a_backend():
    bad = []
    for path in _core_files():
        code = _code(path)
        for pattern, why in FORBIDDEN:
            if pattern.search(code):
                bad.append("{0} {1}".format(path.relative_to(SRC).as_posix(), why))
    assert bad == []


def test_only_the_platform_factory_branches_on_the_os():
    factory = (SRC / "platform" / "__init__.py").read_text(encoding="utf-8")
    assert "sys.platform" in factory  # the one allowed branch
