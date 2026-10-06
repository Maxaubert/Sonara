"""#279: the SDKs are published by release.yml through trusted publishing
(OIDC), switched on per registry by a repository variable, from the packages
packaging/build_packages.py builds and checks (as ci.yml does on every PR)."""
from __future__ import annotations

import importlib.util
import json
import re
from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parents[2]
RELEASE = REPO / ".github" / "workflows" / "release.yml"


def _module():
    spec = importlib.util.spec_from_file_location("build_packages", REPO / "packaging" / "build_packages.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def _package(rel: str) -> dict:
    return json.loads((REPO / rel / "package.json").read_text(encoding="utf-8"))


def _release_jobs() -> dict:
    yaml = pytest.importorskip("yaml")
    return yaml.safe_load(RELEASE.read_text(encoding="utf-8"))["jobs"]


def test_only_the_player_is_kept_off_npm():
    assert _package("clients/player").get("private") is True
    for rel in ("clients/ts", "packaging/npm-runtime"):
        assert not _package(rel).get("private"), rel


def test_the_runtime_package_check_requires_every_file_build_mjs_stages():
    mod = _module()
    build = (REPO / "packaging" / "npm-runtime" / "scripts" / "build.mjs").read_text(encoding="utf-8")
    staged = re.search(r"const RUNTIME_FILES = \[(.*?)\];", build, re.S).group(1)
    for name in re.findall(r'"([^"]+)"', staged):
        assert "bin/" + name in mod.RUNTIME_REQUIRED, name
    assert "bin/sonarad.exe" in mod.RUNTIME_REQUIRED


def test_the_required_npm_files_are_in_the_package_files_lists():
    mod = _module()
    for rel, required in (("clients/ts", mod.TS_REQUIRED), ("packaging/npm-runtime", mod.RUNTIME_REQUIRED)):
        files = _package(rel)["files"]
        for path in required:
            if path == "package.json":
                continue
            assert any(path == f or path.startswith(f) for f in files), (rel, path)


def test_a_missing_file_is_named():
    mod = _module()
    assert mod.missing(["package.json", "bin/sonarad.exe"], ("package.json", "bin/sonarad.exe")) == []
    assert mod.missing(["package.json"], ("package.json", "bin/onnxruntime.dll")) == ["bin/onnxruntime.dll"]


def test_publishing_uses_no_registry_tokens():
    text = RELEASE.read_text(encoding="utf-8")
    for token in ("NPM_TOKEN", "PYPI_TOKEN", "NODE_AUTH_TOKEN", "TWINE_PASSWORD", "secrets."):
        assert token not in text, token


def test_each_registry_publishes_by_oidc_from_its_environment_behind_its_switch():
    jobs = _release_jobs()
    for job, env, switch in (("publish-npm", "npm", "PUBLISH_NPM"), ("publish-pypi", "pypi", "PUBLISH_PYPI")):
        spec = jobs[job]
        assert spec["needs"] == "packages"
        assert spec["permissions"]["id-token"] == "write", job
        name = spec["environment"]["name"] if isinstance(spec["environment"], dict) else spec["environment"]
        assert name == env, job
        # !cancelled(): the implicit success() also wants the release job,
        # which a packages_only run skips, so the publish would never run.
        assert spec["if"] == "!cancelled() && needs.packages.result == 'success' && vars.{0} == 'true'".format(
            switch), job
        # Trusted publishing works only on GitHub-hosted runners; pypa's action needs Linux.
        assert spec["runs-on"] == "ubuntu-latest", job
    # Only the publish jobs can mint an OIDC token.
    for job, spec in jobs.items():
        if job not in ("publish-npm", "publish-pypi"):
            assert "id-token" not in (spec.get("permissions") or {}), job


def test_the_packages_are_built_only_when_a_registry_is_switched_on():
    jobs = _release_jobs()
    cond = jobs["packages"]["if"]
    assert "vars.PUBLISH_PYPI == 'true' || vars.PUBLISH_NPM == 'true'" in cond
    # A failed release job still passes on its outputs: never publish after one.
    assert "needs.release.result == 'success' && needs.release.outputs.released == 'true'" in cond
    assert "inputs.packages_only" in cond
    run = "\n".join(s.get("run", "") for s in jobs["packages"]["steps"])
    assert "packaging/build_packages.py" in run
    assert "runtime_dlls.py stage target/release" in run


def test_a_rerun_skips_a_version_the_registry_has():
    jobs = _release_jobs()
    npm = "\n".join(s.get("run", "") for s in jobs["publish-npm"]["steps"])
    assert "is on npm already: skipped" in npm
    pypi = jobs["publish-pypi"]["steps"]
    assert any("is on PyPI already: skipped" in s.get("run", "") for s in pypi)
    action = next(s for s in pypi if str(s.get("uses", "")).startswith("pypa/gh-action-pypi-publish@"))
    assert action["with"]["skip-existing"] is True


def test_ci_dry_runs_the_publish_in_the_clients_job():
    yaml = pytest.importorskip("yaml")
    ci = yaml.safe_load((REPO / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8"))
    run = "\n".join(s.get("run", "") for s in ci["jobs"]["clients"]["steps"])
    assert "python packaging/build_packages.py --publish-dry-run" in run


def test_a_runtime_tarball_without_sonarad_fails_the_check(tmp_path):
    import io
    import tarfile

    mod = _module()
    tgz = tmp_path / "sonara-runtime-win32-x64-0.0.0.tgz"
    with tarfile.open(tgz, "w:gz") as t:
        for name in mod.RUNTIME_REQUIRED:
            if name == "bin/sonarad.exe":
                continue
            data = b"x"
            info = tarfile.TarInfo("package/" + name)
            info.size = len(data)
            t.addfile(info, io.BytesIO(data))
    with pytest.raises(SystemExit, match="bin/sonarad.exe"):
        mod._check(tgz.name, mod.tarball_files(tgz), mod.RUNTIME_REQUIRED)


def test_a_wheel_without_the_version_module_fails_the_check(tmp_path):
    import zipfile

    mod = _module()
    whl = tmp_path / "sonara_client-0.0.0-py3-none-any.whl"
    with zipfile.ZipFile(whl, "w") as z:
        z.writestr("sonara_client/__init__.py", "")
        z.writestr("sonara_client/client.py", "")
    with pytest.raises(SystemExit, match="sonara_client/version.py"):
        mod._check(whl.name, mod.wheel_files(whl), mod.WHEEL_REQUIRED)


def test_the_npm_dry_run_never_asks_the_real_registry():
    # npm 11 fails a dry run of a version the registry has: every branch
    # without a bump would fail once a release is on npm.
    mod = _module()
    flags = mod.DRY_RUN_OFFLINE
    assert "--offline" in flags
    registry = flags[flags.index("--registry") + 1]
    assert "registry.npmjs.org" not in registry
