# Session Manager Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A "Sessions" tab in the settings page: per-session custom name, mute toggle, and voice override, backed by a durable prefs store.

**Architecture:** New `SessionPrefs` store (`session_prefs.json`, mirrors `sessions.py` storage discipline). Daemon applies prefs at three seams: channel creation / live toggle (mute), announcement text (name), speak-time voice kwargs (voice). Web API exposes a `sessions` array in state plus a `/api/session` mutation endpoint; the page renders a Sessions page in the existing sidebar pattern.

**Tech Stack:** stdlib-only Python 3.14, pytest, vanilla JS in `settings.html`.

**Spec:** `docs/superpowers/specs/2026-07-20-session-manager-design.md`

## Global Constraints

- stdlib only; no new dependencies.
- No em-dashes in any user-visible copy or docs.
- Prefs persistence is best-effort: a storage failure must never break message handling (mirror `sessions.py`: swallow `OSError`, atomic tmp+replace, cap 200).
- Mute semantics: muted session speech is silent; mute-exempt cues still speak; attention earcons still fire (they are enqueue-time, untouched).
- Voice resolution priority: fast-cue override, then session voice pref, then global default.
- Forgetting the foreground session is refused everywhere (daemon and web API).
- The settings page is poll-refreshed every 3s: the Sessions list must not clobber an input the user is editing (rebuild only on signature change and never while a row input has focus).
- Known baseline test failures (never "fix"): test_bin_sonara x3, test_daemon_ducking duck_level 20 vs 30, test_paths x2, test_transport, test_win_tts x2+1 error.

---

### Task 1: SessionPrefs store

**Files:**
- Create: `src/sonara/session_prefs.py`
- Test: `tests/test_session_prefs.py`

**Interfaces:**
- Produces: `SessionPrefs(store_path=None, store_cap=200)` with `name(sid) -> str|None`, `muted(sid) -> bool`, `voice(sid) -> str|None`, `get(sid) -> dict`, `set(sid, key, value) -> bool`, `forget(sid) -> None`. Later tasks rely on these exact names.

- [ ] **Step 1: Write the failing tests**

```python
"""Per-session prefs store: durable name/mute/voice map (#session-manager)."""
import json

from sonara.session_prefs import SessionPrefs


def test_defaults_for_unknown_session():
    p = SessionPrefs()
    assert p.name("s1") is None
    assert p.muted("s1") is False
    assert p.voice("s1") is None
    assert p.get("s1") == {}


def test_set_and_read_back():
    p = SessionPrefs()
    assert p.set("s1", "name", "build box")
    assert p.set("s1", "muted", True)
    assert p.set("s1", "voice", "af_heart")
    assert p.name("s1") == "build box"
    assert p.muted("s1") is True
    assert p.voice("s1") == "af_heart"


def test_falsy_name_and_voice_clear_the_key():
    p = SessionPrefs()
    p.set("s1", "name", "x")
    p.set("s1", "voice", "af_heart")
    p.set("s1", "name", "")
    p.set("s1", "voice", None)
    assert p.name("s1") is None
    assert p.voice("s1") is None


def test_unknown_key_and_bad_session_rejected():
    p = SessionPrefs()
    assert p.set("s1", "rate", 300) is False
    assert p.set(None, "name", "x") is False
    assert p.get("s1") == {}


def test_name_capped_at_60_chars():
    p = SessionPrefs()
    p.set("s1", "name", "x" * 200)
    assert len(p.name("s1")) == 60


def test_persist_roundtrip(tmp_path):
    store = tmp_path / "session_prefs.json"
    p = SessionPrefs(store_path=store)
    p.set("s1", "name", "alpha")
    p.set("s1", "muted", True)
    p2 = SessionPrefs(store_path=store)
    assert p2.name("s1") == "alpha"
    assert p2.muted("s1") is True


def test_forget_removes_and_persists(tmp_path):
    store = tmp_path / "session_prefs.json"
    p = SessionPrefs(store_path=store)
    p.set("s1", "name", "alpha")
    p.forget("s1")
    assert p.get("s1") == {}
    assert SessionPrefs(store_path=store).get("s1") == {}


def test_corrupt_store_is_a_silent_noop(tmp_path):
    store = tmp_path / "session_prefs.json"
    store.write_text("{not json", encoding="utf-8")
    p = SessionPrefs(store_path=store)
    assert p.get("s1") == {}
    assert p.set("s1", "name", "ok")           # still writable after corruption


def test_cap_evicts_oldest(tmp_path):
    store = tmp_path / "session_prefs.json"
    p = SessionPrefs(store_path=store, store_cap=3)
    for i in range(5):
        p.set(f"s{i}", "name", f"n{i}")
    data = json.loads(store.read_text(encoding="utf-8"))
    assert len(data) == 3
    assert "s0" not in data and "s4" in data


def test_empty_entry_dropped_from_store(tmp_path):
    # clearing the last pref removes the whole entry (no {} litter)
    store = tmp_path / "session_prefs.json"
    p = SessionPrefs(store_path=store)
    p.set("s1", "name", "x")
    p.set("s1", "name", "")
    data = json.loads(store.read_text(encoding="utf-8"))
    assert "s1" not in data
```

- [ ] **Step 2: Run to verify failure**

Run: `python -m pytest tests/test_session_prefs.py -q`
Expected: FAIL with `ModuleNotFoundError: No module named 'sonara.session_prefs'`

- [ ] **Step 3: Implement the module**

```python
"""Per-session user preferences: display name, mute, voice override.

A tiny durable map keyed by Claude Code session id. Follows sessions.py's
storage discipline: opt-in store_path (tests stay pure), best-effort atomic
JSON writes (every failure swallowed), capped to the most recent entries,
missing/corrupt file tolerated.
"""
from __future__ import annotations

import json
import os

_ALLOWED_KEYS = ("name", "muted", "voice")
_NAME_MAX = 60


class SessionPrefs:
    def __init__(self, store_path=None, store_cap: int = 200) -> None:
        self._store_path = store_path
        self._store_cap = store_cap
        self._prefs: "dict[str, dict]" = {}
        if store_path is not None:
            self._load()

    def get(self, session: str) -> dict:
        return dict(self._prefs.get(session) or {})

    def name(self, session: str) -> "str | None":
        v = (self._prefs.get(session) or {}).get("name")
        return str(v) if v else None

    def muted(self, session: str) -> bool:
        return bool((self._prefs.get(session) or {}).get("muted"))

    def voice(self, session: str) -> "str | None":
        v = (self._prefs.get(session) or {}).get("voice")
        return str(v) if v else None

    def set(self, session: str, key: str, value) -> bool:
        """Set one pref; returns False for an unknown key or bad session id.
        A falsy name/voice clears the key; muted coerces to bool."""
        if not isinstance(session, str) or not session or key not in _ALLOWED_KEYS:
            return False
        entry = self._prefs.setdefault(session, {})
        if key == "muted":
            entry["muted"] = bool(value)
            if not entry["muted"]:
                entry.pop("muted", None)          # default is unmuted: no litter
        elif value:
            entry[key] = str(value)[:_NAME_MAX] if key == "name" else str(value)
        else:
            entry.pop(key, None)
        if not entry:
            self._prefs.pop(session, None)
        self._persist()
        return True

    def forget(self, session: str) -> None:
        if self._prefs.pop(session, None) is not None:
            self._persist()

    # --- durable store (opt-in via store_path), mirrors sessions.py -------

    def _load(self) -> None:
        try:
            with open(str(self._store_path), "r", encoding="utf-8") as fh:
                data = json.load(fh)
        except (FileNotFoundError, ValueError, OSError):
            return
        if not isinstance(data, dict):
            return
        for sid, entry in data.items():
            if isinstance(sid, str) and isinstance(entry, dict) and entry:
                kept = {k: entry[k] for k in _ALLOWED_KEYS if k in entry}
                if kept:
                    self._prefs[sid] = kept

    def _persist(self) -> None:
        if self._store_path is None:
            return
        try:
            data = {k: v for k, v in self._prefs.items() if v}
            if len(data) > self._store_cap:
                data = dict(list(data.items())[-self._store_cap:])
            path = str(self._store_path)
            parent = os.path.dirname(path)
            if parent:
                os.makedirs(parent, exist_ok=True)
            tmp = path + ".tmp"
            with open(tmp, "w", encoding="utf-8") as fh:
                json.dump(data, fh)
                fh.flush()
                os.fsync(fh.fileno())
            os.replace(tmp, path)
        except OSError:
            pass
```

Note: `test_cap_evicts_oldest` requires eviction to also apply to the in-memory map on the NEXT store write; the given `_persist` caps only the written data, which satisfies the test as written (the store file holds 3). Keep it that way (matches `sessions.py`).

- [ ] **Step 4: Run to verify pass**

Run: `python -m pytest tests/test_session_prefs.py -q`
Expected: all pass

- [ ] **Step 5: Commit**

```bash
git add src/sonara/session_prefs.py tests/test_session_prefs.py
git commit -m "feat(sessions): SessionPrefs durable per-session name/mute/voice store"
```

---

### Task 2: Protocol ops + daemon wiring and handlers

**Files:**
- Modify: `src/sonara/protocol.py` (MsgType block)
- Modify: `src/sonara/paths.py` (after `SESSIONS_PATH`)
- Modify: `src/sonara/daemon.py` (`__init__`, handler block near `SET_VOICE`, `main()`)
- Test: `tests/test_daemon_session_prefs.py`

**Interfaces:**
- Consumes: `SessionPrefs` from Task 1.
- Produces: `SpeechDaemon.session_prefs` attribute; message types `set_session_pref` `{session, key, value}` and `forget_session` `{session}`. Tasks 3, 4, 6 rely on `self.session_prefs`.

- [ ] **Step 1: Write the failing tests**

Model construction on the existing daemon tests (see `tests/daemon_helpers.py` for `FakeSpeaker`; follow the same `SpeechDaemon(FakeSpeaker(), SessionManager(), config, ...)` construction the other `tests/test_daemon_*.py` files use, adapting fixture names to what `daemon_helpers` actually exports):

```python
"""SET_SESSION_PREF / FORGET_SESSION daemon handlers."""
from sonara.daemon import SpeechDaemon
from sonara.sessions import SessionManager
from sonara.session_prefs import SessionPrefs
from tests.daemon_helpers import FakeSpeaker


def make_daemon(prefs=None):
    d = SpeechDaemon(FakeSpeaker(), SessionManager(), {"minqueue": 1},
                     prefs=prefs or SessionPrefs())
    return d


def test_set_pref_persists_via_store():
    p = SessionPrefs()
    d = make_daemon(prefs=p)
    d.handle_message({"v": 1, "type": "set_session_pref",
                      "session": "s1", "key": "name", "value": "alpha"})
    assert p.name("s1") == "alpha"


def test_set_muted_applies_to_live_channel():
    d = make_daemon()
    ch = d.router.channel("s1")
    d.handle_message({"v": 1, "type": "set_session_pref",
                      "session": "s1", "key": "muted", "value": True})
    assert ch.muted is True
    d.handle_message({"v": 1, "type": "set_session_pref",
                      "session": "s1", "key": "muted", "value": False})
    assert ch.muted is False


def test_muting_the_speaking_session_cancels_current():
    d = make_daemon()
    d.router.channel("s1")

    class Cur:
        session = "s1"
    d._current_item = Cur()
    d.handle_message({"v": 1, "type": "set_session_pref",
                      "session": "s1", "key": "muted", "value": True})
    assert d.speaker.cancelled          # FakeSpeaker records cancel()


def test_bad_key_or_session_is_ignored():
    p = SessionPrefs()
    d = make_daemon(prefs=p)
    d.handle_message({"v": 1, "type": "set_session_pref",
                      "session": "s1", "key": "rate", "value": 300})
    d.handle_message({"v": 1, "type": "set_session_pref",
                      "session": 7, "key": "name", "value": "x"})
    assert p.get("s1") == {}


def test_forget_session_clears_everything():
    p = SessionPrefs()
    d = make_daemon(prefs=p)
    d.sessions.register("s1", cwd="/x/proj")
    p.set("s1", "name", "alpha")
    d.router.channel("s1")
    d.handle_message({"v": 1, "type": "forget_session", "session": "s1"})
    assert p.get("s1") == {}
    assert d.sessions.folder("s1") is None
    assert "s1" not in d.router.channels


def test_forget_refuses_foreground():
    d = make_daemon()
    d.sessions.set_foreground("s1", cwd="/x/proj")
    d.router.channel("s1")
    d.handle_message({"v": 1, "type": "forget_session", "session": "s1"})
    assert "s1" in d.router.channels
```

If `FakeSpeaker` has no `cancelled` flag, add one (set in its `cancel()`); check first, the helpers already record most calls.

- [ ] **Step 2: Run to verify failure**

Run: `python -m pytest tests/test_daemon_session_prefs.py -q`
Expected: FAIL (`unexpected keyword argument 'prefs'`)

- [ ] **Step 3: Implement**

`protocol.py`, after `NEXT_SESSION`:

```python
    SET_SESSION_PREF = "set_session_pref"   # {session, key: name|muted|voice, value}
    FORGET_SESSION = "forget_session"       # {session}: drop a stale session everywhere
```

`paths.py`, after `SESSIONS_PATH`:

```python
SESSION_PREFS_PATH = SONARA_DIR / "session_prefs.json"  # per-session name/mute/voice
```

`daemon.py` `__init__` signature and body (pattern-match `pauser`):

```python
    def __init__(self, speaker, sessions, config, ducker=None, pauser=None,
                 prefs=None) -> None:
```

```python
        if prefs is None:
            from sonara.session_prefs import SessionPrefs
            prefs = SessionPrefs()
        self.session_prefs = prefs
```

(place the block right after the `pauser` default, BEFORE the Router construction; Task 3 hooks the Router into prefs).

`daemon.py` handlers, insert directly after the `SET_VOICE` handler's `return`:

```python
        if t == MsgType.SET_SESSION_PREF:
            sid = msg.get("session")
            key = msg.get("key")
            if not isinstance(sid, str) or not self.session_prefs.set(sid, key, msg.get("value")):
                return None
            if key == "muted":
                val = bool(msg.get("value"))
                ch = self.router.channels.get(sid)
                if ch is not None:
                    ch.muted = val
                cur = self._current_item
                if val and cur is not None and getattr(cur, "session", None) == sid:
                    self.speaker.cancel()
                self._wake.set()
            return None

        if t == MsgType.FORGET_SESSION:
            sid = msg.get("session")
            if not isinstance(sid, str) or self.sessions.is_foreground(sid):
                return None
            self.sessions.unregister(sid)
            self.session_prefs.forget(sid)
            self.router.drop(sid)
            return None
```

`daemon.py` `main()`: extend the construction site:

```python
    from sonara.session_prefs import SessionPrefs
    from sonara.paths import SESSION_PREFS_PATH
    daemon = SpeechDaemon(speaker, sessions, cfg,
                          ducker=_backend.ducker, pauser=_backend.pauser,
                          prefs=SessionPrefs(store_path=SESSION_PREFS_PATH))
```

(reuse the existing import style at that site; `SESSIONS_PATH` is already imported there, extend that import line instead of adding a new one if present).

- [ ] **Step 4: Run to verify pass**

Run: `python -m pytest tests/test_daemon_session_prefs.py tests/test_daemon_audio_mode.py -q`
Expected: all pass (audio-mode suite guards the handler block against typos)

- [ ] **Step 5: Commit**

```bash
git add src/sonara/protocol.py src/sonara/paths.py src/sonara/daemon.py tests/test_daemon_session_prefs.py tests/daemon_helpers.py
git commit -m "feat(sessions): set_session_pref / forget_session daemon ops"
```

---

### Task 3: Router honors prefs (custom name, mute seeding, cycle skip)

**Files:**
- Modify: `src/sonara/router.py` (`__init__`, `channel`, `next_item` announce site, `next_session`)
- Modify: `src/sonara/daemon.py` (Router construction)
- Test: `tests/test_router_session_prefs.py`

**Interfaces:**
- Consumes: `SpeechDaemon.session_prefs` (Task 2).
- Produces: `Router(..., display_name=None, channel_init=None)`; both optional, `None` keeps old behavior. Announcement label resolves via `display_name(sid)` falling back to `sessions.folder(sid)`.

- [ ] **Step 1: Write the failing tests**

Model on the existing `tests/test_router*.py` construction (a stub sessions object exposing `foreground()`/`folder()`; reuse the file's existing stub if one exists):

```python
"""Router integration with per-session prefs: names, mute seeding, cycle skip."""
from sonara.router import Router, CONTROL
from sonara.queue import SpeechItem


class Sessions:
    def __init__(self):
        self._fg = None
        self._folders = {}
    def foreground(self): return self._fg
    def folder(self, sid): return self._folders.get(sid)
    def is_foreground(self, sid): return sid == self._fg
    def should_speak(self, sid): return True


def make_router(display_name=None, channel_init=None):
    return Router(Sessions(), minqueue=lambda: 1,
                  announce_text=lambda f, replay=False: f"Session changed: {f}.",
                  display_name=display_name, channel_init=channel_init)


def _item(session, text="hi"):
    return SpeechItem(id=1, session=session, kind="prose", text=text,
                      is_decision=False)


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
```

Adapt `_item` to `SpeechItem`'s real constructor (check `src/sonara/queue.py`; pass whatever fields are required, e.g. `mute_exempt=False, pause_exempt=False`, matching existing router tests). Adapt the two announcement tests to the file's existing switch-forcing idiom if `test_router*.py` already has one; the assertion that matters is the label text.

- [ ] **Step 2: Run to verify failure**

Run: `python -m pytest tests/test_router_session_prefs.py -q`
Expected: FAIL (`unexpected keyword argument 'display_name'`)

- [ ] **Step 3: Implement**

`router.py` `__init__`:

```python
    def __init__(self, sessions, minqueue, announce_text,
                 display_name=None, channel_init=None) -> None:
```

```python
        self._display_name = display_name    # sid -> custom label | None
        self._channel_init = channel_init    # (SessionChannel) -> None, on create
```

`channel()`:

```python
    def channel(self, session: str) -> SessionChannel:
        ch = self.channels.get(session)
        if ch is None:
            ch = SessionChannel(session)
            if self._channel_init is not None:
                self._channel_init(ch)       # seed prefs (e.g. muted) on creation
            self.channels[session] = ch
        return ch
```

`next_item()` announce site: replace

```python
            folder = self.sessions.folder(self._pending_announce) or "another session"
```

with

```python
            label = None
            if self._display_name is not None:
                label = self._display_name(self._pending_announce)
            folder = (label or self.sessions.folder(self._pending_announce)
                      or "another session")
```

`next_session()`: after `keys = [...]`, insert

```python
        # A muted session never takes the floor on a manual cycle; if EVERY
        # other session is muted, degrade to the plain ring (never dead-end).
        audible = [s for s in keys if not self.channels[s].muted]
        if audible:
            keys = audible
```

Note: when `self.active` is muted it drops out of `keys`, so the cycle lands on `keys[0]`; that is the intended "leave the muted session" behavior.

`daemon.py` Router construction:

```python
        self.router = Router(
            self.sessions,
            minqueue=self._minqueue,
            announce_text=lambda folder, replay=False: (
                "Session changed: {0}, reading again.".format(folder) if replay
                else "Session changed: {0}.".format(folder)),
            display_name=lambda sid: self.session_prefs.name(sid),
            channel_init=lambda ch: setattr(
                ch, "muted", self.session_prefs.muted(ch.session)),
        )
```

(the prefs block from Task 2 must sit above this; verify ordering.)

- [ ] **Step 4: Run to verify pass**

Run: `python -m pytest tests/test_router_session_prefs.py tests/ -q -k "router"`
Expected: all pass, existing router suites untouched

- [ ] **Step 5: Commit**

```bash
git add src/sonara/router.py src/sonara/daemon.py tests/test_router_session_prefs.py
git commit -m "feat(sessions): router speaks custom names, seeds mute, cycle skips muted"
```

---

### Task 4: Per-session voice at speak time

**Files:**
- Modify: `src/sonara/daemon.py` (`_voice_override` new method next to `_cue_voice_override`; the speak-loop call site)
- Test: `tests/test_daemon_session_voice.py`

**Interfaces:**
- Consumes: `session_prefs.voice(sid)` (Task 1/2); `_cue_voice_override(item)` (existing, unchanged).
- Produces: `SpeechDaemon._voice_override(item) -> dict` used by `_speak_loop_once`.

- [ ] **Step 1: Write the failing tests**

```python
"""Per-session voice override resolution (#session-manager)."""
from sonara.session_prefs import SessionPrefs
from sonara.router import CONTROL
from tests.test_daemon_session_prefs import make_daemon


class Item:
    def __init__(self, session, kind="prose"):
        self.session = session
        self.kind = kind


def test_session_voice_pref_wins_over_default():
    p = SessionPrefs()
    p.set("s1", "voice", "af_nicole")
    d = make_daemon(prefs=p)
    assert d._voice_override(Item("s1")) == {"voice": "af_nicole"}


def test_no_pref_means_no_override():
    d = make_daemon()
    assert d._voice_override(Item("s1")) == {}


def test_cue_override_beats_session_pref():
    p = SessionPrefs()
    p.set(CONTROL, "voice", "af_nicole")     # nonsensical but must not matter
    d = make_daemon(prefs=p)
    d.config["fast_cues"] = True
    kw = d._voice_override(Item(CONTROL))
    assert kw == {"voice": d._cue_voice()}
```

- [ ] **Step 2: Run to verify failure**

Run: `python -m pytest tests/test_daemon_session_voice.py -q`
Expected: FAIL (`no attribute '_voice_override'`)

- [ ] **Step 3: Implement**

`daemon.py`, directly after `_cue_voice_override`:

```python
    def _voice_override(self, item) -> dict:
        """speaker.speak kwargs for *item*: the fast-cue voice for control
        feedback and session-change announcements (#60), else the session's
        voice pref, else {} (the global default voice)."""
        kw = self._cue_voice_override(item)
        if kw:
            return kw
        v = self.session_prefs.voice(item.session)
        return {"voice": v} if v else {}
```

Speak-loop call site: replace `**self._cue_voice_override(item)` with `**self._voice_override(item)` (single call site in `_speak_loop_once`).

- [ ] **Step 4: Run to verify pass**

Run: `python -m pytest tests/test_daemon_session_voice.py tests/test_daemon_alert_timing.py tests/test_daemon_audio_mode.py -q`
Expected: all pass (alert-timing suite exercises the speak loop end to end)

- [ ] **Step 5: Commit**

```bash
git add src/sonara/daemon.py tests/test_daemon_session_voice.py
git commit -m "feat(sessions): per-session voice override at speak time"
```

---

### Task 5: Catch-up skips muted sessions

**Files:**
- Modify: `src/sonara/history.py` (`other_session_with_unheard`)
- Modify: `src/sonara/daemon.py` (CATCH_UP handler call site)
- Test: extend `tests/test_history.py` (or the file that already tests `other_session_with_unheard`; locate with grep and add there)

**Interfaces:**
- Consumes: `session_prefs.muted` (Task 1/2).
- Produces: `other_session_with_unheard(exclude, skip=None)`; `skip` is `(sid) -> bool`.

- [ ] **Step 1: Write the failing test** (in the located history test file)

```python
def test_other_session_with_unheard_honors_skip():
    h = make_history()                       # the file's existing factory/fixture
    # arrange two other sessions with unheard entries, "b" more recent
    add_unheard(h, "a"); add_unheard(h, "b")  # use the file's existing helpers
    assert h.other_session_with_unheard("fg") == "b"
    assert h.other_session_with_unheard("fg", skip=lambda s: s == "b") == "a"
    assert h.other_session_with_unheard("fg", skip=lambda s: True) is None
```

Adapt the arrange lines to the file's real helpers (the suite already builds histories with unheard entries; reuse that idiom verbatim).

- [ ] **Step 2: Run to verify failure**

Run: `python -m pytest tests/ -q -k "other_session"`
Expected: FAIL (`unexpected keyword argument 'skip'`)

- [ ] **Step 3: Implement**

`history.py`:

```python
    def other_session_with_unheard(self, exclude: str, skip=None):
        """The most recently active OTHER session that has unheard entries,
        or None. Lets catch_up recover a session you left without re-typing
        in it (there is no OS window-focus hook). *skip*, when given, filters
        out sessions the caller must not surface (e.g. muted ones)."""
        best, best_tick = None, -1
        for session, tick in self._touch.items():
            if session == exclude:
                continue
            if skip is not None and skip(session):
                continue
            if tick > best_tick and self.unheard(session):
                best, best_tick = session, tick
        return best
```

`daemon.py` CATCH_UP handler:

```python
                other = self.history.other_session_with_unheard(
                    fg, skip=self.session_prefs.muted)
```

- [ ] **Step 4: Run to verify pass**

Run: `python -m pytest tests/ -q -k "history or catch"`
Expected: all pass

- [ ] **Step 5: Commit**

```bash
git add src/sonara/history.py src/sonara/daemon.py tests/
git commit -m "feat(sessions): catch-up never surfaces a muted session"
```

---

### Task 6: Web API - sessions in state, /api/session mutations

**Files:**
- Modify: `src/sonara/sessions.py` (add `ids()`)
- Modify: `src/sonara/webui.py` (`SettingsServer.state`, new `_sessions` helper, POST routing, `_handle_session`)
- Test: extend `tests/test_webui.py`

**Interfaces:**
- Consumes: daemon attributes `sessions`, `session_prefs`, `router` (Tasks 1-3); message types from Task 2.
- Produces: state key `"sessions"`: list of `{id, folder, name, muted, voice, foreground, pending}`; `POST /api/session` with `{id, key, value}` or `{id, op: "forget"}`.

- [ ] **Step 1: Write the failing tests** (follow `tests/test_webui.py`'s existing server/daemon-stub fixtures; the file already stands up a `SettingsServer` against a fake daemon, reuse that fixture and extend the fake daemon with `session_prefs`/`router`/`sessions` attributes as needed)

```python
def test_state_includes_sessions(webui_client):        # reuse the file's fixture names
    state = webui_client.get_json("/api/state")
    assert isinstance(state["sessions"], list)


def test_api_session_sets_pref(webui_client, fake_daemon):
    r = webui_client.post_json("/api/session",
                               {"id": "s1", "key": "name", "value": "alpha"})
    assert r.status == 200
    assert fake_daemon.last_message["type"] == "set_session_pref"
    assert fake_daemon.last_message["session"] == "s1"


def test_api_session_rejects_unknown_key(webui_client):
    r = webui_client.post_json("/api/session",
                               {"id": "s1", "key": "rate", "value": 1})
    assert r.status == 400


def test_api_session_forget_refuses_foreground(webui_client, fake_daemon):
    fake_daemon.sessions.set_foreground("s1")
    r = webui_client.post_json("/api/session", {"id": "s1", "op": "forget"})
    assert r.status == 400
```

Adapt names/fixtures to the file's actual idiom (it may use raw `http.client` requests; mirror the closest existing `/api/set` test one-for-one).

- [ ] **Step 2: Run to verify failure**

Run: `python -m pytest tests/test_webui.py -q`
Expected: new tests FAIL (`KeyError: 'sessions'`, 404 on `/api/session`)

- [ ] **Step 3: Implement**

`sessions.py`, after `folder()`:

```python
    def ids(self) -> "list[str]":
        """Known session ids, insertion-ordered (live plus persisted)."""
        return list(self._sessions)
```

`webui.py` `SettingsServer`:

```python
    def _sessions(self) -> list:
        d = self._daemon
        fg = d.sessions.foreground()
        out = []
        for sid in d.sessions.ids():
            ch = d.router.channels.get(sid)
            prefs = d.session_prefs
            out.append({
                "id": sid,
                "folder": d.sessions.folder(sid),
                "name": prefs.name(sid),
                "muted": prefs.muted(sid),
                "voice": prefs.voice(sid),
                "foreground": sid == fg,
                "pending": ch.pending() if ch is not None else 0,
            })
        return out
```

`state()`: add `"sessions": self._sessions(),` to the returned dict.

`do_POST` routing, before the 404:

```python
            if path == "/api/session":
                return self._handle_session(payload)
```

Handler, next to `_handle_set`:

```python
        def _handle_session(self, payload):
            sid = payload.get("id")
            if not isinstance(sid, str) or not sid:
                return self._json(400, {"error": "missing session id"})
            if payload.get("op") == "forget":
                if server._daemon.sessions.is_foreground(sid):
                    return self._json(400, {"error": "cannot forget the active session"})
                _dispatch(server._daemon,
                          {"v": 1, "type": "forget_session", "session": sid})
                return self._json(200, server.state())
            key = payload.get("key")
            if key not in ("name", "muted", "voice"):
                return self._json(400, {"error": f"unknown key {key!r}"})
            _dispatch(server._daemon,
                      {"v": 1, "type": "set_session_pref", "session": sid,
                       "key": key, "value": payload.get("value")})
            return self._json(200, server.state())
```

If the webui test fake daemon lacks `router`/`session_prefs`, keep `_sessions()` defensive is NOT the way (tests should model the real daemon): extend the fake instead.

- [ ] **Step 4: Run to verify pass**

Run: `python -m pytest tests/test_webui.py tests/test_sessions.py -q`
Expected: all pass

- [ ] **Step 5: Commit**

```bash
git add src/sonara/sessions.py src/sonara/webui.py tests/test_webui.py
git commit -m "feat(sessions): sessions in /api/state, /api/session mutations"
```

---

### Task 7: Settings page Sessions tab

**Files:**
- Modify: `src/sonara/settings.html` (nav, page section, CSS, JS render + events)

**Interfaces:**
- Consumes: `/api/state`'s `sessions` array and `voices` groups (Task 6); the page's existing `POST`, `acceptState`, `showApiError`, `render` helpers.

- [ ] **Step 1: Add the nav entry** after the Audio button:

```html
        <button data-page="sessions"><span class="side-icon"><svg viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="5" rx="1.5"/><rect x="3" y="11" width="18" height="5" rx="1.5"/><path d="M3 20h12"/></svg></span>Sessions</button>
```

and a sidebar icon color next to the existing `[data-page=...]` rules:

```css
    [data-page="sessions"] .side-icon { background:#4f8fd0; }
```

- [ ] **Step 2: Add the page section** after the `audio` section:

```html
      <section class="page" id="sessions">
        <div class="title-row"><h1>Sessions</h1><p>Name, mute, or re-voice each Claude Code session.</p></div>
        <div class="card">
          <div id="session-rows"></div>
          <div class="hint" id="sessions-empty" style="display:none;padding:14px 0">No sessions yet. Start a Claude Code session and it will appear here.</div>
          <div class="hint" style="padding-top:10px">Muted sessions stay silent (their attention beeps still play). A custom name is what Sonara says on "Session changed". Voice overrides apply only to that session.</div>
        </div>
      </section>
```

- [ ] **Step 3: Add row CSS** (near the `.pref` styles, reusing tokens):

```css
    .sess-row { display:flex; align-items:center; gap:14px; padding:13px 0; border-bottom:1px solid var(--line); }
    .sess-row:last-child { border-bottom:0; }
    .sess-id { flex:1 1 auto; min-width:0; }
    .sess-id input { width:100%; max-width:260px; }
    .sess-sub { color:var(--secondary); font-size:11px; margin-top:3px; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
    .sess-badge { display:inline-block; padding:1px 7px; border-radius:99px; background:var(--accent); color:#fff; font-size:10px; margin-left:6px; }
    .sess-row select { max-width:170px; }
    .sess-forget { border:0; background:transparent; color:var(--secondary); font-size:15px; cursor:pointer; }
    .sess-forget:hover { color:#d05050; }
```

- [ ] **Step 4: Add the JS** (near the other render helpers; call `renderSessions(s)` from the main `render(state)` function):

```javascript
async function setSession(id, key, value) {
  const r = await POST("/api/session", {id, key, value});
  if (r.ok) { await acceptState(r); } else { await showApiError(r); }
}
async function forgetSession(id) {
  const r = await POST("/api/session", {id, op: "forget"});
  if (r.ok) { await acceptState(r); } else { await showApiError(r); }
}
function voiceOptions(s, selected) {
  const groups = [["Kokoro", s.voices.kokoro || []],
                  ["Chatterbox", s.voices.chatterbox || []],
                  ["Windows", s.voices.windows || []]];
  let html = '<option value=""' + (selected ? "" : " selected") + '>Default</option>';
  for (const [label, voices] of groups) {
    if (!voices.length) continue;
    html += `<optgroup label="${label}">`;
    for (const v of voices) {
      html += `<option value="${v}"${v === selected ? " selected" : ""}>${v}</option>`;
    }
    html += "</optgroup>";
  }
  return html;
}
let sessionsSig = "";
function renderSessions(s) {
  const rows = document.getElementById("session-rows");
  const sessions = s.sessions || [];
  document.getElementById("sessions-empty").style.display = sessions.length ? "none" : "block";
  // poll-safe: rebuild only when content actually changed, and never while
  // the user is editing a field inside the list (mirrors the #38 select fix)
  const sig = JSON.stringify(sessions) + "|" + JSON.stringify(s.voices);
  if (sig === sessionsSig) return;
  if (rows.contains(document.activeElement) && document.activeElement !== rows) return;
  sessionsSig = sig;
  rows.textContent = "";
  for (const sess of sessions) {
    const row = document.createElement("div");
    row.className = "sess-row";
    const shortId = sess.id.slice(0, 8);
    const sub = (sess.folder || "unknown folder") + " · " + shortId;
    row.innerHTML =
      '<div class="sess-id">' +
        '<input type="text" maxlength="60" placeholder="' + (sess.folder || "Name this session") + '">' +
        '<div class="sess-sub"></div>' +
      '</div>' +
      '<label class="switch"><input type="checkbox" class="sess-mute"><span></span></label>' +
      '<select class="sess-voice"></select>' +
      '<button class="sess-forget" title="Forget this session">&times;</button>';
    const nameInput = row.querySelector("input[type=text]");
    nameInput.value = sess.name || "";
    const subEl = row.querySelector(".sess-sub");
    subEl.textContent = sub;
    if (sess.foreground) {
      const b = document.createElement("span");
      b.className = "sess-badge"; b.textContent = "Active";
      subEl.appendChild(b);
    }
    if (sess.pending > 0) {
      const b = document.createElement("span");
      b.className = "sess-badge"; b.style.background = "var(--subtle)";
      b.style.color = "var(--secondary)";
      b.textContent = sess.pending + " queued";
      subEl.appendChild(b);
    }
    nameInput.addEventListener("change",
      () => setSession(sess.id, "name", nameInput.value.trim()));
    const mute = row.querySelector(".sess-mute");
    mute.checked = !!sess.muted;
    mute.addEventListener("change", () => setSession(sess.id, "muted", mute.checked));
    const voiceSel = row.querySelector(".sess-voice");
    voiceSel.innerHTML = voiceOptions(s, sess.voice);
    voiceSel.addEventListener("change",
      () => setSession(sess.id, "voice", voiceSel.value));
    const forget = row.querySelector(".sess-forget");
    if (sess.foreground) forget.style.display = "none";
    forget.addEventListener("click", () => forgetSession(sess.id));
    rows.appendChild(row);
  }
}
```

Wire-in: add `renderSessions(s);` inside the existing `render(state)` function body. If the page has no `.switch` toggle CSS class, reuse whatever toggle markup the Audio page's old duck switch used (inspect the existing toggles and copy that exact markup/class instead of `.switch`).

- [ ] **Step 5: Verify by hand**

Run: `python -m pytest tests/ -q -k "settings or webui"` (guards any HTML-referencing tests)
Then deploy to the live daemon (shutdown, robocopy `src/sonara` to `~/.sonara/app/sonara` /MIR, start) and open `sonara settings`: the Sessions tab lists real sessions; rename one and trigger a session change to hear the custom name; mute one and confirm silence; give one a distinct voice and confirm.

- [ ] **Step 6: Commit**

```bash
git add src/sonara/settings.html
git commit -m "feat(sessions): Sessions tab in the settings page"
```

---

### Task 8: README note

**Files:**
- Modify: `README.md` (features section)

- [ ] **Step 1: Add a short feature bullet** describing the Sessions tab (name, mute, per-session voice), matching the README's existing bullet style.

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "docs: session manager feature note"
```

---

## Final

Full suite (`python -m pytest tests/ -q`, expect only the baseline failures), whole-branch review, deploy via the runbook, live verification with the user, then PR referencing the issue.
