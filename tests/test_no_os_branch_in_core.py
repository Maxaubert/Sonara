"""Guard: the portable core must never branch on the OS or import a concrete
backend. The ONLY sys.platform branch in the whole codebase lives in
platform/__init__.py's get_platform() factory.

keymap.py is intentionally NOT in CORE: it re-exports KEY_CODES/MOD_MASKS from
platform.macos.keytables (a concrete-backend import). M3 adds the Windows
keytables + a resolver-injection that removes even that; until then keymap.py is
a documented macOS-coupled module.
"""
import pathlib

CORE = [
    "assembler.py", "cleaner.py", "queue.py", "history.py", "sessions.py",
    "protocol.py", "hooks_entry.py", "speaker.py", "config.py",
]
SRC = pathlib.Path(__file__).resolve().parents[1] / "src" / "sonara"


def test_core_modules_have_no_sys_platform_branch():
    for name in CORE:
        text = (SRC / name).read_text(encoding="utf-8")
        assert "sys.platform" not in text, "{0} branches on sys.platform".format(name)


def test_core_modules_do_not_import_macos_backend():
    for name in CORE:
        text = (SRC / name).read_text(encoding="utf-8")
        assert "platform.macos" not in text, \
            "{0} imports a concrete backend".format(name)


def test_only_factory_branches_on_platform():
    factory = (SRC / "platform" / "__init__.py").read_text(encoding="utf-8")
    assert "sys.platform" in factory  # the one allowed branch
