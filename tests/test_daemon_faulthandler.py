import os

from sonara.platform.windows import process as process_mod


def test_faulthandler_log_under_sonara_dir(tmp_path):
    """arm_faulthandler must write its log UNDER the (monkeypatched) SONARA_DIR,
    never the developer's real ~/.sonara -- so the test suite stays isolated and
    SONARA_DIR redirection works in prod. tmp_path here is the SAME path the
    autouse _isolate_sonara_dir fixture uses, so this equals the patched dir."""
    process_mod.arm_faulthandler()
    expected = tmp_path / ".sonara" / "faulthandler.log"
    assert expected.exists()
    assert process_mod._FAULT_FILE is not None
    assert os.path.realpath(process_mod._FAULT_FILE.name) == os.path.realpath(str(expected))
