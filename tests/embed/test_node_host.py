"""A Node app on the published npm packages (#274): ``@sonara/client`` and
``@sonara/runtime-win32-x64`` are packed (as ``npm publish`` would ship
them), installed into an empty project, and ``hosts/node_host.mjs`` runs
there. The runtime it starts is the one inside the installed package
(``node_modules/@sonara/runtime-win32-x64/bin``), so the real-voice test
also proves ``onnxruntime.dll`` and the VC++ DLLs load from there."""
from __future__ import annotations

import json
import subprocess
from pathlib import Path

import pytest

from . import support
from . import npm_app
from .conftest import prepare_voice_home, real_engine_or_skip

NODE = npm_app.NODE
pytestmark = pytest.mark.skipif(not (npm_app.NODE and npm_app.NPM), reason="node and npm are needed")


def test_the_packed_packages_ship_what_an_app_needs(node_app):
    """The tarballs as published: ESM, CommonJS and types through
    ``exports``, the runtime and its DLLs, the notices."""
    client = node_app / "node_modules" / "@sonara" / "client"
    for f in ("dist/esm/index.js", "dist/esm/index.d.ts", "dist/cjs/index.js", "dist/cjs/index.d.ts",
              "LICENSE", "README.md", "package.json"):
        assert (client / f).is_file(), f
    assert not (client / "src").exists() and not (client / "test").exists()
    rt = node_app / "node_modules" / "@sonara" / "runtime-win32-x64"
    for f in ("bin/sonarad.exe", "index.js", "index.d.ts", "LICENSE", "THIRD_PARTY_NOTICES.md"):
        assert (rt / f).is_file(), f
    # require() and import both resolve, to the same version.
    probe = ("const c=require('@sonara/client');const r=require('@sonara/runtime-win32-x64');"
             "import('@sonara/client').then(m=>console.log(JSON.stringify("
             "{cjs:typeof c.connect,esm:typeof m.connect,rt:r.runtimePath()})))")
    out = subprocess.run([NODE, "-e", probe], cwd=str(node_app), capture_output=True, text=True, check=True).stdout
    got = json.loads(out)
    assert got["cjs"] == got["esm"] == "function"
    assert Path(got["rt"]) == rt / "bin" / "sonarad.exe"


def test_a_node_app_uses_the_whole_api_and_its_audio_is_real(node_app, work, provider):
    wav_dir = work / "wav"
    report = support.run_host(
        [NODE, "host.mjs"],
        support.host_env(work / "home", support.scenario_args(wav_dir), "scenario", provider),
        node_app)
    assert report["version"]
    support.check_scenario(report, wav_dir, provider)
    # engine_test with play: the preview went to the output as a clip.
    clips = [support.read_wav(p) for p in sorted(wav_dir.glob("*-clip.wav"))]
    assert any(c.rate == support.PROVIDER_RATE and set(c.samples) == {support.PROVIDER_VALUE} for c in clips)


@pytest.mark.parametrize("engine", ["kokoro", "onecore"])
def test_a_node_app_speaks_with_a_real_voice_from_node_modules(node_app, work, engine):
    bin_dir = node_app / "node_modules" / "@sonara" / "runtime-win32-x64" / "bin"
    real_engine_or_skip(engine, bin_dir)
    home = work / "home"
    prepare_voice_home(engine, home)
    wav_dir = work / "wav"
    report = support.run_host([NODE, "host.mjs"], support.host_env(home, support.voice_args(engine, wav_dir), "voice"),
                              node_app)
    if report.get("unavailable"):
        pytest.skip(f"{engine} cannot speak on this PC: {report['engine_status']}")
    support.check_real_voice(report, wav_dir, engine)
