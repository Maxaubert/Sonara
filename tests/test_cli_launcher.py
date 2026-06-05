import os
import stat
from unittest import mock

from sonari import cli


def test_launcher_path_is_local_bin_sonari(monkeypatch, tmp_path):
    monkeypatch.setenv("HOME", str(tmp_path))
    assert cli._launcher_path() == str(tmp_path / ".local" / "bin" / "sonari")


def test_place_launcher_writes_executable_wrapper(monkeypatch, tmp_path):
    monkeypatch.setenv("HOME", str(tmp_path))
    plugin_root = "/Users/u/My Plugins/sonari"
    cli._place_launcher(plugin_root)
    path = tmp_path / ".local" / "bin" / "sonari"
    assert path.exists()
    mode = stat.S_IMODE(os.stat(path).st_mode)
    assert mode == 0o755
    text = path.read_text()
    # Execs the absolute plugin bin/sonari, with the (spaced) path quoted.
    assert 'exec "/Users/u/My Plugins/sonari/bin/sonari" "$@"' in text


def test_place_launcher_overwrites_existing(monkeypatch, tmp_path):
    monkeypatch.setenv("HOME", str(tmp_path))
    lb = tmp_path / ".local" / "bin"
    lb.mkdir(parents=True)
    (lb / "sonari").write_text("#!/bin/sh\necho stale\n")
    cli._place_launcher("/plug")
    assert 'exec "/plug/bin/sonari" "$@"' in (lb / "sonari").read_text()


def test_remove_launcher_deletes_file(monkeypatch, tmp_path):
    monkeypatch.setenv("HOME", str(tmp_path))
    lb = tmp_path / ".local" / "bin"
    lb.mkdir(parents=True)
    (lb / "sonari").write_text("x")
    removed = cli._remove_launcher()
    assert removed is True
    assert not (lb / "sonari").exists()


def test_remove_launcher_absent_is_noop(monkeypatch, tmp_path):
    monkeypatch.setenv("HOME", str(tmp_path))
    assert cli._remove_launcher() is False


def test_local_bin_on_path_true_when_present(monkeypatch, tmp_path):
    monkeypatch.setenv("HOME", str(tmp_path))
    lb = str(tmp_path / ".local" / "bin")
    monkeypatch.setenv("PATH", lb + ":/usr/bin")
    assert cli._local_bin_on_path() is True


def test_local_bin_on_path_false_when_absent(monkeypatch, tmp_path):
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setenv("PATH", "/usr/bin:/bin")
    assert cli._local_bin_on_path() is False
