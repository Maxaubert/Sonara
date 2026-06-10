"""macOS Carbon key-code + modifier-mask tables (used to resolve the keymap
into the form the Swift hotkeyd reads)."""

KEY_CODES = {
    "s": 1, "r": 15, "d": 2, "l": 37, "v": 9, "o": 31,
    "period": 47, ".": 47,
    "rightbracket": 30, "]": 30,
    "leftbracket": 33, "[": 33,
}

MOD_MASKS = {
    "cmd": 256, "shift": 512,
    "opt": 2048, "option": 2048,
    "ctrl": 4096, "control": 4096,
}
