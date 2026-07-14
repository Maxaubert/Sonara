# Sonari M3 - Windows Hotkeys Acceptance Checklist

> **Purpose:** The mock suite proves the *register/dispatch/collision* logic and the keymap
> resolution, but it cannot prove that a chord actually fires system-wide, that `GetMessage`
> pumps `WM_HOTKEY`, or how UIPI behaves. Every ⚠ item must be worked through by a human on a
> real Windows 10/11 box with audio.
>
> **Pre-reqs:** Sonari installed (`sonari install`), the daemon running, speech working (M2),
> a non-elevated terminal/Claude window.

---

## 1. ⚠ All nine actions fire (default chord `Ctrl+Shift+Alt+<key>`)

With a **non-elevated** Claude session focused and Sonari speaking, press each chord and confirm
the effect:

| Chord | Action | Expected |
|---|---|---|
| Ctrl+Shift+Alt+S | stop | speech stops now, queue clears |
| Ctrl+Shift+Alt+R | repeat | last item re-spoken |
| Ctrl+Shift+Alt+. | skip | current item skipped |
| Ctrl+Shift+Alt+D | jump_decision | jumps to the pending decision |
| Ctrl+Shift+Alt+L | catch_up | replays everything unheard |
| Ctrl+Shift+Alt+] | faster | "Rate NNN." higher |
| Ctrl+Shift+Alt+[ | slower | "Rate NNN." lower |
| Ctrl+Shift+Alt+V | cycle_verbosity | "Verbosity everything/medium/quiet." |
| Ctrl+Shift+Alt+O | reread_options | current prompt's options re-read |

## 2. ⚠ Mid-speech interrupt

While a long response is being spoken, press **Ctrl+Shift+Alt+S** - audio must cut within ~100ms
and the daemon stays alive (`tasklist | findstr python` still shows it).

## 3. ⚠ Rebinding

Edit `%USERPROFILE%\.sonari\keymap.json` (e.g. change `stop` to `{"key":"q","mods":["ctrl","alt"]}`),
restart the daemon, and confirm the new chord fires and the old one no longer does.
`sonari keymap` prints the active bindings.

## 4. ⚠ Collision detection

Pre-register `Ctrl+Shift+Alt+S` in another app (AutoHotkey, PowerToys, or Terminal globalSummon),
then start Sonari. Confirm:
- `sonari doctor` shows `hotkey chords: FAIL … already owned … for: stop`.
- The **other** chords still fire (only the colliding one is lost).
- `RegisterHotKey` returned FALSE with `GetLastError()==1409` (the row's wording).

## 5. ⚠ UIPI / elevation gap (the #1 landmine)

Run Claude Code **as Administrator** (daemon at normal integrity):
- The chords do **not** reach the elevated window (WM_HOTKEY is blocked by UIPI).
- Speech still works.
- If the daemon itself is elevated, `sonari doctor` shows `hotkey integrity: FAIL … Don't run as
  Administrator (UIPI).`

Run Claude Code **non-elevated** → chords work. Document "don't run elevated for hotkeys."

## 6. ⚠ Secure desktop (transient)

During a UAC prompt / Ctrl+Alt+Del / Windows Hello, hotkeys + TTS are silent; they resume after.
Transient and unavoidable from user space - document, not a blocker.

---

## Notes / mock-blind boundary

- The unit suite (`tests/test_win_hotkeys.py`) covers: `_register_all` maps ids→messages and adds
  `MOD_NOREPEAT`; a `1409` failure records a collision (`already_owned=True`); `_on_hotkey`
  dispatches the right message and ignores unknown ids; `doctor_rows` reports collisions + the
  elevation row; `display_combo` labels. The **real** `RegisterHotKey`/`GetMessage`/`WM_HOTKEY`
  path is exercised only here, on hardware.
- `keymap.resolve_keymap` → VK codes + `Ctrl+Shift+Alt` mask is unit-verified
  (`tests/test_keymap.py::test_resolve_windows_vk_codes`).

## Sign-off

| Item | Tester | Date | Result | Notes |
|------|--------|------|--------|-------|
| 1. Nine actions fire | | | | |
| 2. Mid-speech interrupt | | | | |
| 3. Rebinding | | | | |
| 4. Collision detection | | | | |
| 5. UIPI / elevation | | | | |
| 6. Secure desktop | | | | |
