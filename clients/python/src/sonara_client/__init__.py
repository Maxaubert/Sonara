"""sonara-client: talk to the Sonara runtime (protocol v1). Standard library only.

    import sonara_client

    with sonara_client.connect("my-app", runtime_path=r"C:\\path\\to\\sonarad.exe") as sonara:
        item = sonara.speak("Hello.")
        for event in sonara.subscribe(["items"]):
            if event["item_id"] == item and event["phase"] != "started":
                break
"""
from __future__ import annotations

from .client import Client, Subscription
from .connect import connect
from .discovery import read_runtime, resolve_home
from .engines import SEND_MODES, Engines
from .errors import SonaraError
from .extensions import Agent, Channels, System
from .version import PROTOCOL, __version__

__all__ = [
    "Agent",
    "Channels",
    "Client",
    "Engines",
    "PROTOCOL",
    "SEND_MODES",
    "SonaraError",
    "Subscription",
    "System",
    "__version__",
    "connect",
    "read_runtime",
    "resolve_home",
]
