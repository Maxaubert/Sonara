## Models, data and Microsoft components

Hand-maintained (`packaging/notices/models-and-data.md`). Each entry says whether it ships in the runtime release today. Sources and licences checked on 2026-10-02 (runtime M4, #200).

### Kokoro-82M v1.0 voice model

- Source: hexgrad, https://huggingface.co/hexgrad/Kokoro-82M (model card licence: Apache-2.0). Full text below.
- Files: `kokoro-v1.0.onnx` (the fp32 ONNX export by taylorchu/kokoro-onnx, MIT, of the Apache-2.0 weights) and `voices-v1.0.bin` (the Kokoro v1.0 voice style vectors), from the release assets `model-files-v1.0` of https://github.com/thewh1teagle/kokoro-onnx (MIT). SHA-256 `7d5df8ecf7d4b1878015a32686053fd0eebe2bc377234608764cc0ef3636a6c5` and `bca610b8308e8d99f32e6fe4197e7ec01679264efed0cac9140fe9c29f1fbf7d`.
- Ships: no. The Kokoro engine downloads both files on first use into `%LOCALAPPDATA%\Sonara\models\kokoro\v1.0\` (SHA-256 pinned); a host may pre-seed that folder, and must then ship this notice.
- Ships in `sonarad.exe`: the model's phoneme vocabulary (the `vocab` table of Kokoro-82M's `config.json`, Apache-2.0).

### misaki G2P (vendored misaki-rs) and its English lexicons

- Code: misaki-rs 0.6.0 by Michele Yin, https://github.com/MicheleYin/misaki-rs, MIT (Copyright (c) 2026 Michele Yin), a Rust port of misaki. Vendored and trimmed as `crates/misaki` (US English only, compressed data, no espeak fallback).
- Lexicons: hexgrad, https://github.com/hexgrad/misaki, Apache-2.0. Attribution: the English pronunciation lexicons used by the Kokoro engine come from misaki by hexgrad.
- Data provenance: misaki-rs extended and corrected the US lexicon "using eSpeak" (its README): some entries are output of espeak-ng (GPL-3.0). No espeak-ng code or data ships with Sonara. This provenance is reviewed before the first commercial bundle (runtime spec, section 5).
- Part-of-speech tagger weights (inside misaki-rs): NLTK's `averaged_perceptron_tagger` model (https://github.com/nltk/nltk_data), as packaged by postagger.rs (Apache-2.0, https://github.com/shubham0204/postagger.rs); the tagger design comes from textblob-aptagger by Matthew Honnibal (MIT). The weights were trained on the Wall Street Journal part of the Penn Treebank; reviewed with the lexicon provenance before the first commercial bundle.
- Ships: yes, compiled into `sonarad.exe` (xz-compressed).

### Bundled event sounds

- Files: `crates/sonara-agent/sounds/{attention,reply-done,session-changed,navigate,edge}.wav`, the sound pack the user picked for #211 (2026-10-03), converted from MP3 to WAV.
- Source and licence: free sound sites; the redistribution licence is NOT yet confirmed. Confirm it (or replace the files by name) before a public release; until then this entry is the open item.
- Ships: yes, compiled into `sonarad.exe` (`crates/sonara-agent/build.rs`).

### Microsoft ONNX Runtime

- Source: Microsoft, https://github.com/microsoft/onnxruntime, the official CPU build `onnxruntime-win-x64-1.28.2.zip` (SHA-256 `c4eedd29489d5feca21866d054638416f3655bf6b18851b3b6b85c8313e95c35`).
- Licence: MIT, Copyright (c) Microsoft Corporation. Full text below. Its own third-party notices ship as `onnxruntime-ThirdPartyNotices.txt`.
- Ships: yes, as `onnxruntime.dll` next to `sonarad.exe`, with `onnxruntime-LICENSE.txt` and `onnxruntime-ThirdPartyNotices.txt` (`packaging/runtime_dlls.py`). `sonarad.exe` loads it at run time by its full path; without it the Kokoro engine is unavailable and Windows' voices speak.

### Microsoft Visual C++ runtime

- `sonarad.exe` and `sonara-hook.exe` link the C runtime statically (`+crt-static`) and need no Visual C++ redistributable; they use the Universal CRT that Windows 10 and 11 provide.
- `onnxruntime.dll` imports `msvcp140.dll`, `msvcp140_1.dll`, `vcruntime140.dll` and `vcruntime140_1.dll`. These ship app-locally next to it, copied from the Visual Studio redistributable folder (`VC\Redist\MSVC\<version>\x64\Microsoft.VC14x.CRT`). `runtime_dlls.py` refuses a `msvcp140.dll` or `vcruntime140.dll` older than 14.40.33810.0 (VS 2022 17.10, the oldest runtime ONNX Runtime 1.28.2 runs with) and prints the versions it stages; the shipped version is that of the build runner's newest Visual Studio (14.51.36247.0 on the development PC, 2026-10-02).
- Licence: Microsoft Visual C++ Redistributable, "Distributable Code" under the Microsoft Visual Studio licence terms, which allow app-local deployment of these files. A bundler may instead install the redistributable with its own installer.
- Ships: yes (the four DLLs above).

### Windows voices (OneCore)

- Windows' own speech (`Windows.Media.SpeechSynthesis`), part of the operating system: the zero-download voice and Kokoro's stand-in until its model is ready. Nothing ships.
