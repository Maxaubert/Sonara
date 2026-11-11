"""packaging/sign_macos.py: the macOS runtime ships signed with a Developer
ID and notarized (user decision 2026-11-11), never unsigned or ad-hoc, and
release.yml fails the macOS build when the secrets are missing."""
from __future__ import annotations

import importlib.util
from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parents[2]
RELEASE = REPO / ".github" / "workflows" / "release.yml"


def _module():
    spec = importlib.util.spec_from_file_location("sign_macos", REPO / "packaging" / "sign_macos.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


sm = _module()

CERT = {"APPLE_CERT_P12_BASE64": "cDEy", "APPLE_CERT_PASSWORD": "pw", "APPLE_TEAM_ID": "ABCDE12345"}
API_KEY = {"APPLE_API_KEY_P8_BASE64": "cDg=", "APPLE_API_KEY_ID": "KEY123", "APPLE_API_ISSUER": "iss-uuid"}
APPLE_ID = {"APPLE_ID": "dev@example.com", "APPLE_APP_PASSWORD": "abcd-efgh"}


def test_no_secrets_names_every_one_that_is_missing():
    lacking = sm.missing_secrets({})
    assert lacking[:3] == ["APPLE_CERT_P12_BASE64", "APPLE_CERT_PASSWORD", "APPLE_TEAM_ID"]
    assert "APPLE_API_KEY_ID" in lacking[3] and "APPLE_APP_PASSWORD" in lacking[3]


@pytest.mark.parametrize("notary", [API_KEY, APPLE_ID])
def test_the_certificate_and_either_notary_credential_are_enough(notary):
    assert sm.missing_secrets({**CERT, **notary}) == []


def test_a_certificate_without_notary_credentials_is_not_enough():
    assert len(sm.missing_secrets(CERT)) == 1
    assert len(sm.missing_secrets({**CERT, "APPLE_ID": "x"})) == 1
    assert sm.missing_secrets({**CERT, **API_KEY, "APPLE_TEAM_ID": " "}) == ["APPLE_TEAM_ID"]


def test_check_secrets_fails_without_them(monkeypatch, capsys):
    for name in (*sm.CERT_SECRETS, *sm.API_KEY_SECRETS, *sm.APPLE_ID_SECRETS):
        monkeypatch.delenv(name, raising=False)
    assert sm.main(["check-secrets"]) == 1
    assert "must be signed and notarized" in capsys.readouterr().err


def test_a_developer_id_signature_has_the_hardened_runtime_and_a_timestamp():
    argv = sm.sign_command(Path("/b/sonarad"), "ABC123", "/k/ci.keychain-db")
    assert argv[:4] == ["codesign", "--force", "--sign", "ABC123"]
    assert argv[argv.index("--options") + 1] == "runtime"
    assert "--timestamp" in argv
    assert argv[argv.index("--keychain") + 1] == "/k/ci.keychain-db"
    assert argv[-1] == "/b/sonarad"


def test_an_ad_hoc_signature_is_only_for_local_tests():
    argv = sm.sign_command(Path("/b/sonarad"), "-")
    assert "--options" not in argv and "--timestamp=none" in argv


def test_sonarad_is_signed_with_the_apple_events_entitlement():
    # Hardened, osascript's Apple Events (media pause) are refused without it.
    argv = sm.sign_command(Path("/b/sonarad"), "ABC123")
    ent = Path(argv[argv.index("--entitlements") + 1])
    assert ent.is_file()
    assert "com.apple.security.automation.apple-events" in ent.read_text(encoding="utf-8")
    for other in ("sonara-hook", "sonara", "libonnxruntime.dylib"):
        assert "--entitlements" not in sm.sign_command(Path("/b") / other, "ABC123")


def test_a_hardened_ad_hoc_signature_matches_the_release_options():
    argv = sm.sign_command(Path("/b/sonarad"), "-", hardened=True)
    assert argv[argv.index("--options") + 1] == "runtime"
    assert "--entitlements" in argv and "--timestamp=none" in argv
    # Only ad-hoc turns library validation off (no team ID to match).
    adhoc = Path(argv[argv.index("--entitlements") + 1]).read_text(encoding="utf-8")
    assert "disable-library-validation" in adhoc
    real = sm.sign_command(Path("/b/sonarad"), "ABC123")
    assert "disable-library-validation" not in Path(real[real.index("--entitlements") + 1]).read_text(
        encoding="utf-8")


def test_a_rejected_notarization_prints_apples_log_even_when_notarytool_fails():
    out = '{"id": "abc-1", "status": "Invalid", "message": "Processing complete"}'
    asked = []

    def log(sid):
        asked.append(sid)
        return "the binary is not signed with a valid Developer ID"

    with pytest.raises(SystemExit, match="not signed with a valid Developer ID") as e:
        sm._verdict(1, out, "", log, secret="hunter2")
    assert asked == ["abc-1"] and "Invalid" in str(e.value)
    assert sm._verdict(0, '{"id": "x", "status": "Accepted"}', "", log)["status"] == "Accepted"
    with pytest.raises(SystemExit) as e:
        sm._verdict(2, "", "auth failed for hunter2", log, secret="hunter2")
    assert "hunter2" not in str(e.value) and "auth failed for ***" in str(e.value)


def test_the_library_is_signed_before_the_executables(tmp_path):
    for n in ("sonarad", "sonara-hook", "sonara", "libonnxruntime.dylib", "sonarad.d"):
        (tmp_path / n).write_bytes(b"x")
    assert [p.name for p in sm.files_to_sign(tmp_path)] == [
        "libonnxruntime.dylib", "sonarad", "sonara-hook", "sonara"]
    (tmp_path / "sonara").unlink()
    with pytest.raises(SystemExit, match="sonara not in"):
        sm.files_to_sign(tmp_path)


def test_notarytool_prefers_the_api_key(tmp_path):
    key = tmp_path / "AuthKey.p8"
    auth = sm.notary_auth({**CERT, **API_KEY, **APPLE_ID}, key)
    assert auth == ["--key", str(key), "--key-id", "KEY123", "--issuer", "iss-uuid"]
    auth = sm.notary_auth({**CERT, **APPLE_ID})
    assert auth == ["--apple-id", "dev@example.com", "--password", "abcd-efgh", "--team-id", "ABCDE12345"]
    with pytest.raises(SystemExit):
        sm.notary_auth(CERT)


def _release_jobs() -> dict:
    yaml = pytest.importorskip("yaml")
    return yaml.safe_load(RELEASE.read_text(encoding="utf-8"))["jobs"]


def test_the_macos_release_build_fails_without_the_signing_secrets():
    jobs = _release_jobs()
    mac = jobs["build-macos"]
    assert "macos" in mac["runs-on"]
    runs = [s.get("run", "") for s in mac["steps"]]
    check = next(i for i, r in enumerate(runs) if "sign_macos.py check-secrets" in r)
    sign = next(i for i, r in enumerate(runs) if "sign_macos.py sign" in r)
    notarize = next(i for i, r in enumerate(runs) if "sign_macos.py notarize" in r)
    pack = next(i for i, r in enumerate(runs) if "release_zip.py --platform macos-arm64" in r)
    assert check < sign < notarize < pack
    assert not any(s.get("continue-on-error") for s in mac["steps"]), "a failed signing step fails the release"
    assert "--identity -" not in "\n".join(runs), "never ad-hoc in a release"


def test_the_release_is_published_only_with_both_platform_assets():
    jobs = _release_jobs()
    publish = jobs["release"]
    assert sorted(publish["needs"]) == ["build-macos", "build-windows", "prepare"]
    run = "\n".join(s.get("run", "") for s in publish["steps"])
    assert "sonara-runtime-win-x64-" in run and "sonara-runtime-macos-arm64-" in run
    assert "release_zip.py --merge" in run
    assert "gh release create" in run


def test_the_release_checks_the_requirement_the_bootstrap_enforces():
    import re
    boot = (REPO / "bin" / "sonara-bootstrap.sh").read_text(encoding="utf-8")
    req = re.search(r"^requirement='([^']+)'$", boot, re.M).group(1)
    assert "certificate leaf[field.1.2.840.113635.100.6.1.13]" in req  # Developer ID Application
    release = (REPO / ".github" / "workflows" / "release.yml").read_text(encoding="utf-8")
    assert "-R='" + req + "'" in release


def test_the_entitlements_parse_and_hold_no_xml_comments():
    # codesign's parser (AMFIUnserializeXML) refuses a comment: the
    # reasons live in packaging/macos/README.md.
    import plistlib
    for p in (REPO / "packaging" / "macos").glob("*.entitlements"):
        text = p.read_text(encoding="utf-8")
        assert "<!--" not in text, p.name
        assert plistlib.loads(text.encode())["com.apple.security.automation.apple-events"] is True
