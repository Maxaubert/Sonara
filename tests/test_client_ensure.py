from unittest import mock

import sonara.client as client_mod


def test_ensure_daemon_returns_fast_when_connectable():
    with mock.patch("sonara.client._connectable", return_value=True) as conn, \
         mock.patch("sonara.client.ensure_running") as run, \
         mock.patch("sonara.client.time.sleep") as slept:
        client_mod.ensure_daemon(timeout=3.0)
    # only one connectivity check; never spawns or sleeps
    conn.assert_called_once()
    run.assert_not_called()
    slept.assert_not_called()


def test_ensure_daemon_spawns_then_polls_until_connectable():
    # absent on first check, absent right after spawn, then connectable on the 2nd poll
    connectable_results = iter([False, False, True])

    def fake_connectable():
        return next(connectable_results)

    with mock.patch("sonara.client._connectable", side_effect=fake_connectable) as conn, \
         mock.patch("sonara.client.ensure_running") as run, \
         mock.patch("sonara.client.time.sleep") as slept:
        client_mod.ensure_daemon(timeout=3.0)
    # initial check + spawn + 2 polls = 3 connectivity checks total
    assert conn.call_count == 3
    run.assert_called_once()
    # slept once before the successful 2nd poll
    assert slept.call_count == 1


def test_ensure_daemon_returns_at_once_after_a_shutdown(tmp_path):
    # E7: after `sonara shutdown` every hook event spun the full 3 s timeout
    # waiting for a daemon the sentinel forbids, delaying every tool call.
    from sonara import paths
    paths.ensure_sonara_dir()
    paths.STOPPED_SENTINEL_PATH.write_text("sonara shutdown")
    with mock.patch("sonara.client._connectable", return_value=False), \
         mock.patch("sonara.client.ensure_running") as run, \
         mock.patch("sonara.client.time.sleep") as slept:
        client_mod.ensure_daemon(timeout=3.0)
    run.assert_not_called()
    slept.assert_not_called()
