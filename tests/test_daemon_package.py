"""The daemon is a package (#141): `python -m sonara.daemon`, which the
supervisor loop and the lazy-start launch spec run, must still reach main()."""
import importlib.util
import runpy


def test_python_dash_m_sonara_daemon_runs_main(monkeypatch):
    import sonara.daemon as daemon_module
    assert importlib.util.find_spec("sonara.daemon.__main__") is not None
    calls = []
    monkeypatch.setattr(daemon_module, "main", lambda: calls.append(True))
    runpy.run_module("sonara.daemon", run_name="__main__")
    assert calls == [True]
