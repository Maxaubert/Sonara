# @sonara/client

Talk to the Sonara text-to-speech runtime from Node 18+ or an Electron main process. Zero dependencies; ESM and CommonJS builds with type declarations.

```js
import { connect } from "@sonara/client";
import { runtimePath } from "@sonara/runtime-win32-x64";

const sonara = await connect({ clientName: "my-app", runtimePath: runtimePath() });
sonara.onState((s) => console.log(s.now_playing?.text ?? "idle"));
const item = await sonara.speak("Hello.", { label: "greeting" });
await sonara.control("pause");
await sonara.set("rate", 240);
await sonara.close();
```

- `connect(options)`: finds the shared runtime through `runtime.json`, starts the bundled `sonarad.exe` when none is usable (`autostart`, default on), takes over an idle incompatible one. Options: `clientName` (required), `runtimePath` (default `SONARA_RUNTIME`), `home`, `autostart`, `require`, `extensions`, `keepAlive`.
- `speak(text, { mode: "append" | "replace", interrupt, label })` resolves with the item id.
- `control(action)`: `play`, `pause`, `toggle`, `stop`, `skip`, `previous`, `next`, `restart`, `mute`, `unmute`.
- `set(key, value)` / `get(key)` for `volume` (0..100), `rate` (100..400 wpm), `voice`, `engine`; `voices(engine?)`.
- `onState`, `onItem`, `onLog` return an unsubscribe function; events arrive on a second connection opened by the first listener. Every state listener, also one added later, gets the current state first. `onClose` fires when the runtime goes away.
- `channels`, `agent`, `system`: extension namespaces that send the protocol's extension messages as they are.
- Errors are `SonaraError` with a `code` (`E_BUSY`, `E_NOT_RUNNING`, ...).

Guide: [Bundle Sonara in your app](https://github.com/Maxaubert/Sonara/blob/main/docs/bundling.md). Protocol: [protocol v1](https://github.com/Maxaubert/Sonara/blob/main/docs/protocol-v1.md).

## Development

```sh
npm ci
npm run build       # tsc to dist/esm and dist/cjs
npm run test:unit   # against a fake runtime
npm test            # also against a real sonarad.exe (cargo build -p sonarad, or SONARAD=...)
```

MIT. Ship `THIRD_PARTY_NOTICES.md` with an app that bundles the runtime (see `LICENSING.md` in the repository).
