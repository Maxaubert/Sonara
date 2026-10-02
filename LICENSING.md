# Licensing: bundling Sonara

Sonara is MIT-licensed (`LICENSE`). If you bundle it in your app (the runtime `sonarad.exe`, `@sonara/client`, `@sonara/runtime-win32-x64` or `sonara-client`), you may:

- **sell** your app, or give it away;
- **keep it closed-source**;
- **choose your own licence** for it;
- **code-sign** it, and the bundled `sonarad.exe` with it, under your own certificate.

**Your one obligation:** ship Sonara's licence notices with your app. That is `THIRD_PARTY_NOTICES.md` (it includes Sonara's own MIT notice through `LICENSE` and every third-party licence). The runtime zip and `@sonara/runtime-win32-x64` already contain both files, so copying the runtime folder into your app is enough. An "About" or "Open-source licences" screen that shows the file also works.

Why this holds (runtime spec decision R6):

- Every component that ships is under a permissive licence (MIT, Apache-2.0, BSD, ISC, Zlib, Unicode). Nothing GPL or otherwise copyleft ships: espeak-ng is not used.
- CI enforces it. `cargo deny check licenses bans` allows only those licences and bans the espeak and piper-phonemize crates; `THIRD_PARTY_NOTICES.md` is generated from `cargo metadata` by `packaging/notices/gen_notices.py` and CI fails when it is out of date.
- The JavaScript and Python clients have zero runtime dependencies.
- Models and data that are not code (the Kokoro voice model, the misaki lexicons, ONNX Runtime, the Visual C++ runtime) are listed by hand in `packaging/notices/models-and-data.md`, which the generated file includes.

Known gap, closed before the first commercial bundle: the notices credit each crate by its licence, authors and source, and carry one generic text per licence. They do not yet reproduce each MIT crate's own copyright line or licence file, nor the `NOTICE` file of an Apache-2.0 crate. The generator will collect those from the cargo registry sources.

This is a summary for developers, not legal advice. Bundling steps: `docs/bundling.md`.
