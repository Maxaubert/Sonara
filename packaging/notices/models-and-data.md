## Models, data and Microsoft components

Hand-maintained (`packaging/notices/models-and-data.md`). Each entry says whether it ships in the runtime release today.

### Kokoro-82M v1.0 voice model

- Source: hexgrad, https://huggingface.co/hexgrad/Kokoro-82M
- Licence: Apache-2.0 (the weights and voice files). Full text below.
- Ships: no. The Kokoro engine downloads the model on first use into `%LOCALAPPDATA%\Sonara\models\kokoro\<version>\` (SHA-256 pinned); a host may pre-seed that folder, and must then ship this notice. Not part of this release (the engine lands with runtime milestone M4).

### misaki English lexicons

- Source: hexgrad, https://github.com/hexgrad/misaki
- Licence: Apache-2.0. Attribution: the English pronunciation lexicons used by the Kokoro engine come from misaki by hexgrad.
- Data provenance: some lexicon entries were corrected with the output of espeak-ng (GPL-3.0); no espeak-ng code or data ships with Sonara. This provenance is reviewed before the first commercial bundle (runtime spec, section 5).
- Ships: no (with the Kokoro engine, M4).

### Microsoft ONNX Runtime

- Source: Microsoft, https://github.com/microsoft/onnxruntime (the official CPU build)
- Licence: MIT, Copyright (c) Microsoft Corporation. Full text below.
- Ships: no. It will ship as `onnxruntime.dll` next to `sonarad.exe` with the Kokoro engine (M4).

### Microsoft Visual C++ runtime

- `sonarad.exe` is built with MSVC and loads `vcruntime140.dll` and the Universal CRT (`api-ms-win-crt-*`), which Windows 10 and 11 provide.
- Licence: Microsoft Visual C++ Redistributable, "Distributable Code" under the Microsoft Visual Studio licence terms. A bundler may ship the redistributable (or `vcruntime140.dll` next to the exe) with its own installer, or rely on the copy already on the PC.
- Ships: no; the runtime zip does not include it.

### Windows voices (OneCore)

- The default voice is Windows' own speech (`Windows.Media.SpeechSynthesis`), part of the operating system. Nothing ships.
