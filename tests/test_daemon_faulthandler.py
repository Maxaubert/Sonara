import os

import sonari.daemon as daemon_mod


def test_faulthandler_log_under_sonari_dir(tmp_path):
    """_arm_faulthandler must write its log UNDER the (monkeypatched) SONARI_DIR,
    never the developer's real ~/.sonari — so the test suite stays isolated and
    SONARI_DIR redirection works in prod. tmp_path here is the SAME path the
    autouse _isolate_sonari_dir fixture uses, so this equals the patched dir."""
    daemon_mod._arm_faulthandler()
    expected = tmp_path / ".sonari" / "faulthandler.log"
    assert expected.exists()
    assert daemon_mod._FAULT_FILE is not None
    assert os.path.realpath(daemon_mod._FAULT_FILE.name) == os.path.realpath(str(expected))
