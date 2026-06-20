from unittest import mock

import sonara.daemon as daemon_mod


def test_main_exits_without_building_when_socket_connectable():
    with mock.patch("sonara.daemon.socket_connectable", return_value=True), \
         mock.patch.object(daemon_mod.SpeechDaemon, "run") as run, \
         mock.patch("sonara.daemon.load_config", return_value={}):
        daemon_mod.main()
    run.assert_not_called()


def test_main_builds_and_runs_when_socket_not_connectable():
    with mock.patch("sonara.daemon.socket_connectable", return_value=False), \
         mock.patch.object(daemon_mod.SpeechDaemon, "run") as run, \
         mock.patch("sonara.daemon.load_config", return_value={}), \
         mock.patch("sonara.speaker.Speaker"):
        daemon_mod.main()
    run.assert_called_once()
