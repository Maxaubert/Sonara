"""Sign and notarize the macOS runtime (Developer ID, user decision
2026-11-11: no unsigned or ad-hoc macOS release).

    python packaging/sign_macos.py check-secrets
    python packaging/sign_macos.py import-cert            # CI: a temporary keychain
    python packaging/sign_macos.py sign target/release [--identity ID] [--keychain PATH]
    python packaging/sign_macos.py notarize target/release
    python packaging/sign_macos.py sign target/release --identity -   # ad-hoc, LOCAL tests only

``sign`` signs ``libonnxruntime.dylib`` first, then ``sonarad``,
``sonara-hook`` and ``sonara``, each with the hardened runtime and a secure
timestamp (``codesign --force --options runtime --timestamp``), and checks
each with ``codesign --verify --strict``. ``notarize`` zips the signed
files (``ditto``), sends them to Apple's notary service with
``xcrun notarytool submit --wait`` and fails unless Apple accepts them; the
log of a rejected submission is printed. Bare Mach-O files take no stapled
ticket: Gatekeeper looks the notarization up online. ``release_zip.py
--platform macos-arm64`` then packs the same signed files.

The secrets (GitHub Actions repository secrets; docs/bundling.md):
  APPLE_CERT_P12_BASE64   the Developer ID Application certificate and key (.p12, base64)
  APPLE_CERT_PASSWORD     the .p12 password
  APPLE_TEAM_ID           the 10-character team ID
and for notarytool either an App Store Connect API key (preferred):
  APPLE_API_KEY_P8_BASE64, APPLE_API_KEY_ID, APPLE_API_ISSUER
or an Apple ID with an app-specific password:
  APPLE_ID, APPLE_APP_PASSWORD
``check-secrets`` exits 1 naming what is missing, so release.yml fails the
macOS build instead of shipping it unsigned. Stdlib only.
"""
from __future__ import annotations

import argparse
import base64
import json
import os
import secrets
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Dict, List, Mapping, Optional

# Signed in this order: the library the runtime loads, then the executables.
SIGNED = ("libonnxruntime.dylib", "sonarad", "sonara-hook", "sonara")
CERT_SECRETS = ("APPLE_CERT_P12_BASE64", "APPLE_CERT_PASSWORD", "APPLE_TEAM_ID")
API_KEY_SECRETS = ("APPLE_API_KEY_P8_BASE64", "APPLE_API_KEY_ID", "APPLE_API_ISSUER")
APPLE_ID_SECRETS = ("APPLE_ID", "APPLE_APP_PASSWORD")
IDENTITY_PREFIX = "Developer ID Application"


def _set(env: Mapping[str, str], name: str) -> bool:
    return bool((env.get(name) or "").strip())


def missing_secrets(env: Mapping[str, str]) -> List[str]:
    """The secrets a signed and notarized release still lacks (empty when
    complete). The notary credentials are an API key or an Apple ID."""
    lacking = [n for n in CERT_SECRETS if not _set(env, n)]
    if not all(_set(env, n) for n in API_KEY_SECRETS) and not all(_set(env, n) for n in APPLE_ID_SECRETS):
        lacking.append("{0} (or {1})".format(" + ".join(API_KEY_SECRETS), " + ".join(APPLE_ID_SECRETS)))
    return lacking


def sign_command(path: Path, identity: str, keychain: Optional[str] = None) -> List[str]:
    """``codesign`` for one file: hardened runtime and a secure timestamp
    for a real identity; ad-hoc (``-``) gets neither (local tests only)."""
    argv = ["codesign", "--force", "--sign", identity]
    if identity != "-":
        argv += ["--options", "runtime", "--timestamp"]
        if keychain:
            argv += ["--keychain", keychain]
    else:
        argv += ["--timestamp=none"]
    return argv + [str(path)]


def verify_command(path: Path) -> List[str]:
    return ["codesign", "--verify", "--strict", "--verbose=2", str(path)]


def notary_auth(env: Mapping[str, str], key_path: Optional[Path] = None) -> List[str]:
    """notarytool's credential arguments: the API key when its secrets are
    set (written to ``key_path``), else the Apple ID."""
    if all(_set(env, n) for n in API_KEY_SECRETS):
        if key_path is None:
            raise ValueError("the API key needs a file")
        return ["--key", str(key_path), "--key-id", env["APPLE_API_KEY_ID"].strip(),
                "--issuer", env["APPLE_API_ISSUER"].strip()]
    if all(_set(env, n) for n in APPLE_ID_SECRETS) and _set(env, "APPLE_TEAM_ID"):
        return ["--apple-id", env["APPLE_ID"].strip(), "--password", env["APPLE_APP_PASSWORD"],
                "--team-id", env["APPLE_TEAM_ID"].strip()]
    raise SystemExit("sign_macos: no notary credentials: " + ", ".join(missing_secrets(env)))


def files_to_sign(folder: Path) -> List[Path]:
    """The files of ``SIGNED`` in ``folder``; the executables must be there
    (the library is optional: a build without Kokoro has none)."""
    found = [folder / n for n in SIGNED if (folder / n).is_file()]
    lacking = [n for n in SIGNED[1:] if not (folder / n).is_file()]
    if lacking:
        raise SystemExit("sign_macos: {0} not in {1}: run 'cargo build -p sonarad -p sonara-hook "
                         "-p sonara-cli --release' first".format(", ".join(lacking), folder))
    return found


def _run(argv: List[str], secret: Optional[str] = None) -> str:
    shown = " ".join("***" if secret and a == secret else a for a in argv)
    r = subprocess.run(argv, capture_output=True, text=True)
    if r.returncode != 0:
        raise SystemExit("sign_macos: {0} failed ({1}):\n{2}{3}".format(shown, r.returncode, r.stdout, r.stderr))
    return r.stdout


def find_identity(keychain: Optional[str] = None, team: Optional[str] = None) -> str:
    """The SHA-1 of the one Developer ID Application identity (of ``team``)."""
    argv = ["security", "find-identity", "-v", "-p", "codesigning"] + ([keychain] if keychain else [])
    hits = []
    for line in _run(argv).splitlines():
        parts = line.strip().split(None, 2)
        if len(parts) == 3 and parts[2].startswith('"' + IDENTITY_PREFIX):
            if team is None or "({0})".format(team) in parts[2]:
                hits.append(parts[1])
    if len(set(hits)) != 1:
        raise SystemExit("sign_macos: expected one '{0}' identity{1}, found {2}".format(
            IDENTITY_PREFIX, " of team " + team if team else "", len(set(hits))))
    return hits[0]


def import_cert(env: Mapping[str, str], keychain: Path) -> str:
    """CI: a temporary keychain holding the .p12 of the secrets, made the
    default for codesign; returns the identity's SHA-1."""
    lacking = [n for n in CERT_SECRETS if not _set(env, n)]
    if lacking:
        raise SystemExit("sign_macos: missing secrets: " + ", ".join(lacking))
    password = secrets.token_hex(16)
    p12 = keychain.with_suffix(".p12")
    p12.write_bytes(base64.b64decode(env["APPLE_CERT_P12_BASE64"]))
    try:
        _run(["security", "create-keychain", "-p", password, str(keychain)], password)
        _run(["security", "set-keychain-settings", "-lut", "21600", str(keychain)])
        _run(["security", "unlock-keychain", "-p", password, str(keychain)], password)
        cert_pw = env["APPLE_CERT_PASSWORD"]
        _run(["security", "import", str(p12), "-k", str(keychain), "-P", cert_pw, "-f", "pkcs12",
              "-T", "/usr/bin/codesign", "-T", "/usr/bin/security"], cert_pw)
        _run(["security", "set-key-partition-list", "-S", "apple-tool:,apple:", "-s", "-k", password,
              str(keychain)], password)
        current = [ln.strip().strip('"') for ln in _run(["security", "list-keychains", "-d", "user"]).splitlines()]
        _run(["security", "list-keychains", "-d", "user", "-s", str(keychain), *[c for c in current if c]])
    finally:
        p12.unlink()
    return find_identity(str(keychain), env["APPLE_TEAM_ID"].strip())


def sign(folder: Path, identity: str, keychain: Optional[str] = None) -> List[Path]:
    done = []
    for f in files_to_sign(folder):
        _run(sign_command(f, identity, keychain))
        _run(verify_command(f))
        done.append(f)
    return done


def notarize(folder: Path, env: Mapping[str, str]) -> Dict:
    """Submit the signed files of ``folder`` and wait for Apple's verdict."""
    with tempfile.TemporaryDirectory() as tmp:
        stage = Path(tmp) / "sonara-runtime"
        stage.mkdir()
        for f in files_to_sign(folder):
            _run(["ditto", str(f), str(stage / f.name)])
        archive = Path(tmp) / "sonara-runtime.zip"
        _run(["ditto", "-c", "-k", "--keepParent", str(stage), str(archive)])
        key = None
        if all(_set(env, n) for n in API_KEY_SECRETS):
            key = Path(tmp) / "AuthKey.p8"
            key.write_bytes(base64.b64decode(env["APPLE_API_KEY_P8_BASE64"]))
        auth = notary_auth(env, key)
        secret = env.get("APPLE_APP_PASSWORD") if "--password" in auth else None
        out = _run(["xcrun", "notarytool", "submit", str(archive), "--wait", "--timeout", "30m",
                    "--output-format", "json", *auth], secret)
        result = json.loads(out)
        if result.get("status") != "Accepted":
            log = subprocess.run(["xcrun", "notarytool", "log", str(result.get("id")), *auth],
                                 capture_output=True, text=True).stdout
            raise SystemExit("sign_macos: notarization {0}: {1}\n{2}".format(
                result.get("status"), result.get("message", ""), log))
        return result


def main(argv: Optional[List[str]] = None) -> int:
    ap = argparse.ArgumentParser(description="Sign and notarize the macOS runtime (Developer ID).")
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("check-secrets", help="exit 1 naming the signing secrets that are missing")
    ic = sub.add_parser("import-cert", help="CI: import the .p12 into a temporary keychain")
    ic.add_argument("--keychain", type=Path, default=Path(os.environ.get("RUNNER_TEMP", tempfile.gettempdir()))
                    / "sonara-signing.keychain-db")
    sg = sub.add_parser("sign", help="codesign the runtime files in a folder")
    sg.add_argument("folder", type=Path)
    sg.add_argument("--identity", default=os.environ.get("APPLE_SIGNING_IDENTITY"),
                    help="SHA-1 or name of the identity (default: the one Developer ID Application "
                         "identity; '-' is ad-hoc, for local tests only)")
    sg.add_argument("--keychain", default=os.environ.get("APPLE_SIGNING_KEYCHAIN"))
    nt = sub.add_parser("notarize", help="notarize the signed runtime files in a folder")
    nt.add_argument("folder", type=Path)
    args = ap.parse_args(argv)

    if args.cmd == "check-secrets":
        lacking = missing_secrets(os.environ)
        if lacking:
            print("sign_macos: a macOS release must be signed and notarized; missing secrets: "
                  + ", ".join(lacking) + " (setup: docs/bundling.md)", file=sys.stderr)
            return 1
        print("sign_macos: the signing and notary secrets are set")
        return 0
    if args.cmd == "import-cert":
        identity = import_cert(os.environ, args.keychain)
        # For later steps (GitHub Actions reads these from GITHUB_ENV).
        gh_env = os.environ.get("GITHUB_ENV")
        if gh_env:
            with open(gh_env, "a", encoding="utf-8") as f:
                f.write("APPLE_SIGNING_IDENTITY={0}\nAPPLE_SIGNING_KEYCHAIN={1}\n".format(identity, args.keychain))
        print(identity)
        return 0
    if args.cmd == "sign":
        identity = args.identity or find_identity(args.keychain)
        for f in sign(args.folder, identity, args.keychain):
            print(f)
        return 0
    result = notarize(args.folder, os.environ)
    print("sign_macos: notarization {0} ({1})".format(result.get("status"), result.get("id")))
    return 0


if __name__ == "__main__":
    sys.exit(main())
