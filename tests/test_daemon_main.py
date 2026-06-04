from unittest import mock

import sonari.daemon as daemon_mod


def test_ensure_running_noop_when_socket_connectable():
    with mock.patch("sonari.daemon.socket_connectable", return_value=True) as conn, \
         mock.patch("sonari.daemon.subprocess.Popen") as popen:
        daemon_mod.ensure_running()
    conn.assert_called_once()
    popen.assert_not_called()


def test_ensure_running_spawns_detached_when_socket_absent():
    with mock.patch("sonari.daemon.socket_connectable", return_value=False), \
         mock.patch("sonari.daemon.subprocess.Popen") as popen:
        daemon_mod.ensure_running()
    assert popen.call_count == 1
    args, kwargs = popen.call_args
    # spawned detached
    assert kwargs.get("start_new_session") is True
    # spawns the bin/sonari-daemon shim
    cmd = args[0]
    assert any("sonari-daemon" in str(part) for part in cmd)


def test_main_builds_components_and_runs():
    fake_cfg = {"voice": None, "rate": 200, "verbosity": "everything",
                "background_policy": "earcon_only", "earcons": {}}
    with mock.patch("sonari.daemon.load_config", return_value=fake_cfg), \
         mock.patch("sonari.daemon.SpeechDaemon.run", autospec=True) as run:
        daemon_mod.main()
    assert run.call_count == 1
    built = run.call_args[0][0]
    assert isinstance(built, daemon_mod.SpeechDaemon)
    assert built.config is fake_cfg
