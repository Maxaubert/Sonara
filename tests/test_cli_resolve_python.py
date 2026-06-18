from unittest import mock

from sonari.platform.macos.supervisor import MacSupervisorBackend


def test_resolve_prefers_usr_bin_python3_when_it_qualifies():
    # /usr/bin/python3 reports 3.9; a newer 3.13 is also present, but stability
    # wins: /usr/bin/python3 must be chosen.
    def fake_which(name):
        return {"python3": "/opt/homebrew/bin/python3",
                "python3.13": "/opt/homebrew/bin/python3.13"}.get(name)

    def fake_realpath(p):
        return p  # identity so dedup is by literal path

    def fake_probe(path):
        return {"/usr/bin/python3": (3, 9),
                "/opt/homebrew/bin/python3": (3, 13),
                "/opt/homebrew/bin/python3.13": (3, 13)}.get(path)

    with mock.patch("shutil.which", side_effect=fake_which), \
         mock.patch("os.path.realpath", side_effect=fake_realpath), \
         mock.patch.object(MacSupervisorBackend, "_probe_python_version",
                           side_effect=fake_probe):
        chosen = MacSupervisorBackend().resolve_python()
    assert chosen == "/usr/bin/python3"


def test_resolve_falls_back_to_first_qualifying_path_candidate():
    # /usr/bin/python3 is too old; the first qualifying PATH candidate wins.
    def fake_which(name):
        return {"python3": "/opt/homebrew/bin/python3"}.get(name)

    def fake_probe(path):
        return {"/usr/bin/python3": (3, 8),
                "/opt/homebrew/bin/python3": (3, 12)}.get(path)

    with mock.patch("shutil.which", side_effect=fake_which), \
         mock.patch("os.path.realpath", side_effect=lambda p: p), \
         mock.patch.object(MacSupervisorBackend, "_probe_python_version",
                           side_effect=fake_probe):
        chosen = MacSupervisorBackend().resolve_python()
    assert chosen == "/opt/homebrew/bin/python3"


def test_resolve_returns_none_when_all_below_39():
    def fake_which(name):
        return {"python3": "/opt/homebrew/bin/python3"}.get(name)

    def fake_probe(path):
        return (3, 8)  # everything too old

    with mock.patch("shutil.which", side_effect=fake_which), \
         mock.patch("os.path.realpath", side_effect=lambda p: p), \
         mock.patch.object(MacSupervisorBackend, "_probe_python_version",
                           side_effect=fake_probe):
        assert MacSupervisorBackend().resolve_python() is None


def test_resolve_dedups_candidates_by_realpath():
    # which('python3') and which('python3.9') both point at the same realpath;
    # the probe must be called once for that realpath, not twice.
    def fake_which(name):
        return {"python3": "/a/python3", "python3.9": "/a/python3.9"}.get(name)

    def fake_realpath(p):
        # both /a/python3 and /a/python3.9 resolve to one canonical path
        return "/canon/python3" if p in ("/a/python3", "/a/python3.9") else p

    probe = mock.Mock(side_effect=lambda p: (3, 9))
    with mock.patch("shutil.which", side_effect=fake_which), \
         mock.patch("os.path.realpath", side_effect=fake_realpath), \
         mock.patch.object(MacSupervisorBackend, "_probe_python_version", probe):
        chosen = MacSupervisorBackend().resolve_python()
    assert chosen == "/usr/bin/python3" or chosen == "/canon/python3"
    # /usr/bin/python3 + the single deduped /canon/python3 => at most 2 probes.
    assert probe.call_count <= 2
