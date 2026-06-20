"""Inject the fake `sonara.client` test double via sys.modules at startup.

`bin/sonara-hook` unconditionally puts the plugin's own `src/` at `sys.path[0]`
so a stale global `sonara` can never shadow it. That means a cooperative
`pkgutil.extend_path` namespace can no longer be relied on to override a single
submodule (the plugin's `src/sonara/__init__.py` is a plain package, not a
namespace). To let the hook tests substitute a socket-free `client`, we register
the fake under `sys.modules["sonara.client"]` here — `sitecustomize` runs at
interpreter startup, before `bin/sonara-hook` executes, so the later
`from sonara import client` returns this fake while `sonara` itself and every
other submodule (e.g. `hooks_entry`, `protocol`) still resolve from `src/`.

This is the "inject via sys.modules" approach: the test double lives entirely in
the test harness and the production hot path stays a simple insert-at-0 with no
disk scanning or namespace detection. Activated only when this directory is on
PYTHONPATH (i.e. from the hook tests).
"""
import importlib.util
import os
import sys

_HERE = os.path.dirname(os.path.abspath(__file__))
_FAKE_CLIENT = os.path.join(_HERE, "sonara", "client.py")

if os.path.isfile(_FAKE_CLIENT) and "sonara.client" not in sys.modules:
    _spec = importlib.util.spec_from_file_location("sonara.client", _FAKE_CLIENT)
    _module = importlib.util.module_from_spec(_spec)
    _spec.loader.exec_module(_module)
    sys.modules["sonara.client"] = _module
