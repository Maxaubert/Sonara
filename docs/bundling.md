# Bundle Sonara in your app

Sonara is a text-to-speech reader that apps ship inside themselves. Your users never install "Sonara": your app carries the runtime (`sonarad.exe`, Windows x64) and starts it when needed. Every app that bundles Sonara on a PC shares one running instance, so two apps never talk over each other.

What you need:

| you write | package | what it gives you |
|---|---|---|
| Node 18+ / Electron main | `@sonara/client` | `connect`, `speak`, `control`, `set`/`get`, `voices`, `onState`/`onItem`/`onLog`, `close` (zero dependencies) |
| a player UI (React or any framework) | `@sonara/player` (not on npm yet: build it from `clients/player`) | a headless `PlayerController` (state to view model, buttons to controls) and `<SonaraPlayer client={...} />` from `@sonara/player/react` (React as a peer only); see `clients/player/README.md` |
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
'{"text": "Hello from curl."}' | curl.exe -s -H "Authorization: Bearer $($rt.token)" --data-binary '@-' "http://127.0.0.1:$($rt.http_port)/v1/speak"
'{"action": "pause"}' | curl.exe -s -H "Authorization: Bearer $($rt.token)" --data-binary '@-' "http://127.0.0.1:$($rt.http_port)/v1/control"
curl.exe -sN -H "Authorization: Bearer $($rt.token)" "http://127.0.0.1:$($rt.http_port)/v1/events?events=state,items"
```

The JSON goes to curl on stdin (`--data-binary '@-'`): PowerShell 7.3 and later pass quotes in
arguments to programs as they are, so the older `-d '{\"text\": ...}'` form sends invalid JSON
there (`E_BAD_REQUEST`), while Windows PowerShell 5.1 needs it. Piping works in both. With a
UTF-8 `$OutputEncoding`, Windows PowerShell 5.1 pipes a byte order mark (or two) first; the runtime
ignores it (since 0.21.5).

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

`channels`, `agent` and `system` (spec sections 4.2 to 4.4) are optional layers. Ask for them in `connect({ extensions: [...] })`; the clients expose them as `client.channels`, `client.agent` and `client.system`, which send the protocol messages as they are. The runtime offers all three (`runtime.json` lists them in `extensions`); an extension is enabled for the whole runtime once any client asks for it, and until then its messages answer `E_UNSUPPORTED`. `client.info.unavailable` lists what you asked for and the runtime does not offer. `system` acts on the PC (ducks or pauses other apps, holds global hotkeys) while a client that asked for it is connected, so ask for it only when your app wants that (see `docs/protocol-v1.md`).

## External engines

Protocol 1.2 lets the user add speech engines that are not part of Sonara (OpenAI, a local
OpenAI-compatible server; since 0.16.0 also ElevenLabs, Azure AI Speech and Google Cloud
Text-to-Speech; since 0.17.0 Cartesia, Deepgram and `command`, a speech program of the user's own
that the runtime starts with no shell; since 0.19.0 Gemini) as profiles; the clients expose them as `client.engines` (`list`,
`add`, `remove`, `setKey`/`set_key`, `test`, `reload`, and since 0.19.0 `models`) and
`client.voices(engine, { refresh })`. Sonara names no model or voice of its own (protocol 1.5):
an app offers the provider's lists (`models`, `voices`) and lets the user pick.
A `command` engine is never added through a client: `add` of one is `E_FORBIDDEN` (protocol
1.3); the user adds it with `sonara engines add <id> --kind command` or in `engines.json`. Text goes
to such an engine only once a client selects it with `set engine`; keys live in Windows Credential
Manager. A host that must not send text off the PC, or does not want the feature, starts the
runtime with `--no-external-engines` (which also keeps the runtime from starting a program
through a `command` profile the user configured): the `engines` capability is then absent and every
`engine_*` message is `E_UNSUPPORTED`. Library hosts that embed the engine crates get the same
choice from the licence policy: `Registry::default()` refuses the `External` class.

## Licensing

Summary of `LICENSING.md`: you may sell your app, keep it closed-source, choose its licence and code-sign it (including the bundled `sonarad.exe`). Ship `THIRD_PARTY_NOTICES.md` and `LICENSE` with it; both are in the runtime package and the release zip. Everything that ships is under a permissive licence, CI enforces that (`cargo deny`), and the clients have no dependencies.

`sonarad.exe` links the C runtime statically. `onnxruntime.dll` needs the Microsoft Visual C++ runtime, whose DLLs ship next to it (Microsoft's redistributable terms allow app-local copies); an installer may install the redistributable instead.

## Publishing the packages (maintainers)

Releases are cut by `release.yml` once CI (`ci.yml`) passed on a push to `main` (#250): it builds `sonarad.exe`, `sonara-hook.exe` and `sonara.exe` (release), stages `onnxruntime.dll` (fetched by URL, pinned SHA-256) and the VC++ runtime next to them (`packaging/runtime_dlls.py`) and attaches `sonara-runtime-win-x64-<version>.zip` and its `SHA256SUMS` to the GitHub release (the Claude Code plugin installs that zip on first use).

The same run then publishes the SDKs (#279) by trusted publishing (OIDC): no registry token exists anywhere. `sonara-client` goes to PyPI, `@sonara/client` and `@sonara/runtime-win32-x64` to npm; `@sonara/player` is private and stays unpublished. The `packages` job builds them with `python packaging/build_packages.py`, the script the CI `clients` job runs on every PR (npm tarballs packed as `tests/embed` packs them and checked for the files an app needs, `npm publish --dry-run`, the wheel and sdist through `twine check --strict`). The jobs `publish-npm` (environment `npm`) and `publish-pypi` (environment `pypi`) upload them; each skips a version the registry already has, so a rerun passes.

Two repository variables switch it on, one per registry. While a variable is unset (or anything but `true`) its job is skipped and the release succeeds as before:

| variable | publishes |
|---|---|
| `PUBLISH_PYPI` | `sonara-client` |
| `PUBLISH_NPM` | `@sonara/client`, `@sonara/runtime-win32-x64` |

`release.yml` started by hand (Actions, Run workflow, on `main`) with **packages_only** ticked publishes the packages of the existing release `v<version>` (built from its tag) without cutting a new one.

### One-time setup: PyPI

1. Sign in at pypi.org (an account with 2FA), open **Your account, Publishing** (`https://pypi.org/manage/account/publishing/`) and add a GitHub pending publisher:

   | field | value |
   |---|---|
   | PyPI Project Name | `sonara-client` |
   | Owner | `Maxaubert` |
   | Repository name | `Sonara` |
   | Workflow name | `release.yml` |
   | Environment name | `pypi` |

   A pending publisher does not reserve the name until the first upload, so do steps 2 and 3 soon after.
2. `gh variable set PUBLISH_PYPI --body true -R Maxaubert/Sonara`
3. Publish: the next release does it, or run `release.yml` by hand with **packages_only** for the current one. The first upload creates the project, and the pending publisher becomes its trusted publisher.

### One-time setup: npm

npm configures a trusted publisher only on a package that already exists, and a new trusted publisher must publish successfully within 2 days or it expires (delete it and add it again then). So:

1. Sign in at npmjs.com (2FA on), and create the organization `sonara` (free plan, public packages): the scope of `@sonara/...`.
2. Create both packages with a placeholder `0.0.0` from your PC, outside the repository (`npm login` first; Git Bash):

   ```sh
   for name in @sonara/client @sonara/runtime-win32-x64; do
     dir=$(mktemp -d)
     node -e 'require("fs").writeFileSync(process.argv[1] + "/package.json", JSON.stringify({ name: process.argv[2], version: "0.0.0", description: "Placeholder; releases are published from GitHub Actions.", license: "MIT", repository: { type: "git", url: "git+https://github.com/Maxaubert/Sonara.git" } }, null, 2))' "$dir" "$name"
     (cd "$dir" && npm publish --access public)
   done
   ```

3. For each package, **Settings, Trusted publishing**, add GitHub Actions:

   | field | value |
   |---|---|
   | Organization or user | `Maxaubert` |
   | Repository | `Sonara` |
   | Workflow filename | `release.yml` |
   | Environment name | `npm` |

   Or with npm 11.15.0 or later: `npm trust github <package> --repo Maxaubert/Sonara --file release.yml --env npm --allow-publish`.
4. `gh variable set PUBLISH_NPM --body true -R Maxaubert/Sonara`, then within the 2 days run `release.yml` by hand with **packages_only**: it publishes the current release through OIDC (with provenance), which validates both trusted publishers.
5. Mark the placeholders: `npm deprecate @sonara/client@0.0.0 "placeholder, install the latest version"` (and the same for `@sonara/runtime-win32-x64`). Then, per package, **Settings, Publishing access**: "Require two-factor authentication and disallow tokens" (trusted publishing still works).

### Notes

- The GitHub environments `pypi` and `npm` are created by the first run that uses them. Adding required reviewers to one (Settings, Environments) makes each release wait for an approval before that upload.
- Both registries check the workflow that holds the publish step: that is `release.yml` itself, also when it runs from `workflow_run` or by hand (it is not a reusable workflow). Renaming the file breaks publishing until the trusted publishers name the new one.
- To check the packages locally: `cargo build -p sonarad --release; python packaging/runtime_dlls.py stage target/release; (cd clients/ts && npm ci); python -m pip install --group dev; python packaging/build_packages.py --publish-dry-run` (output in `target/packages`).
