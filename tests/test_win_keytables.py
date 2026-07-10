from sonara.platform.windows import keytables as wk


def test_vk_codes_for_default_action_keys():
    # Virtual-Key codes (Win32): letters are their ASCII uppercase ordinals.
    assert wk.KEY_CODES["s"] == 0x53 and wk.KEY_CODES["o"] == 0x4F
    assert wk.KEY_CODES["."] == 0xBE          # VK_OEM_PERIOD
    assert wk.KEY_CODES["]"] == 0xDD and wk.KEY_CODES["["] == 0xDB


def test_mod_masks_are_registerhotkey_fsmodifiers():
    assert wk.MOD_MASKS["alt"] == 0x0001 and wk.MOD_MASKS["ctrl"] == 0x0002
    assert wk.MOD_MASKS["shift"] == 0x0004 and wk.MOD_MASKS["win"] == 0x0008


def test_default_mods_is_ctrl_alt():
    assert wk.DEFAULT_MODS == ["ctrl", "alt"]


def test_arrow_key_vk_codes():
    assert wk.KEY_CODES["left"] == 0x25 and wk.KEY_CODES["up"] == 0x26
    assert wk.KEY_CODES["right"] == 0x27 and wk.KEY_CODES["down"] == 0x28
    # aliases resolve to the same codes
    assert wk.KEY_CODES["rightarrow"] == wk.KEY_CODES["right"]
