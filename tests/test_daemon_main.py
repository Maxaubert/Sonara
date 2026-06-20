from unittest import mock

import sonara.daemon as daemon_mod


def test_ensure_running_noop_when_socket_connectable():
    with mock.patch("sonara.daemon.socket_connectable", return_value=True) as conn, \
         mock.patch("sonara.daemon.subprocess.Popen") as popen:
        daemon_mod.ensure_running()
    conn.assert_called_once()
    popen.assert_not_called()


def test_ensure_running_spawns_via_platform_launch_spec_when_socket_absent():
    # ensure_running asks the platform supervisor for its (argv, kwargs) launch
    # spec and Popens it verbatim. On Windows that's [pythonw, -m, sonara.daemon]
    # with detach creationflags — no POSIX start_new_session, no bin shim.
    fake_argv = ["pythonw.exe", "-m", "sonara.daemon"]
    fake_kwargs = {"creationflags": 0x208}
    sup = mock.Mock()
    sup.launch_spec.return_value = (fake_argv, dict(fake_kwargs))
    plat = mock.Mock(supervisor=sup)
    with mock.patch("sonara.daemon.socket_connectable", return_value=False), \
         mock.patch("sonara.platform.get_platform", return_value=plat), \
         mock.patch("sonara.daemon.subprocess.Popen") as popen:
        daemon_mod.ensure_running()
    popen.assert_called_once_with(fake_argv, **fake_kwargs)


def test_main_builds_components_and_runs():
    fake_cfg = {"voice": None, "rate": 200, "verbosity": "everything",
                "background_policy": "earcon_only", "earcons": {}}
    with mock.patch("sonara.daemon.load_config", return_value=fake_cfg), \
         mock.patch("sonara.daemon.SpeechDaemon.run", autospec=True) as run:
        daemon_mod.main()
    assert run.call_count == 1
    built = run.call_args[0][0]
    assert isinstance(built, daemon_mod.SpeechDaemon)
    assert built.config is fake_cfg
