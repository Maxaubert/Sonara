"""The durable install record, ~/.sonara/install.json: `sonara install` writes
it, and doctor and the daemon's session-start health check read it."""
from __future__ import annotations

import json
import os

from sonara import paths


def write(python: str, python_version: str, plugin_root: str, app_path: str,
          plugin_version: str) -> None:
    """Persist the durable install record used by doctor + session-start health."""
    from datetime import datetime, timezone
    record = {
        "python": python,
        "python_version": python_version,
        "app_path": app_path,
        "plugin_root": plugin_root,
        "plugin_version": plugin_version,
        "installed_at": datetime.now(timezone.utc).isoformat(),
    }
    os.makedirs(os.path.dirname(str(paths.INSTALL_RECORD_PATH)), exist_ok=True)
    with open(str(paths.INSTALL_RECORD_PATH), "w", encoding="utf-8") as f:
        json.dump(record, f, indent=2)
        f.write("\n")


def read():
    """Return the install.json record dict, or None if unreadable/absent. Never
    raises: doctor and the session-start health check must not fail on it."""
    try:
        with open(str(paths.INSTALL_RECORD_PATH), "r", encoding="utf-8") as f:
            data = json.load(f)
        return data if isinstance(data, dict) else None
    except Exception:  # noqa: BLE001 - doctor / health check must never raise
        return None
