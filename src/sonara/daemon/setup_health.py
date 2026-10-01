"""Session-start setup guidance: when Sonara is not installed (no install
record or no launcher) or the plugin was updated since the last install, the
daemon speaks one cue per session telling the user to run /sonara install."""
from __future__ import annotations

from typing import Optional

from sonara import install_record


def launcher_present() -> bool:
    """Delegating shim -- logic lives in the platform supervisor backend."""
    from sonara.platform import get_platform
    return get_platform().supervisor.is_installed()


def health(plugin_version: str):
    """Return (state, cue) where state is one of:
    "ok"            -> fully installed, no version drift   -> cue None
    "not_installed" -> no install.json or launcher (never ran `sonara install`)
    "version_drift" -> installed but plugin_version differs from this session's

    Cheap: a few file stats, a string compare, and on Windows one windowless
    `schtasks /query` (via the platform supervisor). Never raises.
    Hotkey availability is deliberately NOT part of this check so a deliberate
    speech-only user is never nagged.
    """
    rec = install_record.read()
    installed = (rec is not None and launcher_present())
    if not installed:
        return ("not_installed",
                "Sonara is reading aloud. To enable hotkeys and autostart, "
                "run, slash sonara install.")
    recorded = (rec.get("plugin_version") or "")
    # Only flag drift when BOTH sides are known and differ.
    if plugin_version and recorded and plugin_version != recorded:
        return ("version_drift",
                "Sonara was updated. Run, slash sonara install, to apply.")
    return ("ok", None)


class SetupGuide:
    """Throttles the guidance to at most one cue per session."""

    def __init__(self) -> None:
        self._guided: set = set()

    def health(self, plugin_version: str):
        """Test seam over the module-level health()."""
        return health(plugin_version)

    def cue_for(self, session: str, plugin_version: str) -> Optional[str]:
        """The ONE setup-guidance cue for this session, only when degraded.

        Throttle: at most once per session (recorded whether or not a cue fires).
        Silent when healthy. The check is a few file stats + a version compare
        (plus a windowless `schtasks /query` on Windows) and never raises.
        """
        if session in self._guided:
            return None
        try:
            state, cue = self.health(plugin_version or "")
        except Exception:  # noqa: BLE001 - guidance must never break a session
            return None
        self._guided.add(session)
        if state != "ok" and cue:
            return cue
        return None

    def forget(self, session: str) -> None:
        """A session ended: its next lifecycle may be guided again."""
        self._guided.discard(session)
