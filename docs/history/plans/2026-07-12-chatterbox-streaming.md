# Chatterbox Streaming + Responsiveness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Chatterbox playback interruptible (within one chunk) and gapless via a pipelined chunk-streaming handle, add per-chunk Kokoro fallback, fix two pause correctness bugs, and stop the transient VRAM-gate miss from announcing.

**Architecture:** `tts.run` returns a `_ChatterboxHandle` BEFORE synthesizing, so the speaker sets it as `_current` immediately (closing the synth-gap). `handle.wait()` runs a producer thread that synthesizes chunks ~1-2 ahead into a bounded queue while the consumer plays them in order; `handle.terminate()` aborts within one chunk. A `synth_one(chunk)` closure tries the chatterbox worker and falls back to Kokoro per chunk. The gate/provisioned decision stays up front in `run()`; a gate-miss uses whole-utterance Kokoro quietly (no notice).

**Tech Stack:** Python 3.9+ stdlib (`threading`, `queue`); pytest with injected synth/play seams (no torch, no GPU in CI). One manual live smoke after deploy.

## Global Constraints

- Daemon and `src/sonara/` stay stdlib-only, Python 3.9+ (`from __future__ import annotations`); torch/chatterbox only in the worker venv.
- Never silent: `synth_one` falls back to Kokoro per chunk on a `ChatterboxError`; a gate-miss falls back to whole-utterance Kokoro. Real failures arm the once-per-run notice; gate-misses do NOT (log only).
- The handle owns exactly one producer thread; it must be stopped/joined on both normal completion and `terminate()` (no leaked threads). The abort `threading.Event` is the single source of truth.
- `chatterbox_timeout` default becomes 30 (now a per-chunk worker timeout; a chunk is a few seconds).
- No em-dashes in code or docs. Speech rate does not apply to chatterbox voices.
- Run tests: `./.venv/Scripts/python.exe -m pytest <files> -q` from repo root. The full suite has 20 PRE-EXISTING env-only failures (test_win_tts, test_winfakes, test_transport, test_paths, test_win_autostart, test_bin_sonara, test_daemon_ducking, and one py39-compat env quirk) unrelated to this work; add no new failures.

---

### Task 1: `split_text` helper + `_ChatterboxHandle` (pipelined, interruptible)

**Files:**
- Modify: `src/sonara/chatterbox.py` (add a public `split_text`)
- Create: `_ChatterboxHandle` in `src/sonara/platform/windows/tts.py`
- Test: `tests/test_chatterbox_handle.py` (new; all seams injected)

**Interfaces:**
- Produces: `chatterbox.split_text(text, max_chars=280) -> list[str]` (canonical daemon-side splitter; the worker keeps its own `_split_text` as a defensive net). `_ChatterboxHandle(text, synth_one, on_play=None, play=None, split=None, chunk_play_timeout=60)` with `.wait(timeout=None) -> int`, `.terminate()`, `.poll()`, `.returncode` (0 complete, 1 aborted). `synth_one(chunk_text) -> bytes|None` (None = produced nothing, skip). Consumed by Task 2's `run()`.

- [ ] **Step 1: Write the failing tests**

Create `tests/test_chatterbox_handle.py`:

```python
"""Pipelined chatterbox playback handle. All seams injected: no torch, no GPU,
no winsound. A fake synth_one returns marker bytes; a fake play returns a fake
sub-handle recording wait/terminate."""
import threading
import time

from sonara.platform.windows.tts import _ChatterboxHandle


class FakeSub:
    def __init__(self, wav):
        self.wav = wav
        self.returncode = None
        self.terminated = False
        self._done = threading.Event()

    def wait(self, timeout=None):
        # "plays" instantly in tests
        self.returncode = 0
        self._done.set()
        return 0

    def terminate(self):
        self.terminated = True
        self.returncode = 1
        self._done.set()


def _recording_play():
    played = []

    def play(wav):
        sub = FakeSub(wav)
        played.append(sub)
        return sub
    return play, played


def _split(_text, max_chars=280):
    return ["c1", "c2", "c3"]


def test_all_chunks_play_in_order_and_return_complete():
    play, played = _recording_play()
    synthed = []
    h = _ChatterboxHandle("whatever", synth_one=lambda c: (synthed.append(c) or c.encode()),
                          play=play, split=_split)
    rc = h.wait()
    assert rc == 0 and h.returncode == 0
    assert [s.wav for s in played] == [b"c1", b"c2", b"c3"]
    assert synthed == ["c1", "c2", "c3"]


def test_on_play_fires_once_before_first_playback():
    play, played = _recording_play()
    calls = []
    h = _ChatterboxHandle("x", synth_one=lambda c: c.encode(),
                          on_play=lambda: calls.append(1), play=play, split=_split)
    h.wait()
    assert calls == [1]                       # exactly once


def test_terminate_aborts_within_one_chunk():
    # A slow synth_one lets us terminate mid-stream; remaining chunks must not
    # play. synth blocks until released so the abort lands between chunks.
    play, played = _recording_play()
    gate = threading.Event()
    synth_count = {"n": 0}

    def slow_synth(c):
        synth_count["n"] += 1
        if synth_count["n"] == 1:
            return c.encode()                 # first chunk synths fast
        gate.wait(2.0)                         # later chunks stall until released
        return c.encode()

    h = _ChatterboxHandle("x", synth_one=slow_synth, play=play, split=_split)
    t = threading.Thread(target=h.wait)
    t.start()
    time.sleep(0.2)
    h.terminate()
    gate.set()
    t.join(3.0)
    assert not t.is_alive()
    assert h.returncode == 1
    assert len(played) <= 2                    # did not play all three


def test_none_from_synth_one_skips_that_chunk():
    play, played = _recording_play()
    h = _ChatterboxHandle("x", synth_one=lambda c: None if c == "c2" else c.encode(),
                          play=play, split=_split)
    h.wait()
    assert [s.wav for s in played] == [b"c1", b"c3"]   # c2 produced nothing, skipped


def test_empty_text_is_a_clean_noop():
    play, played = _recording_play()
    h = _ChatterboxHandle("", synth_one=lambda c: c.encode(),
                          play=play, split=lambda t, max_chars=280: [])
    assert h.wait() == 0 and played == []


def test_producer_thread_stops_after_wait():
    play, _ = _recording_play()
    h = _ChatterboxHandle("x", synth_one=lambda c: c.encode(), play=play, split=_split)
    h.wait()
    time.sleep(0.1)
    assert h._producer is None or not h._producer.is_alive()   # no leaked thread


def test_split_text_is_exposed_on_chatterbox_module():
    from sonara import chatterbox
    chunks = chatterbox.split_text("One. Two. Three.", max_chars=8)
    assert len(chunks) >= 2 and all(len(c) <= 8 for c in chunks)
```

- [ ] **Step 2: Run to verify it fails**

Run: `./.venv/Scripts/python.exe -m pytest tests/test_chatterbox_handle.py -q`
Expected: FAIL - `_ChatterboxHandle` and `chatterbox.split_text` do not exist.

- [ ] **Step 3: Add `split_text` to `chatterbox.py`**

In `src/sonara/chatterbox.py`, add (near the registry helpers) a public splitter (the daemon-side canonical copy; the worker's `_split_text` stays as a defensive net, and a comment notes the intentional duplication to avoid the worker importing sonara):

```python
def split_text(text, max_chars=280):
    """Split *text* into speakable chunks no longer than *max_chars*, on sentence
    boundaries (a too-long sentence is hard-split on spaces). Chatterbox degrades
    on long input, so the daemon drives chunking for pipelined, interruptible
    playback. NOTE: chatterbox_worker.py keeps its own _split_text as a defensive
    net; the worker cannot import sonara, so the small pure logic is duplicated on
    purpose. Keep the two in sync."""
    import re
    text = (text or "").strip()
    if not text:
        return []
    sentences = re.findall(r"[^.!?]*[.!?]+|\S[^.!?]*$", text)
    chunks = []
    cur = ""
    for s in sentences:
        s = s.strip()
        if not s:
            continue
        if len(s) > max_chars:
            if cur:
                chunks.append(cur)
                cur = ""
            buf = ""
            for word in s.split(" "):
                if buf and len(buf) + 1 + len(word) > max_chars:
                    chunks.append(buf)
                    buf = word
                else:
                    buf = (buf + " " + word).strip()
            if buf:
                cur = buf
        elif cur and len(cur) + 1 + len(s) > max_chars:
            chunks.append(cur)
            cur = s
        else:
            cur = (cur + " " + s).strip()
    if cur:
        chunks.append(cur)
    return chunks
```

- [ ] **Step 4: Add `_ChatterboxHandle` to `tts.py`**

In `src/sonara/platform/windows/tts.py`, after `_play_wav_bytes`, add:

```python
class _ChatterboxHandle:
    """Pipelined, interruptible playback of a chatterbox utterance. tts.run
    returns this BEFORE any synthesis, so the speaker sets it as _current at
    once (a cancel mid-synth then reaches terminate(), closing the synth-gap).
    wait() runs a producer thread that synthesizes chunks ~1-2 ahead into a
    bounded queue while the consumer plays them in order; terminate() aborts
    within one chunk. Fits the say_runner handle contract
    (wait/terminate/poll/returncode)."""

    def __init__(self, text, synth_one, on_play=None, play=None, split=None,
                 chunk_play_timeout=60):
        from sonara import chatterbox
        self._chunks = (split or chatterbox.split_text)(text)
        self._synth_one = synth_one
        self._on_play = on_play
        self._play = play or _play_wav_bytes
        self._chunk_play_timeout = chunk_play_timeout
        self._abort = threading.Event()
        import queue as _queue
        self._queue_mod = _queue
        self._q = _queue.Queue(maxsize=2)      # synth at most ~2 chunks ahead
        self._producer = None
        self._cur_sub = None
        self.returncode = None

    def _produce(self):
        for chunk in self._chunks:
            if self._abort.is_set():
                break
            try:
                wav = self._synth_one(chunk)
            except Exception:  # noqa: BLE001 - synth_one owns its fallback; guard anyway
                wav = None
            # non-blocking put so an aborted consumer never wedges the producer
            while not self._abort.is_set():
                try:
                    self._q.put(wav, timeout=0.1)
                    break
                except self._queue_mod.Full:
                    continue
            if self._abort.is_set():
                break
        try:
            self._q.put(None, timeout=0.1)     # sentinel
        except self._queue_mod.Full:
            pass

    def wait(self, timeout=None):
        if not self._chunks:
            self.returncode = 0
            return 0
        self._producer = threading.Thread(target=self._produce,
                                          name="sonara-cb-synth", daemon=True)
        self._producer.start()
        played_any = False
        rc = 1
        while True:
            if self._abort.is_set():
                break
            try:
                wav = self._q.get(timeout=0.2)
            except self._queue_mod.Empty:
                continue
            if wav is None:                    # producer finished all chunks
                rc = 0 if not self._abort.is_set() else 1
                break
            if self._abort.is_set():
                break
            if wav == b"" or wav is None:
                continue
            if not played_any and self._on_play is not None:
                try:
                    self._on_play()
                except Exception:  # noqa: BLE001 - ducking must never block speech
                    pass
                played_any = True
            sub = self._play(wav)
            self._cur_sub = sub
            try:
                sub.wait(timeout=self._chunk_play_timeout)
            except Exception:  # noqa: BLE001 - a stuck chunk must not wedge the loop
                try:
                    sub.terminate()
                except Exception:  # noqa: BLE001
                    pass
            self._cur_sub = None
            if self._abort.is_set():
                break
        self.returncode = rc
        self._abort.set()                      # release the producer
        # drain so a producer blocked on put() unblocks, then join it
        try:
            while True:
                self._q.get_nowait()
        except self._queue_mod.Empty:
            pass
        if self._producer is not None:
            self._producer.join(timeout=1.0)
            self._producer = None
        return self.returncode

    def terminate(self):
        self._abort.set()
        sub = self._cur_sub
        if sub is not None:
            try:
                sub.terminate()
            except Exception:  # noqa: BLE001
                pass

    def poll(self):
        return self.returncode
```

Note: `test_none_from_synth_one_skips_that_chunk` requires that a `None` wav is skipped (not played). The `wav == b""` guard plus the `wav is None` sentinel handling: a `None` from synth_one is pushed to the queue as `None`, which the consumer currently treats as the sentinel. Fix the ambiguity in your implementation - use a distinct sentinel object for "done" (e.g. `_DONE = object()`) so a `None` payload (skip-this-chunk) is unambiguous. Adjust `_produce`/`wait` to push/compare `_DONE` for completion and treat a `None` payload as "skip". Make the tests pass with that unambiguous design.

- [ ] **Step 5: Run to verify pass**

Run: `./.venv/Scripts/python.exe -m pytest tests/test_chatterbox_handle.py -q`
Expected: PASS (7).

- [ ] **Step 6: Commit**

```bash
git add src/sonara/chatterbox.py src/sonara/platform/windows/tts.py tests/test_chatterbox_handle.py
git commit -m "feat(chatterbox): pipelined interruptible streaming playback handle"
```

---

### Task 2: Wire `run()` to the handle + per-chunk fallback + gate-miss split + timeout

**Files:**
- Modify: `src/sonara/platform/windows/tts.py` - `run()` chatterbox branch, replace `_chatterbox_or_fallback` with a per-chunk `synth_one` builder + up-front gate decision.
- Modify: `src/sonara/config.py` (`chatterbox_timeout` default 30)
- Test: `tests/test_chatterbox_routing.py` (extend), `tests/test_config.py` (extend)

**Interfaces:**
- Consumes: `_ChatterboxHandle` (Task 1), `chatterbox.is_provisioned/gate_ok/CLIENT.synth_wav/ChatterboxError/_set_fallback_notice`, `kokoro`.
- Produces: chatterbox voice -> `run()` returns a `_ChatterboxHandle` whose `synth_one` tries the worker per chunk and falls back to Kokoro per chunk (arming the notice + `[chatterbox]` log on a real failure); a gate-miss or not-provisioned -> whole-utterance Kokoro handle (no streaming) and, for a gate-miss, NO notice (log `[chatterbox] gate: ...` only). on_play flows into the handle.

- [ ] **Step 1: Write the failing tests**

Add to `tests/test_chatterbox_routing.py` (mirror the existing fixture that builds a bare backend and monkeypatches `sonara.chatterbox`):

```python
def test_chatterbox_voice_returns_streaming_handle(...):
    # provisioned + gate ok + is_chatterbox_voice -> run() returns a
    # _ChatterboxHandle (not a plain _TtsHandle); synth NOT started yet.

def test_synth_one_falls_back_to_kokoro_per_chunk(...):
    # CLIENT.synth_wav raises ChatterboxError on one chunk -> that chunk is
    # synthesized via the kokoro path; _set_fallback_notice was armed; a
    # [chatterbox] line logged. Other chunks still use chatterbox.

def test_gate_miss_uses_whole_utterance_kokoro_without_notice(...):
    # gate_ok False -> run() returns a plain kokoro handle (no streaming),
    # pop_fallback_notice() is None (gate-miss does NOT announce), and a
    # [chatterbox] gate line is logged.

def test_not_provisioned_uses_kokoro_with_notice(...):
    # is_provisioned False -> kokoro whole-utterance, notice armed.

def test_on_play_flows_into_streaming_handle(...):
    # the on_play passed to run() is the handle's on_play (fires before first chunk).
```

Write these as real tests using the existing `test_win_tts_kokoro.py` fixture pattern; assert on `type(handle).__name__ == "_ChatterboxHandle"` and drive `handle.wait()` with a fake `_play_wav_bytes` (monkeypatch the module function) + fake `chatterbox.CLIENT`. Add to `tests/test_config.py`:

```python
def test_chatterbox_timeout_default_is_30():
    from sonara.config import DEFAULTS
    assert DEFAULTS["chatterbox_timeout"] == 30
```

- [ ] **Step 2: Run to verify fail**

Run: `./.venv/Scripts/python.exe -m pytest tests/test_chatterbox_routing.py tests/test_config.py -q`
Expected: FAIL.

- [ ] **Step 3: Implement**

In `config.py`, change `"chatterbox_timeout": 120,` to `"chatterbox_timeout": 30,` (per-chunk now; update the inline comment).

In `tts.py`, replace `_chatterbox_or_fallback` and the `run()` chatterbox branch with:

```python
    def _kokoro_wav(self, text, rate):
        from sonara import kokoro
        kokoro.require_installed()
        return self._get_kokoro().wav_bytes(
            text, kokoro.DEFAULT_VOICE, kokoro.rate_to_speed(rate))

    def _chatterbox_synth_one(self, voice, cfg, rate):
        """Return a synth_one(chunk) -> wav bytes closure: try the worker, fall
        back to the Kokoro default voice per chunk on any ChatterboxError so the
        stream never goes silent. A real failure arms the once-per-run notice and
        logs; the caller has already passed the up-front gate."""
        import sys
        from sonara import chatterbox

        def synth_one(chunk):
            try:
                return chatterbox.CLIENT.synth_wav(chunk, voice, cfg)
            except chatterbox.ChatterboxError as exc:
                print("[chatterbox] fallback: {0}".format(exc),
                      file=sys.stderr, flush=True)
                chatterbox._set_fallback_notice(str(exc))
                return self._kokoro_wav(chunk, rate)
        return synth_one
```

and the `run()` branch:

```python
        from sonara import kokoro, chatterbox
        if (not kokoro.is_kokoro_voice(voice)) and chatterbox.is_chatterbox_voice(voice):
            import sys
            from sonara.config import load_config
            cfg = load_config()
            if not chatterbox.is_provisioned():
                print("[chatterbox] fallback: not provisioned", file=sys.stderr, flush=True)
                chatterbox._set_fallback_notice(
                    "not provisioned (run: sonara voices install chatterbox)")
            elif not chatterbox.gate_ok(cfg):
                # A gate-miss is expected/transient (busy GPU). Quietly use Kokoro
                # this utterance; do NOT announce or burn the once-per-run notice.
                print("[chatterbox] gate: VRAM below threshold, using Kokoro",
                      file=sys.stderr, flush=True)
            else:
                return _ChatterboxHandle(
                    text, self._chatterbox_synth_one(voice, cfg, rate),
                    on_play=on_play)
            # fell through: whole-utterance Kokoro (not-provisioned or gate-miss)
            data = self._kokoro_wav(text, rate)
            if on_play is not None:
                try:
                    on_play()
                except Exception:  # noqa: BLE001 - ducking must never block speech
                    pass
            return _play_wav_bytes(data)
```

(Keep the kokoro/native branches below unchanged. Remove the now-unused old `_chatterbox_or_fallback`.)

- [ ] **Step 4: Run to verify pass**

Run: `./.venv/Scripts/python.exe -m pytest tests/test_chatterbox_routing.py tests/test_config.py tests/test_chatterbox_handle.py -q`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/sonara/platform/windows/tts.py src/sonara/config.py tests/test_chatterbox_routing.py tests/test_config.py
git commit -m "feat(chatterbox): run() builds streaming handle; per-chunk + gate-miss fallback"
```

---

### Task 3: Pause correctness fixes (daemon requeue)

**Files:**
- Modify: `src/sonara/daemon.py` - the pause-requeue guard in `_speak_loop_once` (~lines 1303-1312).
- Test: `tests/test_daemon_pause_mute.py` (extend)

**Interfaces:**
- Consumes: nothing new.
- Produces: (1) a paused-mid-utterance item (any engine, including a streaming chatterbox item that returns `completed=False` on abort) is re-queued and re-spoken on resume, not dropped; (2) a paused `session_change` announcement does not rewind a real content item - the announcement is re-armed via the router instead.

- [ ] **Step 1: Write the failing tests**

Add to `tests/test_daemon_pause_mute.py`:

```python
def test_pause_during_session_change_announcement_does_not_rewind_content(monkeypatch):
    # Bug: pausing while a "Session changed" announcement (kind session_change,
    # id 0) is speaking rewound the active session's channel cursor, double-
    # speaking or losing a real content item. It must re-arm the announcement
    # instead and leave content cursors untouched.
    from sonara.queue import SpeechItem
    daemon, queue, speaker, sessions, config = make_daemon(foreground="A")
    ch = daemon.router.channel("A")
    ch.append(SpeechItem(id=5, session="A", kind="prose", text="real content.",
                         is_decision=False))
    ch_cursor_before = ch.cursor
    ann = SpeechItem(id=0, session="A", kind="session_change",
                     text="Session changed: A.", is_decision=False)
    daemon._current_item = ann
    daemon._paused.set()
    # simulate the requeue path for the announcement item with completed=False
    daemon._requeue_or_note(ann, completed=False)   # extract the guard into a helper
    assert daemon.router.channel("A").cursor == ch_cursor_before   # content NOT rewound
    assert daemon.router._pending_announce == "A"                  # announcement re-armed


def test_pause_requeues_normal_item_for_resume(monkeypatch):
    from sonara.queue import SpeechItem
    daemon, queue, speaker, sessions, config = make_daemon(foreground="A")
    ch = daemon.router.channel("A")
    ch.append(SpeechItem(id=7, session="A", kind="prose", text="hi.", is_decision=False))
    ch.cursor = 1                                     # router advanced past the item
    item = ch.items[0]
    daemon._current_item = item
    daemon._paused.set()
    daemon._requeue_or_note(item, completed=False)
    assert ch.cursor == 0                             # rewound so resume re-speaks it
```

Note: the tests call `daemon._requeue_or_note(item, completed)`. Refactor the inline requeue guard into that small method so it is unit-testable; the speak loop calls it in place of the inline block.

- [ ] **Step 2: Run to verify fail**

Run: `./.venv/Scripts/python.exe -m pytest tests/test_daemon_pause_mute.py -q`
Expected: FAIL - `_requeue_or_note` does not exist; announcement rewinds content.

- [ ] **Step 3: Implement**

In `daemon.py`, replace the inline block:

```python
        requeued = False
        with self._lock:
            if not completed and self._paused.is_set():
                # paused mid-utterance: rewind the cursor so resume re-speaks it
                ch = self.router.channels.get(item.session)
                if ch is not None and ch.cursor > 0:
                    ch.cursor -= 1
                self._current_item = None
                requeued = True
        if not requeued:
            self.note_spoken(item, completed)
```

with a call to a new helper:

```python
        if not self._requeue_or_note(item, completed):
            self.note_spoken(item, completed)
```

and add the method (near `note_spoken`):

```python
    def _requeue_or_note(self, item, completed) -> bool:
        """On a pause-interrupted utterance, re-queue it so resume re-speaks it and
        return True (skip note_spoken). Returns False otherwise (caller notes it).
        A session-change announcement owns no channel cursor position (it comes
        from the router's pending-announce, id 0), so re-arm the announcement
        instead of rewinding a real content item (which double-spoke/lost it)."""
        with self._lock:
            if not (not completed and self._paused.is_set()):
                return False
            if item.kind == "session_change":
                self.router._pending_announce = item.session
                self.router._pending_announce_replay = False
            else:
                ch = self.router.channels.get(item.session)
                if ch is not None and ch.cursor > 0:
                    ch.cursor -= 1
            self._current_item = None
            return True
```

- [ ] **Step 4: Run to verify pass + regressions**

Run: `./.venv/Scripts/python.exe -m pytest tests/test_daemon_pause_mute.py tests/test_daemon_session_change.py tests/test_daemon_loop.py -q`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/sonara/daemon.py tests/test_daemon_pause_mute.py
git commit -m "fix(daemon): pause re-arms session-change announcement; requeue helper"
```

---

### Task 4: Docs

**Files:**
- Modify: `README.md` (Chatterbox section)

**Interfaces:** none.

- [ ] **Step 1: Update the Chatterbox section**

In the README Chatterbox section, add/adjust: speech is now synthesized and played in chunks, so hotkeys (mute, nav, pause, skip) take effect within about a chunk (a couple of seconds) rather than waiting for the whole reply; a busy GPU (below the VRAM threshold) quietly uses the Kokoro voice for that utterance with no spoken notice, and returns to Chatterbox automatically when the GPU frees; `chatterbox_timeout` (default 30s) now bounds a single chunk. No em-dashes.

- [ ] **Step 2: Verify + commit**

Run: `git grep -n "chunk" README.md | head`
Expected: the new wording present.

```bash
git add README.md
git commit -m "docs: chatterbox streaming/interruptibility + quiet gate fallback"
```

---

## Self-Review

**Spec coverage:** pipelined interruptible handle with producer/bounded-queue/abort (T1), split_text shared + worker net (T1), run() builds handle + per-chunk kokoro fallback + up-front gate decision + gate-miss-no-notice + timeout 30 (T2), synth-gap closed by returning the handle before synth (T1/T2), pause requeue fix + announcement re-arm (T3), docs incl. quiet gate + interruptibility (T4). Worker unchanged (one chunk per request; its `_split_text` kept as a net) - noted, no task needed. ✓

**Placeholder scan:** Task 1 Step 4 flags one real design point (distinct DONE sentinel vs None-payload) with an explicit instruction and the failing test that pins it - not a placeholder but a directed decision. Everything else is complete code. ✓

**Type consistency:** `_ChatterboxHandle(text, synth_one, on_play, play, split, chunk_play_timeout)` matches T2's construction; `synth_one(chunk)->bytes|None` matches the closure; `split_text` name consistent T1/T2; `_requeue_or_note(item, completed)->bool` consistent T3 code/tests; `chatterbox_timeout` 30 consistent T2/config. ✓
