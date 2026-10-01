"""Settings messages and setters (#141): SET_RATE, SET_VOICE, SET_VERBOSITY,
SET_MINQUEUE, SET_SUMMARY_MODE, SET_SESSION_PREF and STATUS, plus the
settings page's config-only keys (set_config_value) and custom summarizer
instructions (set_summary_prompt). Values are cleaned by config_schema
(#136) before anything reaches disk."""
from __future__ import annotations

from sonara import config_schema
from sonara.daemon import core
from sonara.protocol import MsgType


class Settings:
    """Message handlers for the settings. Given the daemon (*daemon*) for the
    config, speaker, cues and router, and looks them up at call time. The
    handlers run with the daemon lock held; set_config_value and
    set_summary_prompt are called from the settings page's HTTP threads and
    take the lock themselves. Persists through daemon._persist(). No state
    of its own."""

    def __init__(self, daemon) -> None:
        self._d = daemon

    def register(self, table: dict) -> None:
        core.add_handlers(table, {
            MsgType.SET_RATE: self.on_rate,
            MsgType.SET_VOICE: self.on_voice,
            MsgType.SET_SESSION_PREF: self.on_session_pref,
            MsgType.SET_VERBOSITY: self.on_verbosity,
            MsgType.SET_MINQUEUE: self.on_minqueue,
            MsgType.SET_SUMMARY_MODE: self.on_summary_mode,
            MsgType.STATUS: self.on_status,
        })

    def on_rate(self, msg):
        d = self._d
        is_delta = "delta" in msg
        if is_delta:
            try:
                target = (int(config_schema.get(d.config, "rate"))
                          + int(msg.get("delta", 0)))
            except (ValueError, TypeError):
                return None
        else:
            target = msg.get("rate")
        # Validate/clamp the rate in both branches -- an unvalidated value
        # here is persisted to disk and breaks synthesis.
        rate = config_schema.clean("rate", target)
        if rate is config_schema.INVALID:
            return None
        d.config["rate"] = rate
        d.speaker.set_rate(rate)
        d._persist()
        if is_delta:
            # A control cue (F6): on the session channel it waited behind
            # minqueue and could wipe the placeholder seed.
            d._cues.speak(d.sessions.foreground(),
                          "Rate {0}.".format(rate), exempt_mute=True,
                          pause_exempt=True, cue_key="rate")
            d._wake.set()
        return None

    def on_voice(self, msg):
        d = self._d
        voice = msg.get("voice")
        d.config["voice"] = voice
        d.speaker.set_voice(voice)
        d._persist()
        return None

    def on_session_pref(self, msg):
        d = self._d
        sid = msg.get("session")
        key = msg.get("key")
        if not isinstance(sid, str) or not d.session_prefs.set(sid, key, msg.get("value")):
            return None
        if key == "muted":
            val = bool(msg.get("value"))
            ch = d.router.channels.get(sid)
            if ch is not None:
                ch.muted = val
            cur = d._current_item
            if val and cur is not None and getattr(cur, "session", None) == sid:
                d.speaker.cancel()
            d._wake.set()
        return None

    def on_verbosity(self, msg):
        d = self._d
        d.config["verbosity"] = msg.get("verbosity")
        d._persist()
        return None

    def on_minqueue(self, msg):
        # Validate/clamp before persisting -- a bad value reaches disk and would
        # wedge prose buffering on every turn (mirrors the SET_RATE guard).
        d = self._d
        n = config_schema.clean("minqueue", msg.get("minqueue"))
        if n is config_schema.INVALID:
            return None
        d.config["minqueue"] = n
        d._persist()
        return None

    def on_summary_mode(self, msg):
        d = self._d
        if "enabled" not in msg:
            return None
        enabled = config_schema.clean("summary_mode", msg.get("enabled"))
        d.config["summary_mode"] = enabled
        d._persist()
        target = d.router.active or d.sessions.foreground()
        d._cues.speak(target,
                      "Summary mode on." if enabled else "Summary mode off.",
                      exempt_mute=True, pause_exempt=True)
        d._wake.set()
        return None

    def on_status(self, msg):
        """The settings plus the state-stream snapshot (#143): the same
        fields and seq a subscriber gets, without the event "type"."""
        d = self._d
        out = {
            "verbosity": d.config.get("verbosity"),
            "rate": d.config.get("rate"),
            "voice": d.config.get("voice"),
            "foreground": d.sessions.foreground(),
            "minqueue": d.config.get("minqueue"),
        }
        snap = d._state.current()
        snap.pop("type", None)
        out.update(snap)
        return out

    def set_config_value(self, key: str, value) -> bool:
        """Set a config-only tuning key (settings page, #34). These have no
        protocol message; config_schema validates them (#136). Clean, set
        under the lock, persist, then run the key's live-apply hook (switching
        TO a Kokoro cue voice warms it, #60). Returns False for unknown
        keys/bad values."""
        d = self._d
        if key not in config_schema.config_only_keys():
            return False
        cleaned = config_schema.clean(key, value)
        if cleaned is config_schema.INVALID:
            return False
        with d._lock:
            d.config[key] = cleaned
            d._persist()
        hook = config_schema.SCHEMA[key].apply
        if hook:
            getattr(d, hook)()
        return True

    def set_summary_prompt(self, style, text) -> bool:
        """Store or reset a per-style custom summarizer instruction (#58).
        text=None (or text equal to the built-in default) resets to default;
        empty/whitespace text is rejected (an empty instruction would strip
        the never-addressed-to-you firewall from the call)."""
        d = self._d
        if style not in ("tidy", "natural", "brief"):
            return False
        from sonara.summarizer import default_instruction
        if text is not None:
            text = str(text)
            if not text.strip():
                return False
            if text == default_instruction(style):
                text = None                     # storing the default = reset
        with d._lock:
            prompts = dict(d.config.get("summary_prompts") or {})
            if text is None:
                prompts.pop(style, None)
            else:
                prompts[style] = text
            d.config["summary_prompts"] = prompts
            d._persist()
        return True
