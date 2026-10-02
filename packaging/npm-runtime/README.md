# @sonara/runtime-win32-x64

The Sonara runtime (`sonarad.exe`) for Windows x64, packaged for apps that bundle it. Installs only on `win32`/`x64`. Use it with [`@sonara/client`](https://www.npmjs.com/package/@sonara/client).

```js
import { connect } from "@sonara/client";
import { runtimePath } from "@sonara/runtime-win32-x64";

const sonara = await connect({ clientName: "my-app", runtimePath: runtimePath() });
```

- `runtimePath()`: absolute path of `bin/sonarad.exe` (a path inside `app.asar` becomes `app.asar.unpacked`).
- `runtimeDir()`: the `bin` folder.

With electron-builder, copy `bin/` with `extraResources` and pass `path.join(process.resourcesPath, "sonara", "sonarad.exe")` in a packaged app; see [Bundle Sonara in your app](https://github.com/Maxaubert/Sonara/blob/main/docs/bundling.md).

## Licensing

MIT. You may sell, close-source, relicense and code-sign your app; ship `THIRD_PARTY_NOTICES.md` and `LICENSE` (both in this package) with it.

## Building this package (maintainers)

`npm run build` (also run by `npm pack`/`npm publish`) copies `target/release/sonarad.exe` (from `cargo build -p sonarad --release`), any `onnxruntime*.dll` next to it, `LICENSE` and `THIRD_PARTY_NOTICES.md` from the repository, and refuses a version that differs from the Cargo workspace.
