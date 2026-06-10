# tests/test_macos_hotkeys.py
from sonari.platform.macos.hotkeys import MacHotkeyBackend


def test_display_combo_labels_ctrl_cmd_o():
    hk = MacHotkeyBackend()
    # 4096 (ctrl) | 256 (cmd) == 4352 ; key 31 == 'O'
    assert hk.display_combo(4352, 31) == "Ctrl+Cmd+O"


def test_display_tables_cover_every_keycode_and_modifier():
    from sonari.platform.macos import keytables
    hk = MacHotkeyBackend()
    for code in keytables.KEY_CODES.values():
        assert code in hk._keycode_display
    for mask in keytables.MOD_MASKS.values():
        assert mask in {m for m, _ in hk._mod_display}
