# Sonara runtime Implementation Plan

> **For agentic workers:** execute milestone by milestone with the Workflow tool (one implementer per PR in its own worktree, an independent reviewer, then a finalize step), as in Phase 0. Steps use checkbox (`- [ ]`) syntax for tracking. Milestones M2 onward are specified here by deliverables, interfaces and tests; their bite-sized steps are written by the executor at the start of each milestone, inside the milestone's PR, from this plan and the spec. A milestone plan needs no separate sign-off unless it deviates from the spec.

**Goal:** Turn Sonara into a layered Rust reader that apps bundle (PrismTerminal first): a minimal reader core with optional channels, agent, system, adapter and UI layers, served in-process or by a shared `sonarad.exe`; then move the Claude Code plugin onto it and retire the Python daemon.

**Architecture:** L1 reader core (`sonara-core` pure state machine and text rules, `sonara-engine`, `sonara-audio`, facade `sonara-reader`) knows only items, chunks and playback controls. L2 `sonara-channels`, L3 `sonara-agent`, L4 `sonara-system`, L5 `sonara-hook`, L6 `@sonara/player` each build only on the public API below them. `sonarad.exe` hosts L1 plus enabled layers and serves protocol v1 (a small core plus extensions) to thin MIT clients over loopback TCP JSON lines or HTTP+SSE.

**Tech Stack:** Rust stable (MSVC x64), `regex` (backtracking rules as linear scanners), `serde`/`serde_json`, `tokio` (server), `windows` crate (OneCore, WASAPI session volume, GSMTC), `rodio`/`cpal` (output), `ort` 2.0 rc with Microsoft's official CPU ONNX Runtime and a vendored `misaki-rs` (per M0), `cargo-deny`; TypeScript (Node 18+, zero deps) for `@sonara/client` and `@sonara/player`; Python 3.9+ stdlib for `sonara-client` and the pytest conformance suite.

**Spec:** `docs/plans/2026-10-02-sonara-runtime-spec.md`

## Global Constraints

- Windows x64 only; build target `x86_64-pc-windows-msvc`.
- R6: every shipped component is permissively licensed so bundlers may sell, close-source, relicense and sign their products; `cargo deny check licenses bans` passes in CI (allowlist MIT, Apache-2.0, BSD-2-Clause, BSD-3-Clause, ISC, Zlib, Unicode-3.0; banned `espeak*`, `piper-phonemize*`); JS and Python clients have zero runtime dependencies; `THIRD_PARTY_NOTICES.md` ships with every release.
- R7 layering: an L1 crate never depends on an L2+ crate (checked by a workspace test over `cargo metadata`); every layer uses only the public API of the layers below; the core protocol works with no extension enabled.
- Text rules must equal `tests/fixtures/text_rules/*.json` byte for byte.
- Protocol: JSON lines over `127.0.0.1` TCP, token required in `hello`; HTTP `POST /v1/<type>` with `Authorization: Bearer <token>`; events over SSE `GET /v1/events`. Never bind non-loopback addresses. Field names and error codes exactly as in spec section 4.
- Home `%LOCALAPPDATA%\Sonara` (override `SONARA_HOME`); every path goes through one `paths` module.
- Hotkeys, ducking/media pausing and the settings page exist only in L4 and run only when a client enables the `system` extension.
- Claude product rules (L2/L3): channel policy `latest` ("one message, always the last"); pause stays on when another channel gets a new turn; late text from a previous turn is dropped; never strand other apps ducked or paused. Default Claude hotkeys: Ctrl+Alt+Up (restart), Ctrl+Alt+Down (skip to end), Ctrl+Alt+M (mute cycle), Ctrl+Alt+P (next channel).
- Every PR: issue, `type/<issue>-slug` branch, version bump (Cargo workspace version = pyproject = plugin manifests), CI green, no em-dashes anywhere, `Co-Authored-By` trailer. User-facing milestones are installed on this PC before "merge?".
- The Python plugin stays the shipped product until cutover (M11).

## Review Focus

1. **Non-ASCII and multi-byte text** (emoji, CJK, curly quotes, `→`) in every text path: no panics on byte-index slicing, output equal to the Python rules. Owned by M1 (`unicode_text_never_panics`, `long_input_never_panics`).
2. **Rapid controls** (restart, previous/next, pause pressed many times quickly, as in the #128 report): no stuck pause, no lost or duplicated chunk. Owned by M2 (`rapid_controls_*` state-machine tests) and M5 conformance core.
3. **Kokoro model download interrupted, offline, or hash mismatch:** speech falls back to OneCore immediately, download resumes later, no retry storm. Owned by M4 (`model_download_*`).
4. **A second app bundling an older or newer runtime** while one is running: share, take over only when idle, or `E_INCOMPATIBLE`; never two instances fighting over audio. Owned by M5 (`takeover_*` conformance cases).
5. **Host crashes or is killed mid-utterance while other apps are ducked or paused:** restored on client disconnect and by the crash-restore file if the runtime dies. Owned by M8 (`restore_on_client_drop`, `restore_after_runtime_kill`).

---

## Milestones (one or more PRs each)

| M | Layer | Deliverable | Exit test |
|---|---|---|---|
| M0 | L1 | Engine and audio spike (throwaway code, report kept) | report answers the exit criteria; user blind A/B listening check passed |
| M1 | L1 | Rust toolchain, Cargo workspace, CI, `sonara-core` text rules | golden fixtures pass in Rust; CI green (PR #176) |
| M2 | L1 | `sonara-core` reader state machine: items, chunks, queue modes append/replace, controls, state and item events; layering test | state-machine tests incl. rapid controls; no I/O in the crate |
| M3 | L1 | `sonara-audio` output + `sonara-engine` trait + `onecore` engine | speaks via OneCore with true pause/resume, volume, rate |
| M4 | L1 | `kokoro` engine (M0 stack, vendored misaki-rs, custom lexicon) + model manager | GPL-free Kokoro speech; download/resume/fallback tests; `cargo deny` clean |
| M5 | L1 | `sonara-reader` in-process facade + `sonarad` serving core protocol v1: discovery, single instance, takeover, events; `docs/protocol-v1.md` (core); `conformance/core` | Rust example speaks in-process; conformance core green |
| M6 | L2 | `sonara-channels` + `channels` extension (protocol, conformance) | ported router/channel tests; `latest`/`queue` policies |
| M7 | L3 + L5 | `sonara-agent` (stream, turns incl. the #174 rule, ask, earcons, mute levels, summaries) + `agent` extension + `sonara-hook.exe` Claude adapter | parity conformance on both daemons |
| M8 | L4 | `sonara-system`: ducking, media pausing, crash restore, hotkeys, settings page; `system` extension | ported ducking/pausing tests; hands-on check |
| M9 | SDK | `@sonara/client` (core + extension namespaces), `@sonara/runtime-win32-x64`, `sonara-client`, release zip, `THIRD_PARTY_NOTICES.md`, "Bundle Sonara" guide | example Electron and Python hosts speak via the packages in CI |
| M10 | L6 | `@sonara/player`: headless `PlayerController` + React `<SonaraPlayer/>` on the core API | example app plays, pauses, steps chunks, mutes; component tests |
| M11 | product | Cutover: Claude plugin = `sonarad` + channels + agent + system + `sonara-hook`; config migration from `~/.sonara`; Python daemon removed | fresh install and upgrade from 0.8.x on this PC |
| M12 | host | PrismTerminal integration (PrismTerminal repo, own plan): bundled runtime, Audio mode, `@sonara/player`, text per tab | hands-on in PrismTerminal |

### M0 exit criteria (decide the Kokoro stack)

Measured on this PC, short English sentences and three real Claude replies with code identifiers, paths and acronyms:
1. No GPL crate or DLL in the candidate (`cargo deny`, and no `espeak` symbol in `dumpbin /dependents` output).
2. First audio for one sentence under 1.0 s after warm-up; RTF under 0.6.
3. Release size of runtime plus engine (without model) under 60 MB.
4. User A/B listening: 10 pairs (today's Python Kokoro vs candidate). The candidate is acceptable if the user rates it "same or better" on at least 7 of 10. If neither candidate passes, stop and ask the user (options: accept OneCore-only default for bundles; accept a GPL pack opt-in; continue tuning the fallback).

### Interface notes (binding for milestone plans)

- L1 text: `sonara_core::text::{clean_markdown, normalize_for_speech, stabilize_ordinals}` and `sonara_core::assembler::{ProseAssembler, Chunk}` (M1) are the only text entry points.
- L1 state machine (M2), pure: `sonara_core::reader::{Reader, ItemId, Item, QueueMode, Control, Effect, State, Event, AudioEvent}`; `Reader::speak(&mut self, text: &str, mode: QueueMode, interrupt: bool, label: Option<String>) -> (ItemId, Vec<Effect>)`, `Reader::control(&mut self, c: Control) -> Vec<Effect>`, `Reader::on_audio(&mut self, e: AudioEvent) -> Vec<Effect>`, `Reader::state(&self) -> State`. `Control` = `Play | Pause | Toggle | Stop | Skip | Previous | Next | Restart | Mute | Unmute`. `Effect` = `Synthesize{item, chunk, text} | PlayChunk{item, chunk, gen} | PauseOutput | ResumeOutput | StopOutput | Mute | Unmute | SetVolume(u8) | Emit(Event)`. As built in M2 (#178): `PlayChunk` carries a `gen` and `AudioEvent` = `ChunkStarted{gen} | ChunkFinished{gen} | Failed{gen, reason}`, so stale events from a superseded play are ignored; `Mute`/`Unmute` tell the output to go silent and back while playback keeps moving; setters `set_volume(u8)`, `set_rate(u32)`, `set_voice(Option<String>)`; `split_chunks(text)` gives an item's chunks. The full semantics are the `sonara_core::reader` module docs. No sessions, channels, turns or earcons in L1.
- L1 engine (M3): `sonara_engine::{Engine, EngineId, LicenseClass, Voice, PcmChunk}` with `trait Engine { fn id(&self) -> EngineId; fn license_class(&self) -> LicenseClass; fn voices(&self) -> Vec<Voice>; fn warm(&self) -> Result<()>; fn synthesize(&self, text: &str, voice: &str, rate: u32) -> Result<Box<dyn Iterator<Item = Result<PcmChunk>> + Send>>; fn cancel(&self); }`. As built in M3 (#179): `Engine: Send + Sync` (cancel comes from another thread), the iterator is aliased `PcmStream`; `LicenseClass` = `Permissive | Os` (no copyleft variant); `Voice {id, name, language, engine, license_class, installed}`; `PcmChunk {samples: Vec<i16>, sample_rate, channels}`; `Registry::{new(&[LicenseClass]), default() (Permissive + Os), permissive_only(), register -> Err(LicenseRefused), get(id), voices()}`; `onecore::OneCore` (default feature `onecore`) and `fake::FakeEngine` (feature `test-util`). `rate` is Sonara words per minute; OneCore maps it to `SpeakingRate = clamp(wpm / 200, 0.5, 6.0)` as the Python backend.
- L1 audio (M3): `sonara_audio::Output` with `play(chunks, ItemId, chunk_index)`, `pause()`, `resume()`, `stop()`, `set_volume(u8)`, `play_clip(&[i16], sample_rate)` (mixed, used by L3 earcons), and an event channel of `AudioEvent::{ChunkStarted, ChunkFinished, Failed}`. As built in M3 (#179): `Output` is a trait, `play(pcm: Vec<PcmChunk>, item: ItemId, chunk_index: usize, gen: u64)` takes the `gen` of the `PlayChunk` it answers (the events carry it, M2); `RodioOutput::new() -> (RodioOutput, Receiver<AudioEvent>)` (own thread, device opened on first play, device errors become `Failed`); `TestOutput` (feature `test-util`) records `OutputCall`s and sends events only when a test calls `start`/`finish`/`fail`. `Mute`/`Unmute` are carried out as `set_volume(0)` and back. The M3 single-threaded effect loop (`crates/sonara-audio/examples/driver/`) moved into the M5 facade (#182) and is gone from `sonara-audio`.
- OneCore on a PC with missing voice data (found in M3, 2026-10-02): OneCore keeps a per-program copy of its voice list (`HKCU\SOFTWARE\Microsoft\Speech_OneCore\Isolated\<hash>`). Programs that ran while the voices worked (the Python daemon's `python.exe`) still list David, Zira and Mark and fail with "file not found" (`Error::MissingVoiceData`); a new program such as `sonarad.exe` lists no voices at all and `SpeechSynthesizer::new()` fails with 0x80070002 (`Error::NoVoices`). Both errors name the D7 repair.
- L1 facade (M5): `sonara_reader::ReaderHandle::{new(Config), speak, control, set, get, voices, subscribe() -> Receiver<Event>}`; L2+ crates and `sonarad` use only this facade. As built in M5 (#182): `Config {registry, engine: Option<String>, voice, rate, volume, output}` (`Config::new(registry)`, `with_output(Box<dyn Output>, Receiver<AudioEvent>)`; no output opens `RodioOutput`), `default_registry()` (OneCore on Windows); `speak(text, mode, interrupt, label) -> Result<ItemId>`, `control(Control) -> Result<()>`, `set(Key, Value) -> Result<()>` / `get(Key) -> Result<Value>` with `Key::{Volume, Rate, Voice, Engine}` (`Key::parse` takes the protocol names) and `Value::{Number(u64), Text, Null}` (volume 0..=100, rate 100..=400 wpm, voice id or name of the current engine or `Null`; switching engine resets a voice the new engine lacks), `state()`, `voices(Option<&str>)`, `subscribe() -> Result<Receiver<Event>>` with `Event::{State, Item, Log{message}}` (unbounded per-subscriber channels: a slow subscriber never blocks, a dropped one is forgotten; hosts such as `sonarad` must drain or bound their own per-client queues and never hand a raw `Receiver` to a network client; a failed engine warm-up is replayed as the first `Log` to each new subscriber until the engine changes), `shutdown()` (also on dropping the last clone; later calls `Error::Closed`). One worker thread owns the `Reader`; synthesis runs on a second thread (a play waiting for its chunk moves it to the front; a chunk whose item ended is dropped or cancelled), and a synthesis failure is reported as `AudioEvent::Failed` at the chunk's `PlayChunk` plus an `Event::Log`, so `sonara-core` is unchanged. `say` is now `cargo run -p sonara-reader --example say`.
- L2 (M6): `sonara_channels::Channels` wraps a `ReaderHandle`; channels feed it with a `QueueMode` per policy and switch focus with `Control::Stop` then `speak`. L3 (M7) wraps `Channels`. L4 (M8) subscribes to L1 events to duck and restore, and maps hotkeys to `control`.
- Protocol message and event field names exactly as in spec section 4; error codes exactly as listed there.

---

## M0: Engine and audio spike (issue #169, branch `spike/169-kokoro-engine`)

**Files:**
- Create: `spikes/kokoro-ort/` and `spikes/kokoro-sherpa/` (throwaway Cargo projects, not workspace members, deleted after M4)
- Create: `docs/plans/2026-10-02-m0-engine-spike.md` (kept: results and decision)

### Task 0.1: Install the Rust toolchain

- [ ] **Step 1:** `winget install --id Rustlang.Rustup -e --silent`, then in a new shell `rustup default stable-x86_64-pc-windows-msvc` and `rustup component add clippy rustfmt`.
- [ ] **Step 2:** Verify: `cargo --version` and `rustc --version` print versions; `cargo new --bin %TEMP%\rs-smoke && cargo run --manifest-path %TEMP%\rs-smoke\Cargo.toml` prints `Hello, world!` (proves the MSVC linker from the installed Build Tools is found).
- [ ] **Step 3:** `cargo install cargo-deny --locked`. Verify `cargo deny --version`.

### Task 0.2: Candidate A, ort + misaki-rs (no espeak)

- [ ] **Step 1:** `cargo new spikes/kokoro-ort`; dependencies: `ort` (exact latest 2.0 rc, `load-dynamic` off), `misaki-rs` with `default-features = false`, `hound` (wav out), `ndarray`.
- [ ] **Step 2:** Write `main.rs` that loads `kokoro-v1.0.onnx` and `voices-v1.0.bin` from `%LOCALAPPDATA%\Sonara\models\spike\` (copy from `~/.sonara/kokoro/`), phonemizes a sentence with misaki-rs (unknown words: spell letters), maps phonemes to Kokoro token ids, runs the model with voice `af_sarah` at speed 1.0, writes `out/<n>.wav`, and prints `first_audio_ms`, `rtf`.
- [ ] **Step 3:** Run on the corpus (`spikes/corpus.txt`: 10 sentences plus three real Claude replies saved from `~/.sonara` history, code identifiers included). Record numbers.
- [ ] **Step 4:** `cargo deny check licenses` with the spec allowlist; `dumpbin /dependents` on the release exe. Record.

### Task 0.3: Candidate B, sherpa-onnx C API fed with tokens

- [ ] **Step 1:** Download the sherpa-onnx Windows x64 shared-library release built with `SHERPA_ONNX_ENABLE_TTS` using lexicon input, and check its dependencies for espeak (record the exact archive and whether `espeak-ng-data` is required for English Kokoro).
- [ ] **Step 2:** `spikes/kokoro-sherpa` calls the C API through `bindgen` or the published `sherpa-rs` crate if it supports token/lexicon input without espeak; same corpus, same measurements.
- [ ] **Step 3:** If English Kokoro in sherpa-onnx 1.x cannot run without espeak data, record "blocked until sherpa-onnx 2.0 (#3731)" and stop this candidate.

### Task 0.4: Audio output check

- [ ] **Step 1:** In candidate A, play the PCM through `rodio` (or `cpal`) with pause after 1 s and resume after 2 s; confirm audibly and record pause latency.

### Task 0.5: Listening check and decision

- [ ] **Step 1:** Produce 10 A/B pairs (`ab/01-old.wav`, `ab/01-new.wav` ...) from the Python daemon's Kokoro (today's quality) and the best candidate.
- [ ] **Step 2:** Ask the user to listen and rate each pair "new same or better" / "new worse" (AskUserQuestion, one question per batch of pairs).
- [ ] **Step 3:** Write `docs/plans/2026-10-02-m0-engine-spike.md` with all numbers, licence evidence, ratings and the chosen stack; commit it on the M0 branch; open the PR.

---

## M1: Toolchain, workspace, CI, text rules in Rust (issue #170, branch `feat/170-rust-core-text`)

**Files:**
- Create: `Cargo.toml` (workspace), `rust-toolchain.toml`, `deny.toml`
- Create: `crates/sonara-core/Cargo.toml`, `crates/sonara-core/src/lib.rs`, `crates/sonara-core/src/text.rs`, `crates/sonara-core/src/assembler.rs`
- Create: `crates/sonara-core/tests/golden.rs`
- Modify: `.github/workflows/ci.yml` (add a `rust` job)
- Modify: `CLAUDE.md` (Rust commands in the build block), `pyproject.toml` + `.claude-plugin/*.json` + `src/sonara/__init__.py` + `Cargo.toml` version bump to 0.9.0

**Interfaces:**
- Consumes: `tests/fixtures/text_rules/*.json` (format in `tests/fixtures/text_rules/README.md`).
- Produces: `sonara_core::text::{stabilize_ordinals(&str) -> String, clean_markdown(&str) -> String, normalize_for_speech(&str) -> String}`; `sonara_core::assembler::{ProseAssembler::new() -> Self, ProseAssembler::feed(&mut self, delta: &str, index: u32, is_final: bool) -> Vec<Chunk>}`, `enum Chunk { Text(String), ParagraphBreak }`.

### Task 1: Workspace skeleton and CI

- [ ] **Step 1: Create the workspace files**

`Cargo.toml`:
```toml
[workspace]
resolver = "2"
members = ["crates/sonara-core"]

[workspace.package]
version = "0.9.0"
edition = "2021"
license = "MIT"
repository = "https://github.com/Maxaubert/Sonara"

[workspace.dependencies]
regex = "1"
once_cell = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

`rust-toolchain.toml`:
```toml
[toolchain]
channel = "stable"
components = ["clippy", "rustfmt"]
targets = ["x86_64-pc-windows-msvc"]
```

`deny.toml`:
```toml
[graph]
targets = ["x86_64-pc-windows-msvc"]

[licenses]
allow = ["MIT", "Apache-2.0", "Apache-2.0 WITH LLVM-exception", "BSD-2-Clause", "BSD-3-Clause", "ISC", "Zlib", "Unicode-3.0", "Unicode-DFS-2016"]
confidence-threshold = 0.9

[bans]
deny = [{ name = "espeak-rs" }, { name = "espeak-ng-sys" }, { name = "piper-phonemize" }]
```

`crates/sonara-core/Cargo.toml`:
```toml
[package]
name = "sonara-core"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
regex.workspace = true
once_cell.workspace = true

[dev-dependencies]
serde.workspace = true
serde_json.workspace = true
```

`crates/sonara-core/src/lib.rs`:
```rust
//! Sonara reader core: pure text rules, sessions and playback state. No I/O.
pub mod assembler;
pub mod text;
```

- [ ] **Step 2: Add the CI job** to `.github/workflows/ci.yml` (keep the existing Python jobs; cargo-deny runs in its own Linux job because its action is Docker-based):

```yaml
  rust:
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy, rustfmt
      - uses: Swatinem/rust-cache@v2
      - run: cargo fmt --all -- --check
      - run: cargo clippy --workspace --all-targets -- -D warnings
      - run: cargo test --workspace

  # cargo-deny-action is a Docker action, which GitHub runs only on Linux
  # runners. deny.toml pins the graph to the Windows target, so Windows-only
  # dependencies are still checked from here.
  deny:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: EmbarkStudios/cargo-deny-action@v2
        with:
          command: check licenses bans
```

- [ ] **Step 3:** Create empty `text.rs` and `assembler.rs` (`// filled in Tasks 3-4`), run `cargo build`; expected success.
- [ ] **Step 4: Commit** `build(rust): cargo workspace, toolchain pin, cargo-deny, CI job (#170)`.

### Task 2: Golden fixture harness (failing)

- [ ] **Step 1: Write the harness** `crates/sonara-core/tests/golden.rs`:

```rust
use serde_json::Value;
use sonara_core::assembler::{Chunk, ProseAssembler};
use sonara_core::text::{clean_markdown, normalize_for_speech};
use std::path::PathBuf;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/text_rules")
}

fn assemble(deltas: &[String]) -> Vec<Value> {
    let mut a = ProseAssembler::new();
    let mut out = Vec::new();
    let last = deltas.len().saturating_sub(1);
    for (i, d) in deltas.iter().enumerate() {
        for c in a.feed(d, i as u32, i == last) {
            out.push(match c {
                Chunk::Text(s) => Value::String(s),
                Chunk::ParagraphBreak => Value::Null,
            });
        }
    }
    out
}

#[test]
fn every_golden_case_matches() {
    let mut failures = Vec::new();
    let mut count = 0;
    for entry in std::fs::read_dir(fixtures_dir()).expect("fixtures dir") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let data: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        for case in data["cases"].as_array().unwrap() {
            count += 1;
            let name = format!("{}::{}", path.file_stem().unwrap().to_string_lossy(), case["name"]);
            let ok = match case["fn"].as_str().unwrap() {
                "assemble" => {
                    let deltas: Vec<String> = serde_json::from_value(case["deltas"].clone()).unwrap();
                    Value::Array(assemble(&deltas)) == case["output"]
                }
                "clean_markdown" => clean_markdown(case["input"].as_str().unwrap()) == case["output"].as_str().unwrap(),
                "normalize_for_speech" => normalize_for_speech(case["input"].as_str().unwrap()) == case["output"].as_str().unwrap(),
                other => panic!("unknown fn {other}"),
            };
            if !ok {
                failures.push(name);
            }
        }
    }
    assert!(count > 0, "no golden cases found");
    assert!(failures.is_empty(), "failing cases: {failures:#?}");
}
```

- [ ] **Step 2:** Add stub signatures so it compiles: in `text.rs` `pub fn stabilize_ordinals(t: &str) -> String { t.to_string() }`, `pub fn clean_markdown(t: &str) -> String { t.to_string() }`, `pub fn normalize_for_speech(t: &str) -> String { t.to_string() }`; in `assembler.rs` `pub enum Chunk { Text(String), ParagraphBreak }`, `#[derive(Default)] pub struct ProseAssembler;` with `new()` and `feed()` returning `Vec::new()`.
- [ ] **Step 3:** Run `cargo test -p sonara-core --test golden`. Expected: FAIL listing failing cases.
- [ ] **Step 4: Commit** `test(core): golden text-rule harness over the shared fixtures (#170)`.

### Task 3: Port the cleaner (`text.rs`)

Python source of truth: `src/sonara/cleaner.py` (93 lines). Rust `regex` has no lookaround or backreferences; `_EMPHASIS` (backreference), `_BARE_URL` (lookahead) and `_SNAKE` (lookbehind and lookahead) are hand-written linear scanners, the rest use `regex`. (First drafted on `fancy-regex`; its `replace_all` panics past 1M backtracks, so long input such as 500 KB of plain words crashed the core. Covered by `long_input_never_panics`.)

- [ ] **Step 1: Write unit tests** at the bottom of `text.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinals_are_length_preserving() {
        let s = "1. a\n  22. b";
        assert_eq!(stabilize_ordinals(s), "1: a\n  22: b");
        assert_eq!(stabilize_ordinals(s).len(), s.len());
    }

    #[test]
    fn snake_case_and_arrows() {
        assert_eq!(normalize_for_speech("call get_user_id -> done & ok"), "call get user id to done and ok");
    }

    #[test]
    fn bare_url_keeps_terminator() {
        assert_eq!(clean_markdown("See https://x.y/z. Next."), "See link. Next.");
    }

    #[test]
    fn unicode_text_never_panics() {
        for s in ["→ 🚀 “quoted” 日本語 **bold** é", "_", "`", "1.", ""] {
            let _ = normalize_for_speech(s);
            let _ = clean_markdown(s);
        }
    }
}
```

- [ ] **Step 2:** Run `cargo test -p sonara-core text`. Expected: FAIL (stubs).
- [ ] **Step 3: Implement** `text.rs`:

```rust
//! Strip markdown noise and normalize symbols so text reads naturally aloud.
//! Port of src/sonara/cleaner.py; the golden fixtures are the contract.
use fancy_regex::Regex as FRegex;
use once_cell::sync::Lazy;
use regex::Regex;

static LINK: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[([^\]\n]+)\]\((?:[^)\n]+)\)").unwrap());
static INLINE_CODE: Lazy<Regex> = Lazy::new(|| Regex::new(r"`([^`\n]*)`").unwrap());
static HEADING: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)^#{1,6}\s+").unwrap());
static BULLET: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)^(\s*)[-*+•][ \t]+").unwrap());
static EMPHASIS: Lazy<[Regex; 6]> = Lazy::new(|| {
    // Python uses a backreference (\*{1,3}|_{1,3})([^*_\n]+)\1; Rust regex has no
    // backreferences, so each marker gets its own pattern, longest first.
    [r"\*\*\*([^*_\n]+)\*\*\*", r"___([^*_\n]+)___", r"\*\*([^*_\n]+)\*\*",
     r"__([^*_\n]+)__", r"\*([^*_\n]+)\*", r"_([^*_\n]+)_"]
        .map(|p| Regex::new(p).unwrap())
});
static BARE_URL: Lazy<FRegex> = Lazy::new(|| FRegex::new(r"https?://\S+?(?=[.,;:!?)\]]*(?:\s|$))").unwrap());
static TABLE_SEP: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)^\s*\|?[\s:|-]*-{3,}[\s:|-]*\|?\s*$").unwrap());
static LIST_ORDINAL: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)^(\s*)(\d{1,3})\.(\s+)").unwrap());
static LIST_ITEM_END: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)^(\s*\d{1,3}: .*[^\s.!?:;])[ \t]*\n").unwrap());
static WHITESPACE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").unwrap());
static SNAKE: Lazy<FRegex> = Lazy::new(|| {
    FRegex::new(r"(?<![A-Za-z0-9_])_{0,2}[A-Za-z][A-Za-z0-9]*(?:_[A-Za-z0-9]+)+(?![A-Za-z0-9_])").unwrap()
});
static ARROW: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s*(?:->|=>|-->|→)\s*").unwrap());
static STRAY_MD: Lazy<Regex> = Lazy::new(|| Regex::new(r"[*_`~#|•✓✔✗✘❌✅]+").unwrap());

/// Length-preserving "N. " -> "N: " for numbered list items (raw text, pre-split).
pub fn stabilize_ordinals(text: &str) -> String {
    LIST_ORDINAL.replace_all(text, "${1}${2}:${3}").into_owned()
}

fn emphasis_pass(text: &str) -> String {
    let mut s = text.to_string();
    for re in EMPHASIS.iter() {
        s = re.replace_all(&s, "${1}").into_owned();
    }
    s
}

pub fn clean_markdown(text: &str) -> String {
    let mut t = LINK.replace_all(text, "${1}").into_owned();
    t = INLINE_CODE.replace_all(&t, "${1}").into_owned();
    t = HEADING.replace_all(&t, "").into_owned();
    t = BULLET.replace_all(&t, "${1}").into_owned();
    t = emphasis_pass(&t);
    t = emphasis_pass(&t);
    t = BARE_URL.replace_all(&t, "link").into_owned();
    t = TABLE_SEP.replace_all(&t, " ").into_owned();
    t = stabilize_ordinals(&t);
    t = LIST_ITEM_END.replace_all(&t, "${1}.\n").into_owned();
    t = WHITESPACE.replace_all(&t, " ").into_owned();
    t.trim().to_string()
}

pub fn normalize_for_speech(text: &str) -> String {
    let t = SNAKE.replace_all(text, |c: &fancy_regex::Captures| c[0].replace('_', " ")).into_owned();
    let mut t = clean_markdown(&t);
    t = ARROW.replace_all(&t, " to ").into_owned();
    t = t.replace(" & ", " and ");
    t = STRAY_MD.replace_all(&t, " ").into_owned();
    WHITESPACE.replace_all(&t, " ").trim().to_string()
}
```

- [ ] **Step 4:** Run `cargo test -p sonara-core text` (expected PASS), then `cargo test -p sonara-core --test golden`. Expected: only `assemble` cases still fail. If a `clean_markdown` case fails, the difference is the emphasis translation: Python applies one alternation regex left to right per pass; compare the failing input against `python -c "from sonara.cleaner import clean_markdown; print(repr(clean_markdown(...)))"` and change `emphasis_pass` to scan left to right with the longest marker at each position (single combined `fancy-regex` `(\*{1,3}|_{1,3})([^*_\n]+)\1` is the exact equivalent and is acceptable).
- [ ] **Step 5: Commit** `feat(core): port the text cleaner to Rust (#170)`.

### Task 4: Port the prose assembler (`assembler.rs`)

Python source of truth: `src/sonara/assembler.py` (282 lines), every method ported one to one with the same names in snake_case. Port notes:
- Keep `buf`, `pending`, `emitted` as **byte** offsets on UTF-8 `String`s; every slice point is produced by `find`, regex match ends, or `len() - remainder.len()`, which are char boundaries. `stabilize_ordinals` is byte-length-preserving (`.` and `:` are one byte), so offsets stay valid exactly as in Python.
- `_SENTENCE = (.+?[.!?]["'’”)\]*_`]*)\s+` with DOTALL becomes `regex::Regex::new(r#"(?s)(.+?[.!?]["'’”)\]*_`]*)\s+"#)`; `_PARA` becomes `\n[ \t]*\n`; `_WORD` `[A-Za-z0-9]`.
- Python `str.splitlines()` splits on `\n`, `\r\n`, `\r`, `\x0b`, `\x0c`, `\x1c`, `\x1d`, `\x1e`, `\x85`, ` `, ` `; implement `fn split_lines(s: &str) -> Vec<&str>` with exactly that set (and no trailing empty item for a trailing terminator).
- `PARAGRAPH_BREAK` becomes `Chunk::ParagraphBreak`; text chunks are `Chunk::Text`.

- [ ] **Step 1: Write unit tests** in `assembler.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn texts(v: Vec<Chunk>) -> Vec<Option<String>> {
        v.into_iter().map(|c| match c { Chunk::Text(s) => Some(s), Chunk::ParagraphBreak => None }).collect()
    }

    #[test]
    fn sentence_waits_for_whitespace() {
        let mut a = ProseAssembler::new();
        assert!(texts(a.feed("Version 3.", 0, false)).is_empty());
        assert_eq!(texts(a.feed("14 is out. Next", 1, true)), vec![Some("Version 3.14 is out.".into()), Some("Next".into())]);
    }

    #[test]
    fn duplicate_index_zero_starts_a_new_block() {
        let mut a = ProseAssembler::new();
        a.feed("Old text", 0, false);
        let out = texts(a.feed("New.", 0, true));
        assert_eq!(out, vec![Some("Old text".into()), Some("New.".into())]);
    }

    #[test]
    fn split_lines_matches_python() {
        assert_eq!(split_lines("a\r\nb\rc\u{2028}d\n"), vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn unicode_text_never_panics() {
        let mut a = ProseAssembler::new();
        for (i, d) in ["🚀 “Hi”. ", "日本語。 ", "```é\nx\n``", "`\nend → ok"].iter().enumerate() {
            let _ = a.feed(d, i as u32, i == 3);
        }
    }
}
```

- [ ] **Step 2:** Run `cargo test -p sonara-core assembler`. Expected: FAIL.
- [ ] **Step 3:** Implement `ProseAssembler` by porting each Python method (`feed`, `_consume`, `_closing_fence_rest`, `_partial_fence_tail_len`, `_close_fence`, `_emit_chunk`, `_sentences_of`, `_split_sentences`, `_flush_prose`, `_reset`) with the same control flow and comments that explain the why (keep the audit references #25, #56). `seen` is a `HashSet<u32>`.
- [ ] **Step 4:** Run `cargo test -p sonara-core` (unit + golden). Expected: PASS, every golden case.
- [ ] **Step 5:** Run `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --all`. Expected: clean.
- [ ] **Step 6: Commit** `feat(core): port the prose assembler to Rust (#170)`.

### Task 5: Docs, version, PR

- [ ] **Step 1:** CLAUDE.md build block: add `Rust: cargo fmt --all -- --check; cargo clippy --workspace --all-targets -- -D warnings; cargo test --workspace; cargo deny check licenses bans`. One line under Conventions: "Text rules exist in Python and Rust until the cutover (M11); change both together with the golden fixtures."
- [ ] **Step 2:** Version 0.9.0 in `pyproject.toml`, `.claude-plugin/plugin.json`, `.claude-plugin/marketplace.json`, `src/sonara/__init__.py`, `Cargo.toml`; extend `tests/test_manifests.py` to also assert the `[workspace.package] version` equals pyproject's.
- [ ] **Step 3:** Run the full Python suite and the Rust job commands locally. Expected: all green.
- [ ] **Step 4:** Commit, push, open the PR (`Closes #170`, `Part of #168`).

---

## Execution

- Branching: M0 and M1 run in parallel (done). From M2 on, milestones run in order; M6 to M10 depend only on M5's facade and protocol, so M8 (system) and M9/M10 (SDKs, player) may run in parallel with M6/M7. Each PR opens against `main` once its predecessor is merged, or stacks on it if not yet merged.
- Each milestone: Workflow with implementer, reviewer and finalize agents (as in Phase 0), then hands-on install on this PC for M3 onward, then "merge?".
- Stop points that need the user: M0 listening check; M0 "no candidate passes"; any deviation from the spec; every merge.
