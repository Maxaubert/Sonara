# Sonari Phase 2.1 — Eyes-free prompt interaction — Spec

> **projectType:** claude-plugin
> Status: draft from the brainstorm (live interview with Nima, 2026-06-09); consumed by /stack → /issues.
> Scope: a **minor update** to the existing Sonari plugin (`~/projects/private/claude-tts`).
>
> **Update (2026-06-10) — shipped EXCEPT caret tracking.** Built + deployed: the history substrate, `repeat`,
> `catch_up`, `reread_options` (incl. reading the permission `message` field), and voice continuity. The
> **caret tracking** capability in §2 (arrow-key option navigation + the virtual **Submit**) was built and
> verified working, then **DE-SCOPED**: a macOS `CGEventTap` needs Secure Keyboard Entry OFF *and* an
> Input-Monitoring grant that resets on every (unsigned) rebuild — not shippable to other blind/low-vision
> users. The eyes-free **multi-select Submit** gap it targeted therefore remains OPEN (realistically needs
> host support from Claude Code to expose the selection).

## 1. Core
- **Problem:** Sonari's Phase 2 eyes-free controls fail exactly when an eyes-free / low-vision user needs them most — during Claude's prompts and after interruptions. Three "re-speak" hotkeys (`reread_options`, `repeat`, `catch_up`) read only the live TTS **queue tail** — usually a one-sentence fragment — instead of the meaningful content; `catch_up` doesn't work at all; multi-select **Submit** can't be reached without sighted arrow-navigation; and a focus-bound voice loses everything a non-focused session says.
- **Target user:** Nima and other eyes-free / low-vision Claude Code users — people driving Claude by ear, who can't fall back on reading the screen to recover options, repeats, or missed output.
- **MVP scope:**
  - **In v1 (this minor update):** (1) fix `reread_options`; (2) caret tracking on arrow-nav inside known prompts (incl. a virtual Submit); (3) fix `repeat` (whole last message); (4) fix `catch_up` (per-session backlog replay); (5) the shared **substrate** — per-session rolling narration history + sentence-granular heard-marker, with silent capture of non-voice-owning sessions; (6) the **voice-continuity** rule.
  - **Out (later):** Tier-2 narration of native Claude Code menus (`/config` etc. — host-owned); a *summarized* catch_up; any new hotkey bindings beyond the existing keymap; auto-start of a session's backlog when the voice frees.
- **Definition of Done** (each testable; **⚠ = audible/behavioral → human listen-test, not headless**):
  - ⚠ `reread_options` speaks the current prompt's options from the dedicated slot even after other speech has occurred (permission → number+label; question → label+description+mode announce); with no active prompt it says *"No options right now,"* never the queue tail.
  - ⚠ With a multi-select open, arrow ↑/↓ speaks the focused option, and arrowing past the last option speaks *"Submit"*; pressing Enter there submits — with no sighted navigation.
  - ⚠ `repeat` speaks the focused session's **entire last message** (all sentences), not the last fragment; *"Nothing to repeat"* when empty.
  - ⚠ `catch_up` speaks the focused session's unheard backlog oldest→newest then marks it heard; *"You're all caught up"* when empty; a sentence interrupted by `stop` replays **from its start**.
  - ⚠ Switching focus away from a speaking session does **not** stop speech; only `stop` does.
  - ⚠ A response produced for a non-voice-owning session is **not** spoken live but **is** retrievable via `repeat`/`catch_up` on return.
  - Per-session history is **bounded** (rolling cap) — verifiable by inspection/log (not audible).
- **Hard constraints:**
  - **Runtime performance (HARD):** hotkeys feel instant; **no model/LLM call on the hotkey path** (catch_up is verbatim, never a generated summary); per-session history bounded to cap memory.
  - **One voice only** — never two sessions speaking at once.
  - macOS; relies on the existing Accessibility-API hotkey daemon (`sonari-hotkeyd`) + the speech daemon.

## 2. Behaviors

### Substrate — per-session narration history + heard-marker
- **Inputs:** narration utterances from each Claude session (via hooks, keyed by a session id); `stop`/`skip` events; focus / voice-ownership changes.
- **Outputs:** an ordered, **bounded (rolling cap)** history per session + a **"heard-up-to-here" marker** per session.
- **States:** per session — voice-owner vs not; per item — heard / unheard / partially-heard.
- **Edge cases:**
  - A sentence interrupted by `stop`/`skip` is **unheard**; the marker rewinds to **that sentence's start** (sentence-granular — never resume on the back half of a thought).
  - A **new user prompt** in a session **resets that session's backlog** to the new response.
  - History exceeds the cap → oldest entries drop (rolling).

### `reread_options` (Ctrl+Cmd+O) — re-speak the live option set
- **Inputs:** hotkey; the dedicated "current options" slot (structured option data captured when the prompt appeared).
- **Outputs:** spoken options, context-adaptive.
- **States:** permission prompt · single-choice question · multi-select question · no active options.
- **Behavior:** permission → each option with its number/key. Question → each option's **label + description**, prefaced by the **selection mode** (*"This is a multi-select — pick more than one"* vs single). No active options → *"No options right now"* (never the queue tail).
- **Edges:** option with no description → read the label only; long multi-select → read all, in order.

### Caret tracking (arrow ↑/↓ inside a known prompt)
- **Inputs:** arrow up/down while a known prompt (multi-select/question) is open; the held option list.
- **Outputs:** speak the focused item as the cursor moves; **"Submit"** announced as a virtual item past the last option.
- **States:** cursor on option *i* · cursor on virtual Submit · no known prompt open (inert).
- **Behavior:** makes option navigation **and** submission fully eyes-free; arrow onto Submit → *"Submit"* → Enter submits the current selection.
- **Edges / risk:** the mirrored cursor can **desync** from the TUI's real highlight (page-scroll, Home/End, reorder) → needs a **re-sync anchor** (snap to known top on each fresh prompt). Out of scope: Tier-2 narration of native menus where Sonari holds no structured list.

### `repeat` (Ctrl+Cmd+R) — re-speak the last message
- **Inputs:** hotkey; the focused session's history.
- **Outputs:** the focused session's **entire last message** (all sentences, from the first).
- **States:** has-history · nothing-spoken-yet.
- **Edges:** nothing yet → *"Nothing to repeat"*; a message interrupted mid-sentence repeats from that sentence's start.

### `catch_up` (Ctrl+Cmd+L) — replay the backlog
- **Inputs:** hotkey; the focused session's unheard backlog (marker→now).
- **Outputs:** speak marker→now, **oldest→newest, verbatim**; then advance the marker.
- **States:** backlog-present · all-caught-up.
- **Behavior:** the verb that earns its keep **after a `stop`** or **after time in another window** — it replays what the live queue can no longer give you. Distinct from `repeat` (last message only).
- **Edges:** nothing missed → *"You're all caught up"*; `stop` counts as not-heard (its content is in the backlog); an interrupted sentence replays from its start; a new prompt resets the backlog; acts on the **focused** session.

### Voice continuity + multi-session ownership
- **Inputs:** response-start events per session; focus changes; `stop`/`skip`.
- **Outputs:** exactly one active voice at a time.
- **Behavior:** when a session starts speaking it **owns the single voice and runs to completion**; **focus changes never stop it** — only `stop` (clears) and `skip` (one utterance) interrupt. A session that isn't the voice-owner — because it's unfocused, or the voice was busy when its response landed — is **captured silently** to its own history and retrieved later via `repeat`/`catch_up`.
- **Edges:** voice frees while another session has unheard output → **stay silent (no auto-start)**; the user pulls it with `catch_up`. Never two voices at once.

## 3. Plugin surface · Config · Manifests (claude-plugin type-specific)

### Plugin surface
- **Hooks (intake):** Sonari consumes Claude Code hook events for (a) narration text, (b) **structured prompt options** (permission + question), and (c) a **stable session id** to key per-session history. No new hook events are *required* of the host; if the host emits no intra-prompt navigation event, caret tracking is driven by the hotkey daemon observing arrow keys (below).
- **Hotkey daemon (`sonari-hotkeyd`):** existing global bindings keep their keys — `reread_options` (Ctrl+Cmd+O), `repeat` (Ctrl+Cmd+R), `catch_up` (Ctrl+Cmd+L), `stop` (Ctrl+Cmd+S), `skip` (Ctrl+Cmd+.). **New:** while a known prompt is open, hotkeyd **observes arrow ↑/↓** to drive caret tracking (mirroring, not replacing, the TUI's own navigation). **No new bindings** are added (scope decision).
- **Output contract:** all behaviors emit to the speech daemon; none block the host's critical path (runtime-perf constraint).

### Config
- **Refocus cue (optional toggle):** default **silent on refocus**; optional *"N new here"* announcement when returning to a session that spoke while it wasn't the voice-owner.
- **History cap:** rolling per-session history size (sensible default).

### Manifests
- No new `.claude-plugin/plugin.json` or `marketplace.json` surfaces. The change is internal to the daemons + hook handlers; the keymap gains arrow-observation behavior (conditional on a known prompt being open), not a new bound action.

## 4. Open assumptions (→ resolved by the build/technical agents)
- The exact **session-identity + "frontmost window → session" mapping** mechanism (Nima: delegate to technical agents; Sonari already has partial multi-session support).
- That **arrow-key observation coexists** with the TUI's own navigation without breaking normal input (mirror, don't intercept-and-swallow, unless required).
- That hook payloads reliably carry **structured options**, **narration text**, and a **stable session id**.

## 5. Risk flags (advisory)
- **Caret-cursor desync** — the primary correctness risk; mitigate with a re-sync anchor per fresh prompt.
- **Arrow observation** could interfere with terminal input if not tightly scoped to "a known prompt is open."
- **Verification is largely human/audible.** These are TTS / hotkey / focus behaviors — **not headlessly verifiable.** The build must **escalate to a human listen-test** (Nima) for the ⚠ DoD items rather than assert a green check from a blind unit test. *(This is the engine's "escalate the unverifiable" principle and the claude-plugin playbook's verification note in action — the front door surfaced it correctly.)*
