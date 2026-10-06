"""The local gate script (packaging/gate.py, #256): which gates a set of
changed paths needs, which crates a quick run tests, and that CI keeps the
job names the branch protection requires."""
from __future__ import annotations

import importlib.util
from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parents[2]


def _module():
    spec = importlib.util.spec_from_file_location("gate", REPO / "packaging" / "gate.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


gate = _module()


def test_a_docs_only_change_runs_only_the_python_checks():
    assert gate.select(["docs/README.md", "CLAUDE.md"]) == ["python"]


def test_nothing_changed_runs_nothing():
    assert gate.select([]) == []


def test_a_crate_source_change_runs_rust_and_conformance():
    assert gate.select(["crates/sonara-core/src/assembler.rs"]) == ["rust", "conformance", "python"]


def test_a_dependency_change_also_runs_deny_and_notices():
    got = gate.select(["crates/sonara-engine/Cargo.toml"])
    assert got == ["rust", "deps", "conformance", "python"]


def test_the_settings_page_runs_the_e2e_tests():
    for path in ("crates/sonarad/assets/settings.html", "crates/sonarad/src/settings_page.rs",
                 "crates/sonarad/src/config.rs", "tests/e2e/test_sonarad_settings_e2e.py"):
        assert "e2e" in gate.select([path]), path


def test_a_version_bump_runs_every_gate_but_the_e2e_and_earcons():
    got = gate.select(gate.bump_version.all_paths())
    assert got == ["rust", "deps", "conformance", "python", "sdk"]


def test_client_and_packaging_changes_run_the_sdk_gates():
    for path in ("clients/ts/src/client.ts", "clients/python/src/sonara_client/client.py",
                 "packaging/npm-runtime/index.js", "packaging/smoke/run-node.mjs"):
        assert "sdk" in gate.select([path]), path


def test_the_plugin_scripts_run_conformance():
    for path in ("bin/sonara-hook-launch", "bin/sonara-bootstrap.ps1", "hooks/hooks.json",
                 "conformance/core/test_instance.py", "packaging/release_zip.py"):
        assert "conformance" in gate.select([path]), path


def test_earcon_sources_run_the_earcon_check():
    assert "earcons" in gate.select(["packaging/sounds/build_earcons.py"])
    assert "earcons" in gate.select(["crates/sonara-agent/sounds/done.wav"])


def test_golden_fixtures_run_the_rust_tests():
    assert "rust" in gate.select(["tests/fixtures/text_rules/cases.json"])


# A small workspace: core <- reader <- sonarad, and log on its own.
META = {
    "workspace_root": "C:\\repo",
    "workspace_members": ["core 0.1.0 (path+file:///C:/repo/crates/sonara-core)",
                          "reader 0.1.0 (path+file:///C:/repo/crates/sonara-reader)",
                          "d 0.1.0 (path+file:///C:/repo/crates/sonarad)",
                          "log 0.1.0 (path+file:///C:/repo/crates/sonara-log)"],
    "packages": [
        {"id": "core 0.1.0 (path+file:///C:/repo/crates/sonara-core)", "name": "sonara-core",
         "manifest_path": "C:\\repo\\crates\\sonara-core\\Cargo.toml", "dependencies": []},
        {"id": "reader 0.1.0 (path+file:///C:/repo/crates/sonara-reader)", "name": "sonara-reader",
         "manifest_path": "C:\\repo\\crates\\sonara-reader\\Cargo.toml",
         "dependencies": [{"name": "sonara-core"}, {"name": "serde"}]},
        {"id": "d 0.1.0 (path+file:///C:/repo/crates/sonarad)", "name": "sonarad",
         "manifest_path": "C:\\repo\\crates\\sonarad\\Cargo.toml",
         "dependencies": [{"name": "sonara-reader"}]},
        {"id": "log 0.1.0 (path+file:///C:/repo/crates/sonara-log)", "name": "sonara-log",
         "manifest_path": "C:\\repo\\crates\\sonara-log\\Cargo.toml", "dependencies": []},
    ],
}


def test_quick_tests_the_changed_crate_and_every_crate_that_depends_on_it():
    got = gate.affected_crates(["crates/sonara-core/src/lib.rs"], META)
    assert got == ["sonara-core", "sonara-reader", "sonarad"]


def test_quick_tests_only_a_leaf_crate_when_only_it_changed():
    assert gate.affected_crates(["crates/sonarad/src/main.rs"], META) == ["sonarad"]


@pytest.mark.parametrize("path", ["Cargo.toml", "Cargo.lock", ".cargo/config.toml",
                                  "tests/fixtures/text_rules/cases.json"])
def test_quick_tests_the_whole_workspace_when_a_shared_file_changed(path):
    assert gate.affected_crates([path], META) is None


def test_ci_keeps_the_required_check_names():
    yaml = pytest.importorskip("yaml")
    ci = yaml.safe_load((REPO / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8"))
    assert sorted(ci["jobs"]) == sorted(gate.REQUIRED_CHECKS)
