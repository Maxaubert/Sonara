"""Win32 virtual-key codes + RegisterHotKey fsModifiers, and the Windows default
chord. Pure data -- no OS calls -- so it imports on any host for the mock suite."""

# Virtual-Key codes. Letters == ASCII uppercase; OEM keys per WinUser.h.
KEY_CODES = {
    "s": 0x53, "r": 0x52, "d": 0x44, "l": 0x4C, "v": 0x56, "o": 0x4F,   # 's' = pause
    "f": 0x46, "p": 0x50, "m": 0x4D,  # 'p' = next_session, 'm' = mute; 'f' kept available
    "period": 0xBE, ".": 0xBE,        # VK_OEM_PERIOD
    "rightbracket": 0xDD, "]": 0xDD,  # VK_OEM_6
    "leftbracket": 0xDB, "[": 0xDB,   # VK_OEM_4
    # Arrow keys (VK_LEFT/UP/RIGHT/DOWN), with aliases.
    "left": 0x25, "leftarrow": 0x25,
    "up": 0x26, "uparrow": 0x26,
    "right": 0x27, "rightarrow": 0x27,
    "down": 0x28, "downarrow": 0x28,
    # Navigation block: Home/End are the default restart/flush keys (#160).
    "home": 0x24, "end": 0x23, "pageup": 0x21, "pagedown": 0x22,
}
# Every letter and digit (#38): the table used to cover only 9 letters, so a
# hotkey captured on the settings page with any other letter persisted a name
# resolve_keymap rejects -- and one bad entry disables ALL hotkeys. VK codes
# for A-Z/0-9 equal their ASCII uppercase/digit values.
for _c in "abcdefghijklmnopqrstuvwxyz":
    KEY_CODES.setdefault(_c, ord(_c.upper()))
for _d in "0123456789":
    KEY_CODES.setdefault(_d, ord(_d))

# RegisterHotKey fsModifiers (WinUser.h).
MOD_MASKS = {
    "alt": 0x0001, "ctrl": 0x0002, "control": 0x0002,
    "shift": 0x0004, "win": 0x0008, "cmd": 0x0008,  # 'cmd' -> Win key for portability
}

# MOD_NOREPEAT (0x4000) is OR-ed in at register time, not part of a chord.
MOD_NOREPEAT = 0x4000

# Default chord: Win+Alt (#160). Ctrl+Alt was the default until 0.7.0, but
# AltGr arrives as LCtrl+RAlt, so on layouts with AltGr characters (Norwegian
# and German AltGr+M type the micro sign, Polish letters) a Ctrl+Alt hotkey
# eats that character (E16). A Win chord never matches AltGr. Windows 11 owns
# several Win+Alt chords (arrows snap windows, M/R/G/T/B/PrtScn belong to Game
# Bar and HDR, D/K/H/digits to the shell), so the default KEYS in keymap.py
# avoid them. RegisterHotKey suppresses the Start menu on the Win key-up of a
# chord it delivered, so no extra handling is needed.
DEFAULT_MODS = ["win", "alt"]
