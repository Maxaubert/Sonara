"""An example Python host using the installed ``sonara-client`` package:
connect (autostarting the runtime named by ``SONARA_RUNTIME``), speak and
wait for the item to finish.

    pip install ./clients/python
    SONARA_RUNTIME=target/release/sonarad.exe python packaging/smoke/python_host.py

When ``SONARA_HOME`` is unset it uses a temp home and the fake engine, so the
run is silent and isolated; an app passes neither.
"""
from __future__ import annotations

import os
import sys
import tempfile

import sonara_client


def main() -> int:
    if not os.environ.get("SONARA_RUNTIME"):
        print("set SONARA_RUNTIME to a sonarad.exe", file=sys.stderr)
        return 2
    home = os.environ.get("SONARA_HOME") or tempfile.mkdtemp(prefix="sonara-smoke-py-")
    args = ["--engine", "fake", "--idle-exit", "2"]
    with sonara_client.connect("smoke-python", home=home, runtime_args=args) as sonara:
        print(f"connected to Sonara {sonara.info['version']} (pid {sonara.runtime['pid']})")
        with sonara.subscribe(["items"]) as events:
            item = sonara.speak("Hello from a Python app that bundles Sonara.")
            for e in events:
                if e["item_id"] == item and e["phase"] != "started":
                    print(f"item {item} {e['phase']}")
                    return 0 if e["phase"] == "finished" else 1
    return 1


if __name__ == "__main__":
    sys.exit(main())
