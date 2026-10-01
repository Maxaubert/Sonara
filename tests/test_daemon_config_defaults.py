"""The daemon reads defaults from the config schema, not inline copies (#136).

Audit L-duck-level: the duck level fell back to 20 while the default is 30.
Audit L-settle-default / DC10: summary_settle_ms had only an inline default.
Audit M8: the bundled earcon paths were frozen into config at startup.
"""
from __future__ import annotations

from unittest import mock

import sonara.daemon as daemon_mod
from sonara import config_schema
from tests.daemon_helpers import make_daemon


def test_duck_level_fallback_is_the_schema_default():
    daemon, *_ = make_daemon()
    daemon.config["duck_level"] = "loud"            # hand-edited junk
    assert daemon._audio.duck_level() == 30
    del daemon.config["duck_level"]
    assert daemon._audio.duck_level() == 30
    assert config_schema.default("duck_level") == 30


def test_settle_delay_defaults_from_the_schema(monkeypatch):
    daemon, *_ = make_daemon()
    daemon.config.pop("summary_settle_ms", None)
    started = []

    class _Timer:
        def __init__(self, interval, fn, args=()):
            started.append(interval)
            self.daemon = False

        def start(self):
            pass

        def cancel(self):
            pass

    monkeypatch.setattr(daemon_mod.threading, "Timer", _Timer)
    daemon._summary.schedule_settle("s1", 1)
    assert started == [0.6]


def test_resolve_earcons_overlays_user_overrides_on_the_bundled_set():
    bundled = {"nav": "/app/nav.wav", "choice": "/app/choice.wav"}
    out = daemon_mod.resolve_earcons(bundled, {"choice": "C:/mine/ding.wav",
                                               "bogus": 5})
    assert out == {"nav": "/app/nav.wav", "choice": "C:/mine/ding.wav"}
    assert daemon_mod.resolve_earcons(bundled, None) == bundled


def test_main_resolves_earcons_at_runtime_without_touching_config():
    # A config carrying an (old, partial) earcon map still gets every bundled
    # kind, and nothing is written into the config dict for a later save.
    fake_cfg = {"voice": None, "rate": 200, "verbosity": "everything",
                "background_policy": "earcon_only",
                "earcons": {"choice": "C:/mine/ding.wav"}}
    bundled = {"nav": "/app/nav.wav", "choice": "/app/choice.wav"}
    plat = mock.MagicMock()
    plat.earcon.default_earcons.return_value = bundled
    with mock.patch("sonara.daemon.load_config", return_value=fake_cfg), \
         mock.patch("sonara.platform.get_platform", return_value=plat), \
         mock.patch("sonara.daemon.socket_connectable", return_value=False), \
         mock.patch("sonara.daemon.transport.acquire_singleton_mutex", return_value=object()), \
         mock.patch("sonara.daemon.transport.acquire_singleton", return_value=object()), \
         mock.patch("sonara.platform.windows.ducking.restore_from_state_file"), \
         mock.patch("sonara.platform.windows.pausing.resume_from_state_file"), \
         mock.patch("sonara.daemon.SpeechDaemon.run", autospec=True) as run:
        daemon_mod.main()
    built = run.call_args[0][0]
    assert built.speaker._earcons == {"nav": "/app/nav.wav",
                                      "choice": "C:/mine/ding.wav"}
    assert fake_cfg["earcons"] == {"choice": "C:/mine/ding.wav"}
