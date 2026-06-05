from unittest import mock

from sonari import cli


def test_dev_migrate_noop_when_no_editable_footprint():
    with mock.patch.object(cli, "_detect_editable_sonari", return_value=None):
        lines = cli._dev_install_migrate()
    assert lines == []


def test_dev_migrate_prints_guidance_when_editable_detected():
    with mock.patch.object(cli, "_detect_editable_sonari",
                           return_value="/opt/homebrew/bin/python3"):
        lines = cli._dev_install_migrate()
    assert len(lines) >= 1
    joined = " ".join(lines)
    # Guidance names the interpreter and the manual uninstall command.
    assert "/opt/homebrew/bin/python3" in joined
    assert "pip uninstall sonari" in joined
    # MUST NOT claim to have auto-uninstalled anything.
    assert "Removed" not in joined


def test_dev_migrate_never_auto_uninstalls(monkeypatch):
    """Even when an editable footprint exists, _dev_install_migrate must not
    shell out to pip — it only returns guidance strings."""
    called = []
    monkeypatch.setattr(cli.subprocess, "call",
                        lambda *a, **k: called.append(a) or 0)
    monkeypatch.setattr(cli.subprocess, "check_output",
                        lambda *a, **k: called.append(a) or "")
    with mock.patch.object(cli, "_detect_editable_sonari",
                           return_value="/some/python3"):
        cli._dev_install_migrate()
    assert called == [], "dev migrate must not run any subprocess"
