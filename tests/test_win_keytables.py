from sonara.platform.windows import keytables as wk


def test_vk_codes_for_default_action_keys():
    # Virtual-Key codes (Win32): letters are their ASCII uppercase ordinals.
    assert wk.KEY_CODES["s"] == 0x53 and wk.KEY_CODES["o"] == 0x4F
    assert wk.KEY_CODES["."] == 0xBE          # VK_OEM_PERIOD
    assert wk.KEY_CODES["]"] == 0xDD and wk.KEY_CODES["["] == 0xDB


def test_mod_masks_are_registerhotkey_fsmodifiers():
    assert wk.MOD_MASKS["alt"] == 0x0001 and wk.MOD_MASKS["ctrl"] == 0x0002
    assert wk.MOD_MASKS["shift"] == 0x0004 and wk.MOD_MASKS["win"] == 0x0008


def test_default_mods_is_win_alt():
    # #160: Ctrl+Alt is AltGr on European layouts (it ate the micro sign on
    # AltGr+M); a Win chord never matches AltGr.
    assert wk.DEFAULT_MODS == ["win", "alt"]


def test_home_end_and_page_keys_are_bindable():
    assert wk.KEY_CODES["home"] == 0x24 and wk.KEY_CODES["end"] == 0x23
    assert wk.KEY_CODES["pageup"] == 0x21 and wk.KEY_CODES["pagedown"] == 0x22


def test_arrow_key_vk_codes():
    assert wk.KEY_CODES["left"] == 0x25 and wk.KEY_CODES["up"] == 0x26
    assert wk.KEY_CODES["right"] == 0x27 and wk.KEY_CODES["down"] == 0x28
    # aliases resolve to the same codes
    assert wk.KEY_CODES["rightarrow"] == wk.KEY_CODES["right"]
