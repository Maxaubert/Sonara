# M0: Kokoro engine and audio spike (issue #169)

Date: 2026-10-02. Spec: `docs/plans/2026-10-02-sonara-runtime-spec.md`. Plan: `docs/plans/2026-10-02-sonara-runtime-plan.md` (M0).

## Summary

| | Candidate A: ort + misaki-rs (no espeak) | Candidate B: sherpa-onnx C API |
|---|---|---|
| Status | Works: speaks the whole corpus | **Blocked until sherpa-onnx 2.0 (#3731)** |
| 1. No GPL crate or DLL | Pass for GPL: no espeak or piper crate, DLL or code. `cargo deny` licences flags one non-GPL crate (`language-tokenizer`, WTFPL), see Licences | Fail: espeak-ng is linked statically into `sherpa-onnx-c-api.dll` |
| 2. First audio < 1.0 s, RTF < 0.6 | Pass: 0.53 to 0.64 s, RTF 0.09 to 0.10 | not measured |
| 3. Runtime + engine < 60 MB | Pass with official CPU onnxruntime.dll: 53.7 MB. Fails with ort's default DirectML download: 77.6 MB | not measured |
| 4. User A/B (>= 7 of 10) | replaced by the user's hands-on test at cutover (see Decision) | n/a |

**Recommended stack (adopted, see Decision):**
- `ort` pinned to `=2.0.0-rc.13`, without `download-binaries`, linked to Microsoft's official `onnxruntime-win-x64-1.28.2` CPU build (MIT). Ship `onnxruntime.dll` next to `sonarad.exe`.
- G2P: a vendored fork of `misaki-rs` 0.6.0 (MIT) with `default-features = false`. The fork:
  - keeps the US English lexicon only, compressed;
  - replaces `language-tokenizer` (WTFPL) with an MIT/BSD snowball stemmer;
  - maps misaki-rs's zero-width-joiner diphthongs to Kokoro's single symbols.
- Sonara text rules for code, plus the permissive fallback for unknown words (both in the spike code).
- `rodio` 0.21 (cpal/WASAPI) for output.

## Setup

- PC: Windows 11 Pro 26200, 32 logical cores, Rust 1.99.0 (MSVC), cargo-deny 0.20.2.
- Model: Kokoro v1.0 `kokoro-v1.0.onnx` + `voices-v1.0.bin`, copied from `~/.sonara/kokoro` (unchanged). Voice `af_sarah`, speed 1.0, 24 kHz.
- Corpus (13 lines):
  - 10 short sentences that stress Claude output: `get_user_id`, `SessionChannel`, `src/sonara/daemon/ingest.py`, CI/PR/API/JSON/XML, 3.14, v0.8.5, a URL, a list item `1:`, `Ctrl+Alt+M`, `%LOCALAPPDATA%`, `@sonara/client`, `connect()`, `onnxruntime.dll`, `cargo-deny`.
  - 3 real assistant replies from `~/.sonara/session_digests.json` (read-only; 90 to 110 s of speech each; GUIDs, nvlddmkm, dxgkrnl, afd.sys, DPCs, 28th, Python 3.14, install.json).
- Every engine got the same input: each line passed through Sonara's `normalize_for_speech`, as the daemon does today.
- Every WAV got the same loudness step (`normalize_rms`, #81).
- Spike code is throwaway and stayed outside the repo (scratch Cargo projects `kokoro-ort`, `kokoro-ort-cpu`).

## Baseline: today's Python Kokoro

`kokoro-onnx` 0.4.7 + onnxruntime 1.24.4 on system Python 3.14.7. G2P is espeak-ng through `phonemizer-fork` 3.3.1 (GPL-3.0) and `espeakng-loader`. Today's product is therefore GPL-backed.

| metric | run 1 | run 2 |
|---|---|---|
| model load | 0.586 s | 0.558 s |
| first call ("Ready.") | 0.283 s | 0.286 s |
| one sentence (line 3), 3 runs | 0.753 / 0.754 / 0.749 s | 0.727 / 0.735 / 0.777 s |
| corpus: synth / audio / RTF | 42.49 / 362.99 s / 0.117 | 39.37 / 363.11 s / 0.108 |
| peak working set | 1600 MB | 1365 MB |
| long replies (whole message, no streaming) | | 9.5 to 11.5 s each |

## Candidate A: ort 2.0.0-rc.13 + misaki-rs 0.6.0 (no espeak)

### Pipeline
1. `normalize_for_speech` (today's rules).
2. Spike text rules:
   - `v0.8.5` becomes "version 0 point 8 point 5"; `3.14` becomes "3 point one four"; `28th` becomes "twenty eighth".
   - `IDs` becomes "ID's"; long all-caps names are lowercased (SONARA, LOCALAPPDATA).
   - `%VAR%`, `@scope/`, `fn()`, `--flag`, `Ctrl+`.
   - `name.ext` becomes "name dot ext"; path separators become "slash"; CamelCase is split; `onnx` becomes "onyx".
3. misaki-rs G2P (lexicon + POS tagger + stemming + num2words).
4. Fallback for words not in the lexicon, through misaki-rs's `Fallback` trait:
   - all caps of 5 letters or fewer: spell the letters;
   - otherwise split into 2 or 3 lexicon words (fewest parts, common words first): `localappdata` becomes local+app+data, `sonarad` becomes sonar+ad;
   - otherwise a small letter-to-sound rule set for pronounceable words;
   - otherwise spell the letters.
5. Zero-width-joiner diphthongs are mapped to Kokoro symbols (`a‍ɪ` to `I`, `e‍ɪ` to `A`, `o‍ʊ` to `O`, `a‍ʊ` to `W`, `ɔ‍ɪ` to `Y`, `t‍ʃ` to `ʧ`, `d‍ʒ` to `ʤ`).
6. Batches of at most 510 tokens; style vector `voice[len(tokens)]`; inputs `tokens`, `style`, `speed`.
7. Trim silence at -60 dB.

### Numbers

| metric | A1: ort default (pyke ORT 1.28.0, static, DirectML) | A2: official ORT 1.28.2 CPU dll, run 1 | A2 run 2 |
|---|---|---|---|
| model + G2P load | 0.753 s | 0.872 s | 0.752 s |
| warm-up first call | 0.133 s | 0.157 s | 0.154 s |
| first audio, one sentence (3 runs) | 0.500 / 0.518 / 0.505 s | 0.603 / 0.642 / 0.571 s | 0.557 / 0.559 / 0.525 s |
| corpus: synth / audio / RTF | 31.99 / 365.97 s / **0.087** | 37.87 / 365.97 s / **0.103** | 33.81 s / **0.092** |
| long replies: first audio (first 510-token batch) | 2.2 to 3.1 s | 2.4 to 3.4 s | |
| peak working set | 1361 MB | 1369 MB | 1362 MB |
| release exe | 59.05 MB | 37.84 MB | |
| DLLs | DirectML.dll 18.53 MB | onnxruntime.dll 15.82 MB + onnxruntime_providers_shared.dll 0.02 MB | |
| **total without model** | **77.6 MB (fails < 60)** | **53.7 MB (passes)** | |

**Notes**
- **Long replies:** the first audio of 2 to 3 s comes from 510-token batching inside one message. Sonara's assembler already sends sentence chunks, so the one-sentence figure is the relevant one. M4 should batch at sentence ends.
- **Size:**
  - ort's only Windows prebuilt download is a DirectML build. It links `DirectML.dll` and `d3d12.dll` as hard imports, so A1 cannot drop them.
  - 35.5 MB of the A2 exe is data embedded by misaki-rs: US + GB lexicon JSON (29.8 MB) and the tagger weights (5.7 MB).
  - With US-only data compressed (xz: about 2.6 MB for the lexicons), the exe would be about 5 to 8 MB. Runtime + engine would be about 22 to 25 MB.
- **Peak RAM** (about 1.36 GB) equals the Python baseline. It is ONNX Runtime arena growth on the fp32 model. Tuning arena and memory-pattern settings is an M4 item, not an M0 criterion.
- **VC++ runtime:** both the exe and onnxruntime.dll import `MSVCP140`/`VCRUNTIME140`. The M8 bundle must ship the VC++ runtime DLLs (redistributable) or require them.
- **System32 DLL name clash:** Windows ships its own `onnxruntime.dll` in System32. The app-directory DLL wins the search order for `sonarad.exe`. Keep the DLL next to the exe.

### Intelligibility check (automatic, not a quality rating)

whisper.cpp `ggml-base` (ffmpeg 9.0 `whisper` filter) transcribed every WAV. WER is measured against the spoken-normalized line, on the same reference for both engines.

| line | content | WER Python baseline | WER candidate A |
|---|---|---|---|
| 1 | get_user_id, SessionChannel | 0.13 | 0.13 |
| 2 | src/sonara/daemon/ingest.py | 0.38 | 0.38 |
| 3 | CI PR API JSON XML | 0.00 | 0.00 |
| 4 | 3.14, v0.8.5, 1689 | 0.22 | 0.33 |
| 5 | URL ("link") | 0.00 | 0.00 |
| 6 | 1:, --reset, Ctrl+Alt+M | 0.12 | 0.19 |
| 7 | KokoroEngine, WASAPI, cpal | 0.31 | 0.39 |
| 8 | SONARA_HOME, %LOCALAPPDATA%, sonarad.exe | 0.69 | 0.39 |
| 9 | @sonara/client, connect(), onState() | 0.33 | 0.27 |
| 10 | onnxruntime.dll, cargo-deny, espeak-ng | 0.56 | 0.33 |
| 11 to 13 | real replies | 0.04 / 0.08 / 0.10 | 0.04 / 0.07 / 0.09 |
| **corpus** | | **0.101** | **0.092** |

Examples:
- Line 4: the baseline reads "three.fourteen" and "vee zero.eight.five"; A reads "three point one four" and "version zero point eight point five".
- Line 8: the baseline reads "percent local app data percent backslash".
- A's known weak spots:
  - all-caps words with no lexicon entry are spelled out (WASAPI is spelled by misaki itself before our rule lowercases it; nvlddmkm and dxgkrnl are spelled by both engines);
  - letter-to-sound guesses ("cpal" as "kpal");
  - "Sonara" comes out as "SUN-ar-uh" instead of "suh-NAR-uh". It needs a lexicon entry; M4 should add a small custom lexicon (Sonara, Kokoro, onnx, ...).

## Candidate B: sherpa-onnx C API fed with tokens

- **Archive checked:** `sherpa-onnx-v1.13.8-win-x64-shared-MD-Release-lib.tar.bz2` (release v1.13.8, 2026-09-10; sha256 `3c43d1efba780d7ae7f5d64cae08803dcecf417109a5f593684e536f3cbd5cf1`).
- **espeak-ng is linked statically into `sherpa-onnx-c-api.dll` (4.2 MB).**
  - The strings in the DLL include `Software\eSpeak NG`, `ESPEAK_DATA_PATH`, `espeak-ng-data`, "Invalid phoneme code %d" and "The espeak-ng libra...".
  - `dumpbin /dependents` shows only `onnxruntime.dll` and the CRT, because static code does not appear there. So a dependents check alone would miss it.
- **Build:** `CMakeLists.txt` (v1.13.8) has `include(espeak-ng-for-piper)` and `include(piper-phonemize)` whenever `SHERPA_ONNX_ENABLE_TTS` is on. There is no option to build TTS without espeak.
- **English Kokoro needs espeak-ng-data at runtime:**
  - `offline-tts-kokoro-model-config.cc` requires `data_dir` with `phontab`, `phonindex`, `phondata` and `intonations`.
  - `KokoroMultiLangLexicon` calls `InitEspeak(data_dir)` unconditionally and uses espeak-ng for words not in the lexicon.
- **The C API has no token or phoneme input.** It only offers `SherpaOnnxOfflineTtsGenerate*` with text.
- **Result:** "blocked until sherpa-onnx 2.0 (#3731)". Upstream issue k2-fsa/sherpa-onnx#3731, "Breaking Change: Remove espeak-ng and piper-phonemize dependency", is open (created 2026-07-08). Candidate B stopped here, as the plan says.

## Licence evidence (candidate A)

**`cargo deny check licenses bans`** (spec allowlist: MIT, Apache-2.0, BSD-2-Clause, BSD-3-Clause, ISC, Zlib, Unicode-3.0). Bans: espeak-rs, espeak-rs-sys, espeak-ng, espeak-ng-sys, espeakng, espeakng-sys, espeak-sys, piper-phonemize, piper-phonemize-sys.
- **bans ok** in both variants: no espeak or piper crate in the graph (`misaki-rs` with `default-features = false` drops `espeak-rs`).
- **licences: one rejection in A2**, `language-tokenizer` 0.1.0 (a misaki-rs dependency, used only for its snowball stemmer). Its licence is **WTFPL**, which is permissive and not GPL, but it is not on the allowlist.
  - Fix option 1: vendor misaki-rs and use an MIT/BSD stemmer (`waken_snowball` is already in the graph, BSD-3-Clause). Recommended.
  - Fix option 2: accept an explicit WTFPL exception for that crate.
- A1 also flags `webpki-root-certs` (CDLA-Permissive-2.0). It is only a build dependency of `ort-sys` `download-binaries`, never shipped, and A2 does not have it.
- All other crates in A2 are MIT, Apache-2.0, BSD-3-Clause, Zlib, Unicode-3.0 or Unlicense/MIT.

**Binaries (A2):**
- `dumpbin /dependents kokoro-ort-cpu.exe` lists only system and CRT DLLs plus `onnxruntime.dll`; no espeak.
- Strings search finds no "GNU General Public" in the exe, onnxruntime.dll or providers_shared.dll.
- "espeak" appears in the exe only as misaki-rs's `FallbackError::Espeak` display text and the spike's own label. No espeak code.
- "piper" matches are lexicon words.
- onnxruntime.dll (official 1.28.2, sha256 of the zip `c4eedd29489d5feca21866d054638416f3655bf6b18851b3b6b85c8313e95c35`) is MIT.

**Data provenance to review:**
- The misaki-rs lexicons derive from hexgrad/misaki (Apache-2.0); attribution is needed in THIRD_PARTY_NOTICES.
- The misaki-rs README says its dictionary was extended and corrected "using eSpeak". That is data produced by running espeak, not espeak code. GPL does not usually reach a program's output, but under R2 this should be a recorded decision.
- The POS tagger weights' origin is to be checked in M4.

## Audio output check (Task 0.4)

rodio 0.21.1 on cpal/WASAPI. Default device: 2 ch, 48 kHz, f32, default buffer. Candidate A PCM (8.79 s, line 2) at volume 0.1. Pause at 1 s, resume at 2 s, 3 runs. The test was verified through API state and sink position, not by ear.

| | run 1 | run 2 | run 3 |
|---|---|---|---|
| `is_paused()` after `pause()` | true | true | true |
| position stops advancing after `pause()` | 5.1 ms | 4.1 ms | 4.8 ms |
| position drift while paused (1 s to 2 s) | 0 | 0 | 0 |
| position advancing after `play()` | 4.9 ms | 4.0 ms | 5.1 ms |
| end time vs audio + 1 s pause (9.79 s) | 9.765 s | 9.764 s | 9.765 s |

- True pause and resume work and resume exactly where playback stopped.
- Source-side latency is at most about 5 ms (rodio's 5 ms periodic control check).
- Audible latency adds the WASAPI shared-mode buffer: the source ran about 20 ms ahead of the clock. That gives an estimated 10 to 25 ms total, not measured acoustically.

## Deviations from the plan

- The spike code lived in a scratch area (throwaway), not `spikes/`. The models were copied to the scratch area instead of `%LOCALAPPDATA%\Sonara\models\spike\`.
- The plan's ort setup ("`load-dynamic` off", default download) gives the DirectML build and fails the size criterion. The candidate therefore links (not load-dynamic) to Microsoft's CPU `onnxruntime.dll` through `ORT_LIB_LOCATION` + `ORT_PREFER_DYNAMIC_LINK`.
- Task 0.4 asks to "confirm audibly"; the agent verified through API state and timing instead. One hands-on listen is still worth doing in M3.

## Decision

- **Candidate A is adopted** (ort + a vendored misaki-rs, no espeak), with the official Microsoft CPU ONNX Runtime 1.28.2. Built in runtime milestone M4 (#200).
- **The A/B listening check is replaced by the user's hands-on test at cutover** (user decision, 2026-10-02): the user installs the build and judges the voice as an end user would, instead of rating 10 pairs. The pairs in `ab/` stayed in the scratch area and were not rated.
- **Candidate B (sherpa-onnx) is blocked until sherpa-onnx 2.0** (k2-fsa/sherpa-onnx#3731): its English Kokoro path links espeak-ng statically and needs espeak-ng-data.
- The espeak-assisted provenance of some misaki-rs lexicon entries and the origin of the POS tagger weights (NLTK's averaged perceptron tagger, trained on the Penn Treebank WSJ section) are recorded in `packaging/notices/models-and-data.md` for review before the first commercial bundle.

## M4 notes (2026-10-02)

Measured with the engine as built (`cargo test -p sonara-engine --release --test kokoro_live -- --ignored --nocapture`), same PC, voice `af_heart`, the M0 short lines plus the long reply of corpus line 12 at rate 250 (speed 1.25):

| ORT session options | first audio, one sentence | RTF | peak working set |
|---|---|---|---|
| arena off, memory patterns off | 1.18 to 1.22 s | 0.21 | 665 MB |
| arena on, memory patterns off (**chosen**) | 0.54 to 0.58 s | 0.10 | 698 MB short lines, 751 MB with the long reply |
| arena off, memory patterns on | 0.57 to 1.18 s | 0.21 | 666 MB |
| arena on, memory patterns on | 0.54 s | 0.10 | 747 MB |

- Sentence batches (one model call per sentence instead of 510-token batches) keep peak memory near 750 MB on the long reply, against 1.36 GB in M0.
- **fp16 model** (`kokoro-v1.0.fp16.onnx`, 177 MB): first audio 0.49 to 0.53 s, RTF 0.086, 560 MB peak, half the download. Whisper transcripts of both models were comparable, with one extra garble on fp16 (line 3). **fp32 is kept** (the model of today's Python reader, so the user's hands-on test compares like with like); fp16 is the obvious switch if download size or memory matter more later.
- Model load (verify against `verified.json` + load + G2P lexicons): about 1.0 s. First synthesis after load: 0.14 s.
- **Sizes** (release): `sonarad.exe` 11.6 MB (with the xz-compressed US lexicon and tagger, 3.6 MB), `onnxruntime.dll` 15.8 MB, VC++ runtime DLLs 0.9 MB: about 28 MB without the model.
- `onnxruntime_providers_shared.dll` is not needed for the CPU provider and does not ship.
- **Static CRT:** with `+crt-static`, `dumpbin /dependents` lists only Windows DLLs (kernel32, ntdll, ole32, oleaut32, combase, advapi32, bcrypt, crypt32, user32, ws2_32 and api-ms-win-core-*) for `sonarad.exe` and `sonara-hook.exe`; no `VCRUNTIME140`, `MSVCP140` or `api-ms-win-crt-*`. `onnxruntime.dll` still imports `MSVCP140`, `MSVCP140_1`, `VCRUNTIME140` and `VCRUNTIME140_1`, which ship next to it.
- **Download end to end:** a fresh temp home with `sonarad --output null`: the model (353.7 MB) downloaded in about 5.5 s over HTTPS, then verify, load and the first Kokoro sentence, with `engine_status` going `downloading` (progress) to `loading` to `ready`. On this PC OneCore lists no voices for a new program (D7), so the fallback could not speak meanwhile.
- `language-tokenizer` (WTFPL) turned out to be an unused dependency of misaki-rs: dropping it needed no replacement stemmer (misaki-rs stems with its own rules).
