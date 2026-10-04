# Bundle Sonara in your app

Sonara is a text-to-speech reader that apps ship inside themselves. Your users never install "Sonara": your app carries the runtime (`sonarad.exe`, Windows x64) and starts it when needed. Every app that bundles Sonara on a PC shares one running instance, so two apps never talk over each other.

What you need:

| you write | package | what it gives you |
|---|---|---|
| Node 18+ / Electron main | `@sonara/client` | `connect`, `speak`, `control`, `set`/`get`, `voices`, `onState`/`onItem`/`onLog`, `close` (zero dependencies) |
| a player UI (React or any framework) | `@sonara/player` | a headless `PlayerController` (state to view model, buttons to controls) and `<SonaraPlayer client={...} />` from `@sonara/player/react` (React as a peer only); see `clients/player/README.md` |
| Node 18+ / Electron | `@sonara/runtime-win32-x64` | `bin/` (`sonarad.exe` and the files next to it), the notices, `runtimePath()` |
| Python 3.9+ | `sonara-client` | the same API, standard library only |
| anything else | `sonara-runtime-win-x64-<version>.zip` (GitHub release, with `SHA256SUMS`) | `sonarad.exe`, the files next to it and the notices; speak protocol v1 over TCP or HTTP. The zip also holds `sonara-hook.exe` and `sonara.exe`, the Claude Code plugin's hook adapter and command line, which a host does not need |

The protocol behind all of them is `docs/protocol-v1.md`. The one licensing duty is to ship `THIRD_PARTY_NOTICES.md` (see [Licensing](#licensing)).

## What ships next to `sonarad.exe`

Keep these files in one folder (the package's `bin/`, the zip's top folder):

| file | why |
|---|---|
| `sonarad.exe` | the runtime (C runtime linked in: no Visual C++ redistributable needed for it) |
| `onnxruntime.dll`, `onnxruntime-LICENSE.txt`, `onnxruntime-ThirdPartyNotices.txt` | Microsoft's ONNX Runtime (CPU, MIT) for the Kokoro voice |
| `msvcp140.dll`, `msvcp140_1.dll`, `vcruntime140.dll`, `vcruntime140_1.dll` | the Visual C++ runtime that `onnxruntime.dll` needs (app-local copies) |

**Voices.** The runtime starts with Kokoro (neural, 28 English voices) when `onnxruntime.dll` is there, else with Windows' own voices. Kokoro's model (about 354 MB) is not in the package: it downloads on first use into `%LOCALAPPDATA%\Sonara\models\kokoro\v1.0\`, and Windows' voice reads meanwhile, so the first sentences never wait for it. `state.engine_status` shows the progress (draw it in your UI if you like). To ship the model with your installer instead, copy `kokoro-v1.0.onnx` and `voices-v1.0.bin` (the pinned files, see `THIRD_PARTY_NOTICES.md`) into that folder; nothing is downloaded then.

## How `connect()` finds the runtime

1. It reads `runtime.json` in the home folder (`%LOCALAPPDATA%\Sonara`, or `SONARA_HOME`, or the `home` option). If that process is alive, it connects and sends `hello`.
2. It uses that instance when it speaks protocol 1 and offers everything in `require`.
3. Otherwise it starts your bundled `sonarad.exe --home <home>` (no console window) and waits up to 5 s for it.
4. When the running instance is too old for you, it asks it to step down (a takeover). The instance agrees only when nothing is playing or queued; while it is busy the client retries for up to 30 s, then fails with `E_INCOMPATIBLE`.

A takeover does not compare versions: it runs whenever the running instance cannot serve you and you have a runtime to start. If your own bundled `sonarad.exe` lacks the capability you `require` too, an idle (possibly newer) shared instance is stopped, yours starts and `connect()` still fails with `E_INCOMPATIBLE`. So `require` only what your bundled runtime offers, and keep it current: two apps that bundle runtimes of different protocol majors will take the idle instance from each other in turn.

The runtime exits by itself 30 s after the last app disconnected. You never stop it yourself: other apps may be using it.

## Electron

```sh
npm install @sonara/client @sonara/runtime-win32-x64
```

electron-builder copies the runtime folder next to your app's resources (it must stay outside `app.asar` to be executable):

```json
"build": {
  "extraResources": [
    { "from": "node_modules/@sonara/runtime-win32-x64/bin", "to": "sonara" },
    { "from": "node_modules/@sonara/runtime-win32-x64/THIRD_PARTY_NOTICES.md", "to": "sonara/THIRD_PARTY_NOTICES.md" },
    { "from": "node_modules/@sonara/runtime-win32-x64/LICENSE", "to": "sonara/LICENSE" }
  ]
}
```

In the main process (CommonJS, so it runs on every Electron version; with an ESM main, use `import` and `import.meta.dirname`, Electron 29 or later):

```js
const { app, ipcMain, BrowserWindow } = require("electron");
const path = require("node:path");
const { connect } = require("@sonara/client");
const { runtimePath } = require("@sonara/runtime-win32-x64");

app.whenReady().then(async () => {
  const win = new BrowserWindow({ webPreferences: { preload: path.join(__dirname, "preload.js") } });
  const sonara = await connect({
    clientName: "my-app",
    runtimePath: app.isPackaged ? path.join(process.resourcesPath, "sonara", "sonarad.exe") : runtimePath(),
  });
  sonara.onState((state) => win.webContents.send("sonara:state", state)); // drive your player UI
  sonara.onLog((e) => console.log("sonara:", e.message)); // includes "event stream closed"
  ipcMain.handle("sonara:speak", (_e, text) => sonara.speak(text));
  ipcMain.handle("sonara:control", (_e, action) => sonara.control(action)); // play, pause, next...
  app.on("before-quit", () => sonara.close());
});
```

That is the whole integration: `speak(text, { mode: "replace", interrupt: true, label })` returns an item id, `control(action)` takes `play`, `pause`, `toggle`, `stop`, `skip`, `previous`, `next`, `restart`, `mute`, `unmute`, and `set("volume" | "rate" | "voice" | "engine", value)` changes settings. Each `state` event is a full snapshot (`now_playing`, `queued`, `paused`, `muted`, `volume`, `rate`, `voice`), so a player only renders the latest one. `require()` works too: the package ships CommonJS and ESM builds with type declarations.

Other Node hosts use `runtimePath()` as is. It points at `app.asar.unpacked` when the package sits inside an asar archive, so `asarUnpack: ["node_modules/@sonara/runtime-win32-x64/**"]` is an alternative to `extraResources`.

## Python

```sh
pip install sonara-client
```

```python
import sonara_client

with sonara_client.connect("my-app", runtime_path=r"C:\Program Files\MyApp\sonara\sonarad.exe") as sonara:
    item = sonara.speak("Build finished. Two warnings.", label="build")
    with sonara.subscribe(["items"]) as events:  # its own connection; also "state", "log"
        for e in events:
            if e["item_id"] == item and e["phase"] != "started":
                break
```

Ship `sonarad.exe` and the notices from the release zip with your app. `runtime_path` may also come from the `SONARA_RUNTIME` environment variable. `sonara.control("pause")`, `sonara.set("rate", 240)`, `sonara.voices()` mirror the Node API; errors are `sonara_client.SonaraError` with a `code`.

## curl (any language)

Without an SDK, read `runtime.json` and use HTTP. The runtime must already be running (start `sonarad.exe` yourself, or let an SDK do it).

```powershell
$rt = Get-Content "$env:LOCALAPPDATA\Sonara\runtime.json" | ConvertFrom-Json
curl.exe -s -H "Authorization: Bearer $($rt.token)" -d '{\"text\": \"Hello from curl.\"}' "http://127.0.0.1:$($rt.http_port)/v1/speak"
curl.exe -s -H "Authorization: Bearer $($rt.token)" -d '{\"action\": \"pause\"}' "http://127.0.0.1:$($rt.http_port)/v1/control"
curl.exe -sN -H "Authorization: Bearer $($rt.token)" "http://127.0.0.1:$($rt.http_port)/v1/events?events=state,items"
```

Any language with sockets can use the TCP JSON-lines transport instead (`hello` with the token first); see `docs/protocol-v1.md`.

## Errors

Every failure is a `SonaraError` with a `code`. The runtime's codes (`E_BAD_REQUEST`, `E_BUSY`, `E_NOT_FOUND`, `E_UNSUPPORTED`...) are listed in `docs/protocol-v1.md`. The clients add the first three and use `E_INCOMPATIBLE` for a takeover that did not work out:

| code | when |
|---|---|
| `E_NOT_RUNNING` | no runtime is running and none could be started (`autostart: false`, or no `runtimePath`) |
| `E_START_FAILED` | the bundled runtime did not start, or wrote no `runtime.json` within 5 s |
| `E_CLOSED` | the connection closed before the reply (the runtime exited, a takeover, or `close()`) |
| `E_INCOMPATIBLE` | the running instance cannot serve you and stayed busy for 30 s, or your bundled runtime lacks what you `require` |

## Extensions

`channels`, `agent` and `system` (spec sections 4.2 to 4.4) are optional layers. Ask for them in `connect({ extensions: [...] })`; the clients expose them as `client.channels`, `client.agent` and `client.system`, which send the protocol messages as they are. The current runtime offers the core only, so these answer `E_UNSUPPORTED` and `client.info.unavailable` lists what you asked for and did not get.

## External engines

Protocol 1.2 lets the user add speech engines that are not part of Sonara (OpenAI, a local
OpenAI-compatible server) as profiles; the clients expose them as `client.engines` (`list`,
`add`, `remove`, `setKey`/`set_key`, `test`) and `client.voices(engine, { refresh })`. Text goes
to such an engine only once a client selects it with `set engine`; keys live in Windows Credential
Manager. A host that must not send text off the PC, or does not want the feature, starts the
runtime with `--no-external-engines`: the `engines` capability is then absent and every
`engine_*` message is `E_UNSUPPORTED`. Library hosts that embed the engine crates get the same
choice from the licence policy: `Registry::default()` refuses the `External` class.

## Licensing

Summary of `LICENSING.md`: you may sell your app, keep it closed-source, choose its licence and code-sign it (including the bundled `sonarad.exe`). Ship `THIRD_PARTY_NOTICES.md` and `LICENSE` with it; both are in the runtime package and the release zip. Everything that ships is under a permissive licence, CI enforces that (`cargo deny`), and the clients have no dependencies.

`sonarad.exe` links the C runtime statically. `onnxruntime.dll` needs the Microsoft Visual C++ runtime, whose DLLs ship next to it (Microsoft's redistributable terms allow app-local copies); an installer may install the redistributable instead.

## Publishing the packages (maintainers)

Releases are cut by `release.yml` on every push to `main`: it builds `sonarad.exe`, `sonara-hook.exe` and `sonara.exe` (release), stages `onnxruntime.dll` (fetched by URL, pinned SHA-256) and the VC++ runtime next to them (`packaging/runtime_dlls.py`), attaches `sonara-runtime-win-x64-<version>.zip` and its `SHA256SUMS` to the GitHub release (the Claude Code plugin installs that zip on first use) and stops there. Nothing is published to npm or PyPI automatically: there are no registry tokens yet. To publish by hand from a checkout of the release tag (the version is already the same in every manifest; `tests/test_manifests.py` checks it):

```sh
cargo build -p sonarad --release
python packaging/runtime_dlls.py stage target/release
python packaging/notices/gen_notices.py --check

cd clients/ts && npm ci && npm run build && npm test && npm publish --access public
cd ../../packaging/npm-runtime && npm run build && npm publish --access public   # prepack copies sonarad.exe, the files next to it and the notices

cd ../../clients/python && python -m pip install build twine && python -m build && python -m twine upload dist/*
```

`release.yml` also holds a `publish-packages` job that does the same once `NPM_TOKEN` and `PYPI_TOKEN` secrets exist; it is skipped while they are missing.
