"""Router integration with per-session prefs: names, mute seeding, cycle skip."""
from sonara.router import Router
from sonara.queue import SpeechItem


class Sessions:
    def __init__(self):
        self._fg = None
        self._folders = {}
    def foreground(self): return self._fg
    def folder(self, sid): return self._folders.get(sid)


def make_router(display_name=None, channel_init=None):
    s = Sessions()
    r = Router(s, minqueue=lambda: 1,
               announce_text=lambda f, replay=False: f"Session changed: {f}.",
               display_name=display_name, channel_init=channel_init)
    return r


def _item(session, text="hi"):
    return SpeechItem(id=0, session=session, kind="prose", text=text, is_decision=False)


def test_announcement_uses_display_name():
    names = {"s2": "build box"}
    r = make_router(display_name=lambda sid: names.get(sid))
    r.channel("s2").append(_item("s2")); r.channel("s2").turn_done = True
    r._arm_switch("s2", replay=False)             # the switch announcement is pending
    got = r.next_item()
    assert got.kind == "session_change"
    assert "build box" in got.text


def test_display_name_falls_back_to_folder():
    r = make_router(display_name=lambda sid: None)
    r.sessions._folders["s2"] = "proj"
    r.channel("s2").append(_item("s2")); r.channel("s2").turn_done = True
    r._arm_switch("s2", replay=False)
    got = r.next_item()
    assert got.kind == "session_change"
    assert "proj" in got.text


def test_channel_init_seeds_mute():
    muted = {"s1": True}
    r = make_router(channel_init=lambda ch: setattr(ch, "muted",
                                                    muted.get(ch.session, False)))
    assert r.channel("s1").muted is True
    assert r.channel("s2").muted is False


def test_next_session_skips_muted():
    r = make_router()
    for s in ("s1", "s2", "s3"):
        r.channel(s).append(_item(s)); r.channel(s).turn_done = True
    r.channels["s2"].muted = True
    r.active = "s1"
    target, _replay = r.next_session()
    assert target == "s3"                        # s2 skipped


def test_next_session_falls_back_when_all_muted():
    r = make_router()
    for s in ("s1", "s2"):
        r.channel(s).append(_item(s)); r.channel(s).turn_done = True
        r.channels[s].muted = True
    target, _replay = r.next_session()
    assert target is not None                    # degrade, do not dead-end
