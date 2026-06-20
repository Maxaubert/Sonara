"""macOS Carbon key-code + modifier-mask tables (used to resolve the keymap
into the form the Swift hotkeyd reads)."""

KEY_CODES = {
    "s": 1, "r": 15, "d": 2, "l": 37, "v": 9, "o": 31,   # 's' = pause
    "f": 3, "p": 35, "m": 46,         # 'p' = next_session, 'm' = mute (kVK_ANSI_P / _M); 'f' kept available
    "period": 47, ".": 47,
    "rightbracket": 30, "]": 30,
    "leftbracket": 33, "[": 33,
    # Arrow keys (Carbon virtual key codes), with aliases.
    "left": 123, "leftarrow": 123,
    "right": 124, "rightarrow": 124,
    "down": 125, "downarrow": 125,
    "up": 126, "uparrow": 126,
}

MOD_MASKS = {
    "cmd": 256, "shift": 512,
    "opt": 2048, "option": 2048,
    "ctrl": 4096, "control": 4096,
}

# Default chord on macOS (Ctrl+Cmd, avoids VoiceOver's Ctrl+Opt).
DEFAULT_MODS = ["ctrl", "cmd"]
