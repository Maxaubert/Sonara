# Distribution research (2026-10-02)
Read-only research behind `docs/plans/2026-10-02-sonara-runtime-spec.md`: how to make Sonara a reader that other apps bundle. Five angles researched in parallel, then synthesized. Sources are listed per angle with access dates. [V] = verified, [I] = inference. Not legal advice.

## Synthesis
# Sonara as a bundleable reader: architecture report

Tags: **[V]** means verified in the research (docs or this machine). **[I]** means inference. Not legal advice.

## 1. Problem and success criteria

**Problem.** Sonara 0.8.3 works well as a reader, but it only ships as a Claude Code plugin.
- **Claude-shaped core.** The message vocabulary (choice, plan, permission, tool_announce) and the session model come from Claude Code. The turn end is sent as a sound event (`EARCON turn_done`) [V, C1, C2, C4, C5].
- **Plugin-only install.** The installer aborts without a Claude plugin root, and hook installation is wired into `WinSupervisorBackend` [V, C6].
- **No streaming for hosts.** SPEAK takes whole text only. Streaming hosts have to use undocumented hook messages [V, C3].
- **Fixed instance.** `~/.sonara` is hard-coded, and hotkeys and the web UI are always on [V, C8, C9].
- **Weak public contract.** There is no handshake, no error replies and no published version policy [V, §4].
- **GPL in the default engine.** The Kokoro path pulls in espeak-ng (GPL-3.0+) through kokoro-onnx [V].

**Success criteria:**
1. A developer can add Sonara to a host in under an hour: Codex users, an Electron app (PrismTerminal), a Python CLI, or anything that can `curl`.
2. One reader per user, shared by all hosts. Hosts never talk over each other or fight over hotkeys or ducking.
3. A versioned, documented protocol, with a handshake, capabilities and coded errors. Thin, permissively licensed clients.
4. The engine and model are shipped apart from host installers. OneCore works with zero download.
5. Nothing GPL reaches a host process, and the default runtime bundle is GPL-free.
6. The existing Claude Code plugin keeps working throughout.

## 2. Architecture approaches

**Shared premise [I], from runtime and packaging research.** Ducking, pausing other apps, global hotkeys and "one message, always the last" are machine-wide concerns. Every serious analogue that has to arbitrate the machine runs as a sidecar: Speech Dispatcher, and Ollama in its shared mode [V]. So all three approaches keep **one per-user daemon plus a protocol**. They differ in how the core is built and shipped.

### A. Stable reader service in Python, frozen, with thin SDKs and host adapters

**Components:**
- `sonara-runtime`: today's daemon, frozen with Nuitka standalone or PyInstaller onedir. Avoid onefile because of antivirus flags [I].
- Protocol 1.1, additive only: HELLO with capabilities, an optional request `id` with coded errors, generic `session_open`, `focus`, `turn_end` and `ask` types, a delta/streaming input, and SPEAK acks with item lifecycle events.
- A loopback HTTP endpoint `POST /v1/event` with a token header.
- `sonara-client`: stdlib-only Python client. `@sonara/client`: pure TypeScript npm client.
- `sonara connect <host>`, which patches host configs idempotently and reversibly.

**How a developer bundles it:**
- **Codex users.** Run `sonara connect codex`. This writes `~/.codex/hooks.json`: Stop maps `last_assistant_message` to SPEAK or turn_end under `codex:<thread-id>`, and PermissionRequest maps to `ask`. Never use `notify`: its single slot is already taken by codex-computer-use on this machine [V]. Plugin-bundled Codex hooks show as "removed" [V], so it has to be a config patch.
- **Electron (PrismTerminal).**
  1. `npm i @sonara/client`.
  2. Ship the portable runtime zip through electron-builder `extraResources`. PrismTerminal already ships a pinned whisper sidecar this way [V].
  3. Call `connect({autostart:true, runtimePath})`, which tries the shared instance first, then the bundled copy.
  4. Use SPEAK/SUBSCRIBE plus `SONARA_HOST_TAB`.
- **Python CLI.** `pip install sonara-client`, then `sonara_client.speak(text, source="mycli")`. The no-SDK alternatives are `cmd | sonara read` or `sonara say`.

**Engine and licensing.** It keeps kokoro-onnx, which means GPL phonemizer and espeak-ng inside the runtime [V]. Moving English G2P to misaki without the espeak fallback needs a hand-picked dependency set [V]. Otherwise the frozen runtime is a GPL-3 combined work [I].

**Effort.** Days for the freeze. Roughly 2-4 weeks for protocol 1.1, the ingest split, the adapter interface and the clients [I].

**Risks.**
- Size: 150-250 MB before the model [V estimate].
- winsound has no true pause [V].
- Every hook spawns Python, about 100-300 ms each [I].
- Windows only.

### B. Rust reader core, same service, with optional C ABI and bindings

**Components:**
- `sonarad`, about 5-15 MB plus about 13 MB for onnxruntime [I].
- Crates [V exist]:
  - windows-rs for ducking (IAudioSessionControl2), GSMTC pausing and OneCore
  - cpal or rodio for audio
  - global-hotkey
  - sherpa-onnx's C/Rust API, or ort plus misaki-rs with no espeak, for Kokoro
- A Rust hook client of about 1 MB that starts in milliseconds.
- The same protocol and the same TypeScript and Python clients as A.
- Later, if hosts ask: napi-rs, maturin and C ABI builds for in-process use.

**How a developer bundles it.** Same steps as A, but the binary is smaller. A future `sonara-core` crate or npm addon would allow daemonless use, giving up cross-app arbitration.

**Engine and licensing.** This is the cleanest GPL-free path: misaki-rs with `default-features=false`, and sherpa-onnx 2.0, which plans to drop espeak (#3731, still open) [V]. One open check: whether sherpa-onnx's current Kokoro English path needs espeak-ng-data. Its model archives do ship that data [V].

**Effort.** About 4-8 weeks part-time to port roughly 9-10k lines of core logic. The existing tests serve as the spec [I].

**Risks.**
- ort is still a release candidate (2.0.0-rc.13) [V].
- Reading quality can regress if the G2P differs.
- Two daemons to maintain during the transition.

### C. Per-host native ports (TypeScript for Electron, others later)

**Components.** A TypeScript reader built on sherpa-onnx-node or kokoro-js, inside each host.

**How a developer bundles it.** `npm i` and run in-process. Non-Node hosts get nothing.

**Engine and licensing.** kokoro-js depends on npm `phonemizer`, which is espeak-ng compiled to WASM under an Apache label. Treat it as GPL [V/I]. sherpa-onnx-node 1.x links espeak-ng statically [V].

**Effort.** Moderate per host, and it multiplies with every host.

**Risks.** It breaks success criterion 2: every embedded reader ducks, pauses and binds hotkeys on its own [I]. Node still needs native addons for ducking and hotkeys. GPL code would sit inside host processes. **Rejected as the core.** It is fine only as a thin client.

## 3. Recommendation and roadmap

**Recommendation: A now, B as the long-term core, behind one frozen protocol.**

The integration model (sidecar plus protocol plus thin clients) is the same in A and B, so the language choice only affects size, startup and licensing [I]. Start with A. It unblocks Codex and PrismTerminal in weeks, and the protocol and its conformance suite are what let B replace A later without any host noticing. Fix the GPL exposure in phase 3, before Sonara is advertised as bundleable.

**Phase 1: contract and core cleanup.** One PR each, per the merge policy.
1. **Protocol 1.1** (spec and code):
   - HELLO returning `{daemon_version, protocol, min_protocol, capabilities}`.
   - Optional request `id` and coded errors.
   - `session_open`, `focus`, `turn_end` and `ask` types, plus a delta/append input.
   - SPEAK ack carrying an item id.
   - A semver and deprecation policy, and a section on auth and the threat model.
   - JSON Schema plus conformance fixtures.
   - Move the Claude hook messages into an adapter appendix.
2. **Ingest split.** Claude decision handlers move to `adapters/claude_code`. Each session gets an explicit `source`, replacing the `":"` heuristic.
3. **Adapter interface** (install, uninstall, doctor, health cue). Move claude_hooks out of the supervisor, and make `plugin_root` optional.
4. **Runtime overrides.** `SONARA_HOME` and a lockfile override. `hotkeys_enabled` and `webui_enabled` flags.

**Phase 2: first distribution.**
5. **Codex hooks adapter** and `sonara connect`/`disconnect`, with checks in `sonara doctor`.
6. **Loopback HTTP ingress** and `@sonara/client` (npm) plus `sonara-client` (PyPI).
7. **Frozen `sonara-runtime`**, as a per-user installer and a portable zip, SignPath-signed once enrolled. Models download on first use, SHA-256 pinned, to `%LOCALAPPDATA%\Sonara\models`, with OneCore as the default.

**Phase 3: licensing and engine.**
8. **Engine split.** Split TtsBackend into TtsEngine and AudioOutput, which also fixes real pause. Then a pluggable engine registry that carries a `license_class` per engine.
9. **GPL-free Kokoro.** Use misaki lexicon G2P, with letter-spelling or a vetted permissive G2P for unknown words. Benchmark sherpa-onnx against kokoro-onnx on this machine, and A/B listen on real Claude and Codex output. espeak becomes an opt-in "pronunciation pack".
10. **Notices.** Add THIRD_PARTY_NOTICES and fix the "espeak-ng LGPL" error in the phase-3 spec.

**Phase 4: reach.** Field mappings for Gemini, Copilot, Cursor and Windsurf. An ACP proxy (about 35 agents, with streaming). A Codex app-server subscriber, which is experimental [V].

**Phase 5 (deferred): Rust `sonarad`.** Port module by module against the conformance suite and ship it behind a switch. The Rust hook client can come earlier as a latency win.

**Deferred indefinitely:** MCP `speak` tool, SAPI5 voice, browser extension, macOS and Linux support.

**Existing Claude Code plugin.** It becomes one adapter among several. It stays in the marketplace and keeps working unchanged through every phase. It installs the shared runtime instead of being the only way to install it. Later its hooks can switch to Claude `http` hooks, which spawn no process, once header and env interpolation are verified [I].

**PrismTerminal deferred phases.** Its Phase 0 groundwork (SUBSCRIBE, SPEAK, host_tab) is already the embedding-host path. PrismTerminal becomes the reference Electron integration in phase 2 (items 6 and 7): `@sonara/client` plus the zip runtime in `extraResources`. Resume its deferred phases after item 7. They also need item 4, so PrismTerminal can own its player UI and keys.

## 4. Open questions for the user

1. **Core language long term?** (a) Python only, frozen. (b) **Python now, Rust later behind the protocol (recommended).** (c) Rust rewrite now.
2. **Licence of the shipped runtime?** (a) **MIT runtime with a GPL-free default G2P; espeak only as an opt-in pack (recommended).** (b) Declare the runtime GPL-3, clients MIT. (c) Keep it as is and do not bundle Kokoro.
3. **First host after Claude Code?** (a) **Codex hooks (recommended).** (b) PrismTerminal. (c) ACP proxy.
4. **Platform scope?** (a) **Windows only until the Rust core (recommended).** (b) Add macOS and Linux in the Python runtime now.
5. **Instance model for bundlers?** (a) **Shared per-user instance first, bundled copy as fallback (recommended).** (b) Every host runs a private instance. (c) Shared only.

## 5. Licensing red flags

| Component | Status | Source |
|---|---|---|
| espeak-ng | GPL-3.0-or-later, no LGPL option | [V] |
| kokoro-onnx 0.5.0 (current engine) | Requires phonemizer-fork (GPLv3+) and espeakng-loader, which loads libespeak-ng | [V] |
| Phase-3 spec | Wrongly calls espeak-ng "LGPL" | [V] |
| Piper | Now piper1-gpl, GPL-3.0 | [V] |
| sherpa-onnx 1.x | Static build links espeak-ng | [V] |
| kokoro-js, via npm `phonemizer` | espeak-ng in WASM, labelled Apache; issue #6 is open | [V] |
| espeakng-loader wheel | Ships the GPL library; PyPI page states no licence | [V] |
| Supertonic 3 | OpenRAIL-M weights with use restrictions | [V] |
| Piper voices | Licences vary per voice | [I] |

Today users pip-install kokoro-onnx themselves. Bundling it into an installer turns the runtime into a GPL combined work, which needs a source offer and GPL notices [I].

**How the recommendation avoids this:**
- **Process boundary.** Hosts link only MIT clients (TypeScript, Python, HTTP) and talk to the runtime over a socket. Speech Dispatcher relies on the same boundary [V]. The FSF "arm's length" view behind it is legally untested, so get a quick legal check before a commercial bundle includes the GPL pack [I].
- **GPL-free default engine.** OneCore plus Kokoro (Apache weights) with misaki lexicon G2P and no espeak fallback.
- **espeak quarantined.** It is a separately downloaded, labelled pack that runs only inside the daemon, with source and notices.
- **Engines declare their licence.** Each engine reports a `license_class`, so a host bundles only the engines it can license.
- **Notices and tracking.** Add THIRD_PARTY_NOTICES, fix the spec error, and track sherpa-onnx 2.0 as the espeak-free native engine.

**Unverified, check before committing:**
- whether kokoro-onnx 0.5.0's dependencies match 0.6.1 (one source says they do)
- whether sherpa-onnx's Kokoro English path needs espeak-ng-data
- Codex plugin-bundled hooks
- Claude `http` hook header and env interpolation
- Gemini hooks on Windows
- whether xterm.js supports OSC 133

**Local files:**
- C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/docs/protocol.md
- C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/docs/plans/embedding-research.md
- C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/src/sonara/requirements-kokoro.txt
- C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/docs/history/specs/2026-06-10-sonari-phase3-windows-design.md
- C:/Users/Admin/.codex/config.toml

## Host integration surfaces

### Recommendation

Keep the daemon and wire protocol as the single reader core. Add a normalized ingress event (final, delta, attention, end) and an authenticated loopback HTTP endpoint next to TCP. Then ship thin per-host adapters behind one `sonara connect <host>` command. Start with the Codex hooks.json adapter (Stop.last_assistant_message plus PermissionRequest; never use `notify`, its single slot is already taken by codex-computer-use here). Next, the HTTP endpoint plus an npm client for PrismTerminal, VS Code and browser hosts. Then field mappings for Gemini (AfterAgent/AfterModel/Notification), Copilot (subagentStop plus agentStop transcript), Cursor (afterAgentResponse) and Windsurf (post_cascade_response). Finally an ACP proxy that covers about 35 agents with streaming. Treat MCP `speak`, transcript tailing, stdin pipe and a SAPI5 voice as fallbacks, not the primary path.

### Findings

HOST INTEGRATION SURFACES (angle: hosts). Accessed 2026-10-02. V = verified (docs or this machine), I = inference.

Local facts checked on this machine (read-only):
- Codex CLI 0.153.2 is installed. `codex features list` shows `hooks = stable, true` and `plugin_hooks = removed`. `codex app-server` (experimental) has daemon, proxy, generate-ts and generate-json-schema subcommands. `codex mcp-server` also exists. (V)
- ~/.codex/config.toml line 13: `notify = [ ...codex-computer-use.exe, "turn-ended" ]`. The single `notify` slot is already used by OpenAI's own computer-use tool, so Sonara must not depend on `notify`. (V)
- ~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl contains these record types: `response_item` (message role=assistant, agent_message, reasoning, tool calls), `event_msg` (task_started, task_complete, item_completed, turn_aborted) and `realtime_item` transcript_segment. These files are a usable tail source, but the schema is internal and changes (there is a migrate-rollouts command). (V)
- Sonara hooks/hooks.json already uses Claude Code MessageDisplay, PreToolUse, PostToolUse(AskUserQuestion) and Notification(permission_prompt). (V)

TABLE: host | text stream exposed | questions / permissions | how a third-party reader hooks in | setup friction
1. Claude Code | Hooks. Stop/SubagentStop carry `last_assistant_message` and `transcript_path`. MessageDisplay fires per displayed chunk (display-only, 10 s timeout). Agent SDK streams `stream_event` text_delta when includePartialMessages=true | Notification types permission_prompt, idle_prompt, elicitation_dialog, agent_needs_input. PermissionRequest event. Elicitation event | Plugin with bundled hooks/hooks.json (marketplace). Hook types are command, http, mcp_tool, prompt and agent. An `http` hook can POST straight to a daemon with no process spawn | Low: plugin install. Already shipped (V)
2. OpenAI Codex CLI | (a) Hooks: SessionStart/End, UserPromptSubmit, Stop (`last_assistant_message`, turn_id), Interrupt, SubagentStop, PreToolUse, PermissionRequest, Pre/PostCompact. Config in ~/.codex/hooks.json or a [hooks] table, user or repo level, with a `commandWindows` override. Final messages only, no deltas. (b) `notify`: one program, gets agent-turn-complete JSON with last-assistant-message. (c) app-server: JSON-RPC 2.0 over stdio, WebSocket or unix socket. Sends item/agentMessage/delta, item/completed and turn/completed. Several clients can subscribe to one thread via thread/resume. (d) `codex exec --json` JSONL: item.completed with agent_message text. (e) rollout JSONL files | PermissionRequest hook. app-server approval requests | Hooks give the best final-message path. app-server gives streaming but is experimental, and the host has to run via app-server: an IDE or Desktop does, a plain TUI is not confirmed (I) | Medium: Sonara must write ~/.codex/hooks.json itself. Plugin-bundled hooks show as "removed" here (V). It is unclear whether Codex plugins can carry hooks today (I)
3. Gemini CLI | Hooks in settings.json (user, project, system): AfterAgent `prompt_response` (final text), AfterModel `llm_response` "fired for every chunk" (streaming), SessionStart/End, BeforeTool/AfterTool. Common fields include transcript_path | Notification with notification_type `ToolPermission` | Command hooks. Also speaks ACP natively | Low to medium: JSON config edit. Windows behaviour not checked (I)
4. GitHub Copilot CLI | Hooks: sessionStart/End, userPromptSubmitted, pre/postToolUse, agentStop (transcriptPath, NO response text), subagentStop (`response` text), permissionRequest, notification | notification types permission_prompt, elicitation_dialog, agent_idle, agent_completed | Hooks in ~/.copilot/hooks/, .github/hooks/*.json, settings.json, or plugins. A `powershell` field is supported. The main-agent text has to be read from the transcriptPath file on agentStop | Medium: transcript format parsing
5. Cursor (IDE, cloud agents) | hooks.json (user, project, enterprise C:\ProgramData\Cursor). afterAgentResponse `{text}` = final text. afterAgentThought. stop {status} | before* hooks are blocking permission gates. No dedicated "waiting for user" event (I) | Command hooks over stdio JSON. Also ACP agent | Low: JSON file. Does not fire in cloud agents (forum reports)
6. Windsurf / Devin Desktop | Cascade hooks: post_cascade_response (markdown response since the last user input, async fire-and-forget), post_cascade_response_with_transcript | pre_* hooks | Command hooks | Low to medium
7. Aider | Text only on the terminal and in .aider.chat.history.md / --llm-history-file. --notifications-command runs on "response ready" but passes NO text. Python scripting API (Coder) | None for prompts | Tail the history markdown, or wrap the PTY | Medium to high: polling and markdown parsing
8. VS Code (and forks) | No API to read another extension's chat output (I, consistent with vscode#190941). Built-in accessibility.voice.autoSynthesize reads Copilot chat via VS Code Speech (and is known to read tool and thinking noise, vscode#296720). Terminal output: stable window.onDidStartTerminalShellExecution plus execution.read() stream (raw, escape codes, needs shell integration) | n/a | Sonara VS Code extension: terminal shell-execution reader plus a command "Read selection or last output" calling SPEAK | Medium: extension per marketplace (VS Code, Open VSX)
9. ACP (Agent Client Protocol, Zed and others) | JSON-RPC over stdio. session/update agent_message_chunk (streaming), tool_call, plan | session/request_permission | A reader can be a transparent ACP proxy between editor and agent and tee the chunks. One adapter covers about 35 agents: Gemini CLI, Copilot, Cursor, Goose, OpenCode, Cline, Kiro, Qwen, plus Claude and Codex via adapters | Medium: user points the editor at `sonara-acp -- <agent cmd>`
10. Generic terminals (Windows Terminal, xterm.js/PrismTerminal, WezTerm) | Raw PTY bytes. OSC 133 A/B/C/D marks prompt and command boundaries. OSC 9 / 777 are desktop notifications (777 in Windows Terminal, WezTerm, Ghostty). xterm.js reportedly lacks OSC 133 (search summary, unconfirmed) | An OSC 9/777 notification is the only universal "needs attention" signal | Terminal-embedded host (PrismTerminal): SPEAK/SUBSCRIBE plus SONARA_HOST_TAB, or a PTY tee with ANSI strip and an idle/OSC 133 heuristic | High for raw PTY: TUI redraws make scraping noisy
11. Browsers (ChatGPT, Claude.ai web) | DOM only | DOM | Extension content script per site, sending to the daemon through Chrome native messaging. chrome.ttsEngine lets an extension register itself as a browser TTS voice | Medium: store publishing, per-site selectors break
12. MCP | Sonara as an MCP server exposing a `speak` tool: the model decides when to call it, which is unreliable as the main path and costs tokens. Sonara as an MCP client gains nothing. Claude Code's `mcp_tool` hook type could call a Sonara MCP server deterministically | elicitation | Portable to every MCP host (Claude Desktop, Cursor, Codex, VS Code) | Low setup, low fidelity (I)
13. Plain stdin / pipe / CLI | Any process | n/a | `cmd | sonara read` (line or paragraph chunking), `sonara say "text"`, `sonara tail <file> --format codex|aider|md` | Lowest
14. OS level (reverse adapter) | Any SAPI5 app | n/a | Register Kokoro as a SAPI5 voice through a COM server (prior art: VoiceBroker in Python, NaturalVoiceSAPIAdapter). All SAPI apps get the voice, but not Sonara's routing, ducking or "last message" logic | High: COM registration, admin (I)

BEST ADAPTER STRATEGY (I, derived from the table):
1. Reader core = the daemon plus the wire protocol (docs/protocol.md). Freeze one normalized ingress event that every adapter emits: {source, session/tab, kind: final|delta|question|permission|status|end, text, interrupt}. Today's SPEAK covers kind=final. Add a `delta`/append mode (Gemini AfterModel, Codex app-server, ACP chunks, Agent SDK) and an `attention` kind (permission or question), so prompts are announced the same way for every host.
2. Add an authenticated loopback HTTP ingress (POST /v1/event, token header) next to the TCP NDJSON. Reasons: Claude Code `http` hooks need no Python spawn per chunk; JS/Electron hosts, browser native-messaging shims and curl one-liners all work; hosts in any language need no client library. Ship tiny client SDKs (npm `@sonara/client` for Electron/VS Code, plus the existing Python client.py).
3. Tier the adapters by fidelity, all thin, stateless and host-specific only in field mapping:
   A. Native hook adapters, one entrypoint `sonara-hook --host <claude|codex|gemini|copilot|cursor|windsurf> <event>` that reads stdin JSON and maps it: Claude Stop/MessageDisplay/Notification; Codex Stop/PermissionRequest; Gemini AfterAgent (or AfterModel for streaming)/Notification; Copilot subagentStop plus agentStop→transcript read/notification; Cursor afterAgentResponse; Windsurf post_cascade_response. Use a fast native launcher, not a Python cold start, because streaming events fire per chunk.
   B. Protocol taps for streaming and multi-agent coverage: an ACP proxy (one adapter, about 35 agents, including Zed/JetBrains-hosted agents) and a Codex app-server subscriber (thread/resume, item/agentMessage/delta).
   C. Generic fallbacks: `sonara say`, stdin pipe, a transcript tailer (Codex rollouts, Copilot transcriptPath, aider .chat.history.md), and an MCP server `speak` tool as an opt-in extra.
   D. Embedding hosts (PrismTerminal, a VS Code extension, browser extension): SPEAK/SUBSCRIBE plus the host_tab env var (already designed). The host owns the UI, Sonara owns voice and routing.
4. Setup: one `sonara connect <host>` / `sonara disconnect <host>` command that patches each host's config idempotently and reversibly (it records what it added) and is checked by `sonara doctor`. For Codex, use hooks.json and never `notify`, because the notify slot is single and already taken on this machine. Where a host has a plugin or extension channel (Claude marketplace, Copilot plugins, VS Code/Open VSX), publish there instead of patching config.
5. Priority order by reach and effort: (1) Codex hooks adapter (same shape as Claude, final message, low effort). (2) HTTP ingress plus npm client (unblocks PrismTerminal, VS Code, browser). (3) Gemini, Copilot and Cursor hook mappings (each about a table of fields). (4) ACP proxy for streaming and the long tail. (5) MCP speak tool and the SAPI voice only if asked.

Caveats: Gemini on Windows, Cursor CLI hooks, Codex plugin-bundled hooks and Claude http-hook header/env interpolation were not verified. Codex app-server is marked experimental. The search summary said xterm.js lacks OSC 133 support, but this was not checked against the xterm.js docs.

### Sources

- https://learn.chatgpt.com/docs/hooks (Codex hooks, accessed 2026-10-02)
- https://learn.chatgpt.com/docs/app-server (Codex app-server, accessed 2026-10-02)
- https://takopi.dev/reference/runners/codex/exec-json-cheatsheet/ (codex exec --json, accessed 2026-10-02)
- https://backgrind.com/blog/codex-cli-notifications/ (Codex notify payload, accessed 2026-10-02)
- https://code.claude.com/docs/en/hooks (Claude Code hooks incl. MessageDisplay, http hooks, plugin hooks, accessed 2026-10-02)
- https://code.claude.com/docs/en/agent-sdk/streaming-output (Agent SDK partial messages, accessed 2026-10-02)
- https://geminicli.com/docs/hooks/ and https://geminicli.com/docs/hooks/reference (accessed 2026-10-02)
- https://docs.github.com/en/copilot/reference/hooks-configuration (Copilot CLI hooks, accessed 2026-10-02)
- https://cursor.com/docs/hooks (accessed 2026-10-02)
- https://docs.windsurf.com/windsurf/cascade/hooks (accessed 2026-10-02)
- https://aider.chat/docs/config/options.html and https://aider.chat/docs/usage/notifications.html (accessed 2026-10-02)
- https://agentclientprotocol.com/protocol/prompt-turn and https://agentclientprotocol.com/overview/agents (accessed 2026-10-02)
- https://code.visualstudio.com/api/references/vscode-api (onDidStartTerminalShellExecution/read, accessed 2026-10-02)
- https://github.com/microsoft/vscode/issues/296720 (autoSynthesize reads tool noise, accessed 2026-10-02)
- https://github.com/microsoft/vscode/issues/190941 (no general terminal-output API, accessed 2026-10-02)
- https://devblogs.microsoft.com/commandline/shell-integration-in-the-windows-terminal/ and https://terminfo.dev/osc (OSC 133/9/777, accessed 2026-10-02)
- https://developer.chrome.com/docs/extensions/reference/api/ttsEngine (accessed 2026-10-02)
- https://modelcontextprotocol.io/docs/2026-07-28/learn/architecture and https://github.com/blacktop/mcp-tts (accessed 2026-10-02)
- https://github.com/AceCentre/VoiceBroker and https://github.com/gexgd0419/NaturalVoiceSAPIAdapter (SAPI5 bridge prior art, accessed 2026-10-02)
- Local: C:/Users/Admin/.codex/config.toml (notify line 13), C:/Users/Admin/.codex/sessions/**/rollout-*.jsonl, `codex --help`, `codex features list`, `codex app-server --help`
- Local: C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/docs/protocol.md, hooks/hooks.json

## Packaging patterns of comparable runtimes

### Recommendation

Package Sonara the way Speech Dispatcher and Ollama do. Ship a signed sidecar runtime: a frozen `sonara-runtime` exe, offered as a per-user installer and as a portable zip. Hosts reach it through thin, permissively licensed client SDKs, starting with Python and TypeScript/Node, which speak only the documented JSON-lines protocol and never link the engine.

Concretely:
1. Make the lockfile a public discovery contract, with a SONARA_HOME override and an SDK `connect({autostart})` that health-checks, spawns and waits.
2. Add a HELLO or version handshake with a capabilities list, plus a `/version` on http_port.
3. Keep models outside the exe: a SHA-pinned download to a shared `%LOCALAPPDATA%` folder with an env override, int8 Kokoro (about 100 MB) as the bundler default, and OneCore as the zero-download fallback.
4. Keep the Claude hooks and the Codex/session-file followers as separate source adapters, distinct from the runtime.
5. Before marketing Sonara as bundleable, resolve the espeak-ng GPL-3 exposure: kokoro-onnx pulls in GPL phonemizer and espeakng-loader. Either move English G2P to misaki with no espeak fallback, or license the runtime GPL with MIT clients. Track sherpa-onnx 2.0 (espeak-free) as a future native engine.

Optionally add an OpenAI-compatible `/v1/audio/speech` so apps can use Sonara with no SDK. Sign with SignPath and auto-update the shared install from GitHub Releases; host apps update a bundled copy themselves.

### Findings

PACKAGING ANGLE: how local AI and speech runtimes ship so other apps can embed them (read-only research, accessed 2026-10-02)

A. VERIFIED FACTS PER PROJECT

1. Ollama: sidecar daemon plus HTTP API plus SDKs
- MIT licence. Official SDKs are ollama-python and ollama-js. It also has an OpenAI-compatible REST surface, and 30+ community SDKs are listed. [1]
- Discovery: a fixed well-known port, 127.0.0.1:11434, overridable with OLLAMA_HOST. Browser origins are opt-in through OLLAMA_ORIGINS (for example chrome-extension://*). Models live in ~/.ollama/models, overridable with OLLAMA_MODELS. [2]
- Two Windows distributions:
  - OllamaSetup.exe: a per-user install with no admin needed, `/DIR` sets the folder, and it auto-updates with a "Restart to update" tray prompt.
  - A standalone `ollama-windows-amd64.zip` "for embedding", run as a service with NSSM. [2][3]
- A one-line install also exists (`irm https://ollama.com/install.ps1 | iex`). [1]
- `GET /api/version` returns `{"version": ...}`, so hosts can check compatibility. [4]
- The pattern: a shared per-user instance that hosts probe on a known port, or a bundled zip copy for apps that want their own.

2. llama.cpp and whisper.cpp: C library plus server plus bindings
- Both are MIT. whisper.cpp has a C API (`whisper.h`), a `whisper-server` with an OAI-like API, and bindings for Rust, JS, Go, Java, .NET, Python, Swift, Unity and more. [5]
- llama-server listens on 127.0.0.1:8080, takes `--port` and `--api-key`, and serves `/health` (503 while loading, 200 `{"status":"ok"}` when ready) plus OpenAI-compatible `/v1/*`. [6]
- The pattern: one core, offered as an in-process library or as a server process. Hosts pick one, and readiness is checked with a health endpoint.

3. sherpa-onnx: the most "bundle-friendly" speech runtime
- Apache-2.0. A C API with 12 language bindings (C, C++, Python, JS, Java, C#, Kotlin, Swift, Go, Dart, Rust, Pascal), plus WASM, a Node native addon and a WebSocket server. [7]
- It supports Kokoro-82M, Piper/VITS, Matcha and others. [7]
- npm: `sherpa-onnx` (WASM) and `sherpa-onnx-node`, the native addon the docs recommend for Node, with per-platform packages such as `sherpa-onnx-win-x64`. Current version 1.13.8, Node 18 or newer. [8][9]
- Prebuilt binaries and model archives are published on GitHub Releases. [7]
- Kokoro model archives [10]:
  - `kokoro-multi-lang-v1_0.tar.bz2`: about 334 to 350 MB (model.onnx is 310 MB).
  - The int8 build: about 103 MB.
  - Each archive holds model.onnx, voices.bin, tokens.txt and espeak-ng-data/.
- LICENCE PROBLEM: the prebuilt static package links espeak-ng (GPL-3.0-or-later) unconditionally, which makes the whole binary a GPL combined work. Issue #3731 (2026-07-08, open) plans sherpa-onnx 2.0.0 without espeak-ng, using a user lexicon or a pre-phonemized `tokens` input. [11]
- Downstream apps already ship TTS-disabled or no-espeak sherpa builds to avoid this (OpenWhispr PR #2340, inkwell PR #121). [12]

4. Piper TTS
- The original rhasspy/piper (MIT) was archived on 2025-10-06. Development moved to OHF-Voice/piper1-gpl, which is GPL-3.0. [13][14]
- The maintainer's stated reason: espeak-ng is now embedded directly instead of through a separate piper-phonemize library. [14]
- It ships as a pip wheel (`piper-tts`), a C/C++ `libpiper`, an HTTP server and Docker. [13]
- It is used by Home Assistant and NVDA. [13]

5. espeak-ng
- GPL-3.0-or-later, with some BSD-2 parts. Ships as a C shared library (a DLL on Windows, API in `speak_lib.h`) and a Windows MSI. SAPI5 support comes from a third-party engine. [15]

6. Kokoro runtimes
- kokoro-onnx: the package is MIT and the weights are Apache-2.0. Its current pyproject (v0.6.1) depends on `espeakng-loader` and `phonemizer>=3.4.0`. [16][17]
  - The `phonemizer` package is GPL-3.0-or-later. [18]
  - `espeakng-loader` ships the espeak-ng shared library and data, about 10 MB per wheel, and its PyPI page states no licence. [19]
  - Sonara pins kokoro-onnx==0.5.0 (src/sonara/requirements-kokoro.txt). That 0.5.0 has the same phonemizer dependency is my inference and was not checked.
- kokoro-js: Apache-2.0 and built on transformers.js. Device options are wasm, webgpu and cpu (Node). Dtypes are fp32, fp16, q8, q4 and q4f16. It streams through `TextSplitterStream`. [20]
  - It depends on `phonemizer` (npm, xenova), which is "eSpeak NG" compiled to WASM yet labelled Apache-2.0. Issue #6 "Invalid license" (2026-01-26, open, unanswered) argues it must be GPL. [21][22]
- Kokoros (Rust): Apache. Offers `koko openai` (an OpenAI-style `/v1/audio/speech` on port 3000, with PCM streaming) and Docker. Its docs list only macOS and Linux build requirements, it uses espeak-ng, and the README offers no explicit prebuilt downloads. [23]
- misaki, hexgrad's G2P: Apache-2.0. The espeak-ng fallback is optional (`fallback=None` works). This is a GPL-free phonemizer path for English Kokoro. [24]

7. Speech Dispatcher: the closest analogue to a reader runtime
- A central server speaks SSIP, a simple text protocol, to clients and to output modules. There are 20+ modules, including espeak-ng, Festival and Piper. [25]
- Licensing is designed for bundlers:
  - The server is GPL-2.0-or-later.
  - The client libraries (libspeechd for C, speechd for Python) are LGPL-2.1-or-later.
  - Module helpers are BSD-2.
  - Modules connect over pipes "regardless of their licenses".
- The protocol boundary is explicitly what keeps the GPL from spreading. [25]

8. NVDA add-ons: plugin packaging with API compatibility
- An add-on is a `.nvda-addon` zip with a manifest. `minimumNVDAVersion` and `lastTestedNVDAVersion` are checked against `nvdaAPIVersions.json`. There are stable, beta and dev channels. Store submission runs automated validation plus a VirusTotal scan, and first submissions get manual review. [26]

B. COMPARISON (as it applies to Sonara)

- Library vs sidecar vs both:
  - The mature projects offer both (llama.cpp, whisper.cpp, sherpa-onnx, Piper). Ollama and Speech Dispatcher are sidecar-first.
  - Sonara is Python and has stateful features (per-session router, ducking, hotkeys, settings page). A sidecar is the only reasonable cross-language form. An in-process library only fits Python hosts.
- Discovery:
  - Ollama uses a fixed port plus an environment override.
  - Speech Dispatcher uses a well-known socket and autospawns the server.
  - Sonara uses a random port and token in `~/.sonara/daemon.lock`. That is secure, but hosts need a client library to read it, and Sonara has no autospawn contract.
- Shared vs bundled instance:
  - Ollama supports both: the shared per-user install, or the zip run by the app itself.
  - Sonara's lockfile path is hard-wired to `~/.sonara`. A bundled private instance would collide unless the data dir can be overridden.
- Versioning:
  - Ollama has `/api/version`, llama-server has `/health`, NVDA uses min and last-tested API fields.
  - Sonara has an advisory `"v": 1`, a grow-only rule, and ignores unknown types and fields (docs/protocol.md lines 14-18).
  - Missing: a handshake reply carrying the daemon version, the protocol version and a capability list.
- Install size:
  - Ollama needs 4 GB or more for the binary install.
  - A Kokoro fp32 model is about 310 MB, int8 about 90 to 103 MB.
  - A frozen CPython plus onnxruntime plus numpy runtime is over 200 MB, per the existing estimate in embedding-research.md.
  - The engine and the model are always shipped apart, downloaded on first use to a shared per-user folder with an env override.
- Code signing and updates:
  - Ollama's installer self-updates, and the zip form leaves updates to the host.
  - NVDA scans and reviews add-ons.
  - Sonara has nothing here yet. The user's global plan (SignPath Foundation, in-app update check against GitHub Releases) would apply to a frozen runtime exe.
- Licensing (the biggest hidden risk):
  - Every Kokoro or Piper path that uses espeak-ng (kokoro-onnx via phonemizer and espeakng-loader, sherpa-onnx 1.x static, piper1-gpl, possibly kokoro-js via phonemizer.js) pulls GPL-3 code into the bundle.
  - Speech Dispatcher's answer is a protocol boundary plus LGPL or permissive client libraries.
  - sherpa-onnx's answer is to drop espeak-ng in 2.0.
  - The GPL-free English route is misaki with no fallback, or a lexicon.

C. INFERENCE: PATTERNS A "READER RUNTIME" SHOULD COPY
1. Split the product the way Speech Dispatcher does:
   - (a) `sonara-runtime`: the daemon, a sidecar exe.
   - (b) thin, permissively licensed client SDKs (Python, which already exists as client.py; TypeScript/Node; maybe a C# one) that only speak the JSON-lines protocol.
   - (c) "source adapters" (Claude Code hooks, Codex session-file follower, a generic SPEAK CLI or stdin pipe) kept apart from the engine.
   The GPL-exposed TTS stays behind the process boundary, so host apps link only the MIT or Apache client. Whether a process boundary truly isolates GPL is the FSF "arm's length" view and legally untested; get the user's own call on this. [14][25]
2. Discovery contract:
   - Keep the lockfile, but document it as the API: path, JSON fields, and a `SONARA_HOME` or `SONARA_LOCK` env override so a bundled private instance works next to a shared one.
   - Add `sonara-runtime --ensure`, or SDK `connect({autostart:true})`, which reads the lock, health-checks, spawns if absent and waits for ready. This copies Speech Dispatcher autospawn and llama `/health`.
3. Shared-first, bundle-optional, like Ollama:
   - The SDK first looks for a running shared instance, then a configured runtime path, then the app's bundled copy.
   - Ship a portable zip (Ollama's `windows-amd64.zip` model) as well as the installer.
4. A versioned handshake:
   - Make the token-line reply a HELLO, for example `{daemon_version, protocol: 1, min_protocol, capabilities:[speak, subscribe, summaries, kokoro, onecore]}`.
   - Keep the grow-only rule, and expose a `/version` on the existing http_port like Ollama's `/api/version`.
5. Ship models apart:
   - Engine exe and model download with SHA-256 pins into `%LOCALAPPDATA%\Sonara\models`, overridable like OLLAMA_MODELS, and shared by every host that bundles Sonara.
   - Offer int8 Kokoro (about 100 MB) as the default for bundlers, with OneCore as the zero-download fallback.
6. Deal with the phonemizer licence before advertising "bundle me":
   - Option 1: switch the English G2P to misaki with fallback=None (Apache).
   - Option 2: state that the runtime is GPL-3 and the clients are MIT.
   - Option 3: track sherpa-onnx 2.0 with lexicon or tokens input as the native engine.
   - Ship a THIRD_PARTY_NOTICES file either way.
7. An optional OpenAI-compatible `/v1/audio/speech` on the http_port (as Kokoros and many local TTS servers do) would let apps that already speak that API use Sonara with no SDK. A reader-specific API (SPEAK with source and tab, SUBSCRIBE, ducking, "one message, always the last") stays the main value.
8. Signing and updates: SignPath-sign the runtime exe and installer. The shared install self-updates from GitHub Releases. A bundled copy is updated by the host app, as Ollama's zip form is.

Unverified or not checked: Ollama's own Windows code-signing status; how large the sherpa-onnx Windows binaries are; whether Kokoros publishes prebuilt Windows binaries; the exact dependencies of kokoro-onnx 0.5.0 (0.6.1 was checked); the espeakng-loader licence.

Relevant local files: C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/docs/protocol.md, C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/docs/plans/embedding-research.md, C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/src/sonara/requirements-kokoro.txt, C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/pyproject.toml

### Sources

- [1] https://github.com/ollama/ollama (accessed 2026-10-02)
- [2] https://docs.ollama.com/faq (accessed 2026-10-02)
- [3] https://docs.ollama.com/windows (accessed 2026-10-02)
- [4] https://github.com/ollama/ollama/blob/main/docs/api.md (accessed 2026-10-02)
- [5] https://github.com/ggml-org/whisper.cpp (accessed 2026-10-02)
- [6] https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md (accessed 2026-10-02)
- [7] https://github.com/k2-fsa/sherpa-onnx (accessed 2026-10-02)
- [8] https://k2-fsa.github.io/sherpa/onnx/javascript-api/index.html (accessed 2026-10-02)
- [9] https://www.npmjs.com/package/sherpa-onnx (via search, accessed 2026-10-02)
- [10] https://k2-fsa.github.io/sherpa/onnx/tts/pretrained_models/kokoro.html and https://github.com/k2-fsa/sherpa-onnx/issues/2374 (via search, accessed 2026-10-02)
- [11] https://github.com/k2-fsa/sherpa-onnx/issues/3731 (accessed 2026-10-02)
- [12] https://github.com/OpenWhispr/openwhispr/pull/2340 and https://github.com/SirSicard/inkwell/pull/121 (via search, accessed 2026-10-02)
- [13] https://github.com/OHF-Voice/piper1-gpl (accessed 2026-10-02)
- [14] https://github.com/OHF-Voice/piper1-gpl/discussions/57 (accessed 2026-10-02)
- [15] https://github.com/espeak-ng/espeak-ng (accessed 2026-10-02)
- [16] https://github.com/thewh1teagle/kokoro-onnx (accessed 2026-10-02)
- [17] https://raw.githubusercontent.com/thewh1teagle/kokoro-onnx/main/pyproject.toml (accessed 2026-10-02)
- [18] https://pypi.org/project/phonemizer/ (accessed 2026-10-02)
- [19] https://pypi.org/project/espeakng-loader/ (accessed 2026-10-02)
- [20] https://github.com/hexgrad/kokoro/tree/main/kokoro.js and https://raw.githubusercontent.com/hexgrad/kokoro/main/kokoro.js/package.json (accessed 2026-10-02)
- [21] https://raw.githubusercontent.com/xenova/phonemizer.js/main/package.json (accessed 2026-10-02)
- [22] https://github.com/xenova/phonemizer.js/issues/6 (accessed 2026-10-02)
- [23] https://github.com/lucasjinreal/Kokoros (accessed 2026-10-02)
- [24] https://github.com/hexgrad/misaki (accessed 2026-10-02)
- [25] https://raw.githubusercontent.com/brailcom/speechd/master/README.md (accessed 2026-10-02)
- [26] https://github.com/nvaccess/addon-datastore/blob/master/docs/submitters/submissionGuide.md (accessed 2026-10-02)
- Local: C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/docs/protocol.md, docs/plans/embedding-research.md, src/sonara/requirements-kokoro.txt, pyproject.toml

## TTS engines and licensing

### Recommendation

ENGINE STRATEGY (inference built on the verified facts above):

1. Put a pluggable engine interface in the core, with no engine dependencies in the core itself:
   - Engine: id, capabilities {streaming, languages, voices(), sample_rate, word_timings, license_class: permissive|copyleft|os}, warm(), synthesize(text, voice, speed) -> iterator of PCM int16 chunks, cancel().
   - Separate Phonemizer/G2P interface (text -> phoneme or token ids), used by phoneme-based engines (Kokoro, VITS).
   - Engines are discovered by a registry or entry points, so a host can bundle only the engines it can license.
   - The host protocol (SPEAK/SUBSCRIBE) stays engine-agnostic.

2. Tier 0, always available, zero bytes and zero license risk: platform voices.
   - OneCore/SAPI on Windows (exists), AVSpeechSynthesizer on macOS, speech-dispatcher on Linux.
   - Optionally the Web Speech API inside Electron hosts.
   - This makes Sonara useful the moment it is embedded.

3. Tier 1, the default neural engine: Kokoro-82M (Apache weights) as an opt-in downloaded pack.
   - Use fp32 (326 MB) or fp16 (163 MB) plus voices (27 MB). Do not default to int8, which measured slower on x86 CPUs.
   - Stream sentence by sentence to keep first audio at about 0.3-1 s.
   - Move the runtime from kokoro-onnx (Python-only, hard-wired to GPL phonemizer and espeak-ng) to sherpa-onnx: Apache-2.0, prebuilt for Windows, macOS and Linux, with C, Node, Python, Rust, Go and C# bindings. One runtime then serves a Python daemon, a Node/Electron in-process build, and a future native sidecar.

4. Phonemization, the GPL firewall.
   - Default: a GPL-free G2P. Use misaki's Apache English lexicons, installed without misaki[en]'s espeak, torch and spacy extras. Feed the results to sherpa-onnx as lexicon or tokens; this matches sherpa-onnx's planned 2.0 Apache-only path (issue #3731).
   - Out-of-vocabulary words go to a permissive neural G2P (check that its license really is permissive before adopting it) or to a letter-spelling fallback.
   - Offer espeak-ng only as a separately downloaded, clearly labelled GPL "pronunciation pack" that runs inside the out-of-process daemon, never linked into a host. When it is shipped, include its source and notices.
   - Correct the "espeak-ng LGPL" error in the phase-3 spec and add a THIRD_PARTY_NOTICES file.

5. Do not adopt as bundled defaults:
   - Piper (GPL-3.0 now).
   - kokoro-js as-is: its npm phonemizer embeds espeak-ng under an Apache label.
   - Kokoros (espeak-ng, Windows not documented).

6. Watch list as optional engines:
   - Supertonic 3: no G2P, so GPL-free, fast and multilingual, but OpenRAIL-M weights. Treat it as an opt-in engine with its license shown.
   - KittenTTS: tiny, Apache-2.0, but a developer preview.
   - The Apache-licensed Piper and sherpa-onnx 2.0, once they ship.

7. Verification before committing:
   - Benchmark sherpa-onnx Kokoro against kokoro-onnx on this box (fp32, fp16, int8): first audio, RTF, RAM, install size.
   - A/B listen to the lexicon-only G2P against the espeak fallback on real Claude and Codex output (code identifiers, acronyms, paths).
   - Get a quick legal sanity check on the socket-boundary position if a GPL pack is shipped in a commercial bundle.

### Findings

ENGINES ANGLE: licensing, size, latency, portability (accessed 2026-10-02). [V] = verified from a source, [I] = my inference. This is not legal advice.

0. WHERE SONARA STANDS TODAY
- [V] Sonara is MIT (LICENSE, pyproject.toml). Its Kokoro path imports kokoro_onnx (src/sonara/kokoro.py:270). kokoro-onnx's pyproject depends on onnxruntime, espeakng-loader>=0.2.4, phonemizer>=3.4.0 and numpy. Sonara's own provisioning spec records that phonemizer-fork and espeakng-loader are pulled in transitively, with "bundled espeak-ng, no system espeak-ng needed" (docs/history/specs/2026-06-18-sonari-kokoro-provisioning-design.md:12).
- [V] espeak-ng is "GPL version 3 or later", with no LGPL option. phonemizer-fork is GPLv3+. The espeakng-loader wrapper is MIT, but the libespeak-ng it loads is GPL-3.0+.
- [V] Error in Sonara's docs: docs/history/specs/2026-06-10-sonari-phase3-windows-design.md:29 calls it "embedded espeak-ng LGPL". It is GPL-3.0+. Any third-party notice must fix this.
- [I] Today users pip-install kokoro-onnx themselves through opt-in provisioning, so Sonara does not distribute espeak-ng. That changes once the Kokoro stack is bundled into an installer, for example inside PrismTerminal. The shipped daemon is then a GPL-3.0 combined work, so it needs a source offer and GPL notices. MIT code can be combined into GPL, so that part is fine.
- [I] A host app that talks to the daemon only over TCP or a pipe, as a separate program, is generally treated as aggregation, not a derivative work. speech-dispatcher relies on exactly this: GPLv2 server, LGPL client, "connected... through a socket... such that GPL licensing propagation doesn't apply" [V]. So Sonara's daemon-over-socket design is also its license firewall. Linking espeak-ng (or a library that embeds it) into the host process breaks that firewall.

1. KOKORO-82M WEIGHTS
- [V] Apache-2.0, 82M parameters, 8 languages and 54 voices (v1.0), 24 kHz. English G2P is misaki, plus espeak-ng.
- [V] ONNX file sizes (onnx-community): fp32 326 MB, fp16 163 MB, q8f16 86 MB, quantized (int8) 92 MB, q4f16 155 MB. voices-v1.0.bin is about 26-27 MB.
- [V] CPU latency:
  - Short utterances: about 500 ms with fp32 ONNX and about 1100 ms with INT8 ONNX (hexgrad/kokoro#291, an NVDA user).
  - Old Xeon E5620: RTF 1.4 (fp16), 1.5 (fp32), 4.5 (int8). Here RTF means compute seconds per second of audio, so lower is better.
  - EPYC 4-core: RTF about 0.47 (PyTorch) and 0.51 (ONNX).
- [I] Int8 is smaller but slower on x86 CPUs, so ship fp32 or fp16 and stream sentence by sentence. First audio then lands at about 0.3-1 s on modern desktop CPUs. That is fine for "read the last message" but too slow for screen-reader-style micro-utterances.

2. kokoro-onnx (Python, the current engine)
- [V] MIT, built on onnxruntime. It forces phonemizer (GPL-3.0+) and espeakng-loader, which brings libespeak-ng (GPL-3.0+).
- [I] It is not GPL-free in any configuration without patching. It is also Python-only, which makes it a poor fit for embedding in a Node, Electron or Rust host.

3. misaki (G2P)
- [V] Apache-2.0. English uses a built-in dictionary. espeak is an optional fallback (fallback=None or EspeakFallback). Without a fallback, out-of-vocabulary (OOV) words "generate no phonemes".
- [V] Catch: the misaki[en] extra still installs phonemizer-fork and espeakng-loader, plus torch, transformers and spacy.
- [I] A GPL-free misaki needs a hand-picked dependency set: the core package plus the English lexicons, no espeak, no torch. OOV words then need another fallback. Options are a permissive neural G2P (for example DeepPhonemizer; its license is unverified) or spelling the word out letter by letter.

4. sherpa-onnx
- [V] Apache-2.0. Runs on Windows, macOS, Linux, Android, iOS, HarmonyOS and WASM, on x86, ARM and RISC-V.
- [V] Bindings: C, C++, Python, JS/Node, Java, C#, Kotlin, Swift, Go, Dart, Rust, Pascal.
- [V] TTS models: Kokoro, Piper/VITS, Matcha, ZipVoice, Pocket TTS. Latest release is v1.13.8 (2026-09-10), and the project is very active.
- [V] Kokoro phonemization uses lexicon.txt, espeak-ng-data and tokens.txt. Model sizes: kokoro-multi-lang-v1_0 fp32 is 310 MB plus 26 MB voices; v1_1 has 103 speakers and an int8 variant.
- [V] Current builds pull in espeak-ng through piper-phonemize. Issue #3731 (opened 2026-07-08, still open) plans to remove both "to keep it fully compatible with Apache-2.0" in a breaking 2.0.0. After that, phonemes come from a user lexicon.txt or from pre-computed tokens passed in a new GenerationConfig "tokens" field.
- [V] OpenWhispr (PR #2340, 2026-09-29) dropped the GPL espeak-ng library by using SHERPA_ONNX_ENABLE_TTS=OFF archives, because it only needed speech recognition.
- [I] sherpa-onnx is the best cross-language runtime for a portable Kokoro engine. Pair it with your own GPL-free G2P (misaki lexicons) and feed it tokens; the 2.0 direction makes this a first-class path.

5. kokoro-js (transformers.js, web and Node)
- [V] npm kokoro-js 1.2.1 is Apache-2.0 and depends on phonemizer ^1.2.1 and @huggingface/transformers ^3.5.1.
- [V] dtypes are fp32, fp16, q8, q4 and q4f16, running on WASM, WebGPU, or CPU in Node.
- [V] I downloaded and inspected the npm "phonemizer" 1.2.1 tarball. It declares Apache-2.0 and ships an Apache LICENSE, but it describes itself as "text to phones converter using eSpeak NG" and contains an espeakng.worker plus 1.3 MB dist bundles.
- [I] It embeds espeak-ng compiled to WASM under a permissive label. That is a license-provenance red flag: treat it as GPL-3.0 when you redistribute it.

6. Kokoros (Rust)
- [V] Apache-2.0. Phonemization is a native espeak-ng integration. It offers an OpenAI-compatible server with streaming; time to first audio is about 1.44 s on an M2. Documented platforms are macOS, Linux and Docker, and Windows is not documented.
- [I] Same espeak-ng GPL issue, and less mature than sherpa-onnx.

7. Piper
- [V] rhasspy/piper (MIT) is archived. Development moved to OHF-Voice/piper1-gpl, which is GPL-3.0 because it "embeds espeak-ng directly". piper-phonemize is GPL-3.0+.
- [V] The maintainer is "working on my own phonemizer that will be part of an Apache 2 version of Piper in the future". The project is "looking for maintainers". It is used by Home Assistant and NVDA.
- [I] Voice licenses vary per voice and are not uniformly permissive. Avoid Piper as a default for embedding.

8. Others
- [V] Supertonic 3 (2026-04-29): code MIT, model OpenRAIL-M (use restrictions). Takes raw text with no G2P, so no espeak. 31 languages, about 99M parameters, ONNX SDKs for Python, Node, C++, C#, Go, Swift, Rust, Flutter and the web. RTF is 0.31 at 5 steps versus 0.47-0.51 for Kokoro on the same CPU, with flatter prosody.
- [V] KittenTTS: Apache-2.0, 25-80 MB (nano int8 is 25 MB), CPU ONNX, "developer preview". [I] Its phonemizer may also be espeak-based; not checked.
- [V] Picovoice's vendor benchmark (Ryzen 7 5700X, treat it as biased): Kokoro and Piper are "above 1 GB" memory; Piper first audio is 1720 ms.

9. PLATFORM VOICES
- [I] Windows OneCore/SAPI (already in Sonara), macOS AVSpeechSynthesizer and Linux speech-dispatcher are provided by the OS. Nothing gets bundled, there is no license burden, first audio is roughly 100 ms or less, and quality is lower.
- [V] speech-dispatcher: server GPLv2, client libraries (C and Python) LGPL-2.1+.
- [I] Electron and Chromium hosts also have Web Speech speechSynthesis on top of the OS voices, at zero cost.

### Sources

- https://github.com/thewh1teagle/kokoro-onnx (accessed 2026-10-02)
- https://raw.githubusercontent.com/thewh1teagle/kokoro-onnx/main/pyproject.toml (accessed 2026-10-02)
- https://pypi.org/project/phonemizer-fork/ (accessed 2026-10-02)
- https://github.com/thewh1teagle/espeakng-loader (accessed 2026-10-02)
- https://github.com/espeak-ng/espeak-ng (accessed 2026-10-02)
- https://github.com/hexgrad/misaki and https://raw.githubusercontent.com/hexgrad/misaki/main/pyproject.toml (accessed 2026-10-02)
- https://huggingface.co/hexgrad/Kokoro-82M (accessed 2026-10-02)
- https://huggingface.co/onnx-community/Kokoro-82M-v1.0-ONNX/tree/main/onnx (accessed 2026-10-02)
- https://github.com/k2-fsa/sherpa-onnx (accessed 2026-10-02)
- https://github.com/k2-fsa/sherpa-onnx/releases (v1.13.8, accessed 2026-10-02)
- https://github.com/k2-fsa/sherpa-onnx/issues/3731 (accessed 2026-10-02)
- https://k2-fsa.github.io/sherpa/onnx/tts/pretrained_models/kokoro.html (accessed 2026-10-02)
- https://github.com/OpenWhispr/openwhispr/pull/2340 (accessed 2026-10-02)
- https://github.com/hexgrad/kokoro/tree/main/kokoro.js (accessed 2026-10-02)
- https://registry.npmjs.org/kokoro-js/latest and https://registry.npmjs.org/phonemizer/latest (tarball inspected locally in scratchpad, accessed 2026-10-02)
- https://github.com/lucasjinreal/Kokoros (accessed 2026-10-02)
- https://github.com/rhasspy/piper (accessed 2026-10-02)
- https://github.com/OHF-Voice/piper1-gpl and https://github.com/OHF-Voice/piper1-gpl/discussions/57 (accessed 2026-10-02)
- https://github.com/KittenML/KittenTTS (accessed 2026-10-02)
- https://github.com/supertone-inc/supertonic (accessed 2026-10-02)
- https://heyneo.com/blog/kokoro-tts-vs-supertonic-3-tts (accessed 2026-10-02)
- https://blog.nemesisnet.co.za/self-hosted-tts-with-kokoro-onnx-what-cpu-only-inference-actually-gets-you/ (accessed 2026-10-02)
- https://github.com/hexgrad/kokoro/issues/291 (accessed 2026-10-02)
- https://picovoice.ai/blog/on-device-tts/ (vendor benchmark, accessed 2026-10-02)
- https://github.com/brailcom/speechd (accessed 2026-10-02)
- C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/src/sonara/kokoro.py
- C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/docs/history/specs/2026-06-18-sonari-kokoro-provisioning-design.md
- C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/docs/history/specs/2026-06-10-sonari-phase3-windows-design.md
- C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/LICENSE

## Sonara codebase: core vs adapters

### Recommendation

Treat distributability as "one shared per-user reader service plus thin host adapters and clients", not a library that each app embeds.

Smallest effective sequence (each its own PR per the user's policy):

1. Protocol 1.1, additive only:
   - HELLO with capabilities and daemon/protocol version.
   - An optional request `id` with coded error replies.
   - Generic `session_open`, `focus`, `turn_end` and `ask` types.
   - PROSE documented as a public streaming input.
   - Item lifecycle events and a SPEAK ack carrying the item id.
   - A written semver and deprecation policy and an explicit auth and threat-model section.
   - A JSON Schema plus conformance fixtures.
2. Ingest split: move the Claude decision handlers (choice, plan, permission, choice_answered, await_choice, setup_health) into adapters/claude_code. Replace the `":" in session` heuristic with an explicit per-session source.
3. Adapter interface (install, uninstall, doctor, health cue): move claude_hooks out of WinSupervisorBackend.install and make plugin_root optional, so Sonara installs standalone from a wheel or installer. Then add a Codex adapter: `notify` agent-turn-complete -> SPEAK/turn_end under "codex:<thread-id>".
4. Split TtsBackend into TtsEngine (Kokoro portable, OneCore) and AudioOutput, with an option to stream PCM to a host. Turn summarizer prompts and engines into a SummaryEngine with per-source instructions.
5. Add SONARA_HOME and a lockfile override, plus config flags so a host can disable the daemon's hotkeys and web UI.
6. Ship a stdlib-only sonara-client (Python) and a TypeScript client, so Electron hosts like PrismTerminal integrate in a few lines.

Do steps 1 to 3 first. Together they make Sonara usable by Codex and any text-producing app without porting, while keeping the Claude plugin as just one adapter.

### Findings

CODEBASE ANGLE – Sonara 0.8.3, branch fix/166-final-verification (HEAD f408e72). Read-only; nothing was edited.

== 1. Module classification (verified by reading the source and the import graph) ==
(a) HOST-AGNOSTIC READER CORE (about 3.5k lines)
- Text: cleaner.py (93), assembler.py (282). Pure; golden fixtures are in tests/fixtures/text_rules/.
- Queue and reading: queue.py (SpeechItem), channel.py (165), router.py (289). Pure, with no I/O.
- State: history.py (88), digest_store.py, session_prefs.py, sessions.py (213). Mostly generic, but see coupling C4.
- Daemon engine: daemon/core.py (handler table, SessionRegistry, lock), daemon/controls.py, daemon/settings.py, daemon/playback.py, daemon/cues.py, daemon/state_stream.py, daemon/server.py, daemon/audio.py (policy; the OS work goes through ducker/pauser), daemon/rehydrate.py, daemon/__init__.py (facade).
- Speech orchestration: speaker.py (cancel epochs; it takes an injected say_runner and earcon_player).
- Wire and config: protocol.py, platform/transport.py (OS-free TCP and lockfile), client.py, config.py, config_schema.py, paths.py, lifecycle.py.
- daemon/ingest.py is MIXED: PROSE, FLUSH, SPEAK and the session lifecycle are generic; CHOICE, PLAN, PERMISSION, CHOICE_ANSWERED and the await_choice guard are Claude-specific.
(b) CLAUDE-CODE ADAPTER
- hooks/hooks.json, bin/sonara-hook*, hooks_entry.py (pure event-to-message mapping), install/claude_hooks.py (387; writes ~/.claude/settings.json), .claude-plugin/plugin.json and marketplace.json, commands/*.md.
- daemon/decision_text.py (expects the AskUserQuestion `questions` shape and ExitPlanMode `plan`).
- daemon/setup_health.py (a "/sonara:install" cue keyed on plugin_version).
- install/app_copy.py (plugin_root, .claude-plugin/plugin.json, CLAUDE_PLUGIN_ROOT, CLAUDE_PLUGIN_VERSION).
- install_record.py fields (plugin_root, plugin_version).
- summarizer.py prompts ("a message written by a coding assistant") plus the claude -p / codex exec engines. config_schema.SUMMARY_COMMANDS = ("claude", "codex") is hardcoded.
(c) WINDOWS PLATFORM LAYER
- platform/windows/*: tts.py (605; OneCore, Kokoro dispatch AND winsound playback in one module), hotkeys.py, ducking.py, pausing.py, supervisor.py (526, Task Scheduler and launcher), process.py, singleton.py, child_process.py, earcon*, self_volume.py, keytables.py.
- Behind platform/base.py ABCs, with get_platform() as the single OS switch. It raises "Sonara is Windows-only".
- kokoro.py and kokoro_provision.py are portable engine code (lazy kokoro_onnx import, uv venv), but they are reached only through the Windows TTS backend.
(d) PRODUCT SHELL
- cli.py (408), webui.py (385) with settings.html, install/ (installer, doctor, voices, service, cleanup, deps), keymap.py, daemon/hotkeys.py (controller), daemon/previews.py and previews.py, daemon/startup.py, chatterbox_legacy.py (migration only).
- There is no tray. The daemon itself always starts the settings HTTP server and global hotkeys (daemon/__init__.py around lines 456-478).

== 2. Coupling that blocks another host (verified in code; line refs are in Sonara-wt-final/src/sonara) ==
C1 Message vocabulary is shaped around Claude hook events.
- choice, plan, permission, tool_announce and choice_answered mirror AskUserQuestion, ExitPlanMode, the Notification permission_prompt, PreToolUse and PostToolUse.
- decision_text reads Claude's payload shapes directly.
- protocol.md files PROSE, FLUSH, SET_FOREGROUND and SESSION_START under "Hook messages (hooks to daemon)", so a third-party host has no documented streaming input.
C2 The turn boundary is overloaded onto a sound.
- End of turn is EARCON{kind:"turn_done", session} (ingest.on_earcon).
- The summary settle window and the minqueue flush hang off a message whose public meaning is "play a sound".
- A host that wants turn-end semantics without the chime cannot express that.
C3 SPEAK cannot stream: it is queue-of-one and whole-text.
- It is normalize_for_speech only, with no assembler, no summary and kind "summary".
- A host that streams generated text (Codex, a chat UI, any LLM app) must fall back to the undocumented hook path: PROSE + SET_FOREGROUND + EARCON turn_done + FLUSH.
C4 Session semantics are Claude's.
- Ids are either a Claude session id or "<source>:<tab>". ingest.on_session_end uses the heuristic `":" in session` to decide whether to forget prefs, so the namespace exists by convention only.
- The display name comes from the `cwd` basename (SessionManager._basename).
- Foreground means "the last session that got a prompt". background_policy "earcon_only" means a non-foreground session's PROSE is never voiced unless router.authorize_replay is called; SPEAK special-cases this.
- The host_tab env reading in hooks_entry hardcodes PRISM_TAB_ID.
C5 Claude Code quirks live in core ingest.
- await_choice suppresses the permission prompt that AskUserQuestion also fires, using global truthiness for session-less earcons (ingest.py on_permission and on_earcon).
- SESSION_START carries plugin_version and plugin_root and triggers setup_health's "/sonara:install" cue.
C6 Installing the Claude adapter is wired into the Windows platform layer.
- WinSupervisorBackend.install() calls install.claude_hooks.install_hooks(pythonw, plugin_root) (supervisor.py about line 441); uninstall and doctor_rows do the same. SupervisorBackend.install(..., plugin_root=) in base.py bakes this in.
- installer.py aborts without a Claude plugin root (app_copy.resolve_plugin_root, print_no_plugin_root), so the product cannot be installed except as a Claude plugin, or by pointing at the repo.
C7 The engine and the audio output are fused.
- TtsBackend.run() synthesizes AND plays (winsound) and returns a proc handle.
- Kokoro cannot be reused with another output device, for example an Electron renderer that wants the PCM.
- winsound has no true pause (already noted in embedding-research.md).
C8 Paths and instance model are fixed.
- SONARA_DIR = ~/.sonara is hardcoded (paths.py). There is no SONARA_HOME override except test redirection.
- There is one daemon per user (singleton mutex), discovered via ~/.sonara/daemon.lock.
- lifecycle.ensure_running(), called by client.ensure_daemon, launches via platform supervisor.launch_spec(). A bundling host cannot point the client at its own daemon binary or location.
- The daemon runs from ~/.sonara/app, a copy taken from the plugin tree.
C9 Shell features are always on inside the daemon.
- Hotkeys and the web UI always start. A host that wants to own its own player UI and keys cannot opt out; there is no config flag (21 Settings in config_schema, none for this).
C10 Packaging is one distribution.
- pyproject has the single project "sonara" with the `sonara` script and extras windows and kokoro. Authors still list only the upstream author.
- There is no slim client package and no documented import API for other Python hosts. Other languages need a reimplementation (protocol.md is the only contract).
C11 The state stream leaks Claude kinds.
- now_playing.kind lists choice, plan, permission and tool_announce.
- now_playing text for SPEAK reports kind "summary".

== 3. Clean split with the smallest set of moves (INFERENCE: proposed, not implemented) ==
Target: sonara.core (reader engine plus protocol), sonara.adapters.<host>, sonara.engines.<tts|summary>, sonara.platform.windows, sonara.shell. This stays one repo and one wheel at first.
M1 (ingest split; small)
- Split daemon/ingest.py into ingest/stream.py (PROSE, FLUSH, SPEAK, session lifecycle, FORGET) and adapters/claude_code/decisions.py (CHOICE, PLAN, PERMISSION, CHOICE_ANSWERED, await_choice, selection_cue).
- Register the latter through core.add_handlers from an adapter list, so the core has no Claude handlers by default.
M2 (generic input types, additive)
- Add `turn_end{session}`, separate from the earcon. Keep EARCON turn_done as an alias.
- Add `session_open{session, label, host_tab, source}` and `focus{session}` as public names for SESSION_START and SET_FOREGROUND. Drop the cwd and plugin fields from the generic path.
- Add `ask{session, text, options[]}`: a generic decision that decision_text renders. Claude's choice, plan and permission are translated to it in hooks_entry (adapter side).
- Document PROSE as a public streaming input.
- Namespace session ids explicitly (`source` field or "<source>:<id>" required for non-Claude hosts) and replace the ":" heuristic with a per-session `source` attribute.
M3 (installer)
- Move hook installation out of WinSupervisorBackend into an Adapter interface: install(), uninstall(), doctor_row(), health_cue().
- The installer iterates over the enabled adapters (claude_code now, codex next).
- Make plugin_root optional: install from the wheel itself.
M4 (engines)
- Split TtsBackend into TtsEngine.synthesize(text, voice, rate) -> PCM/WAV bytes (Kokoro portable; OneCore Windows-only) and AudioOutput.play/pause/stop/volume (winsound now; a WASAPI or sounddevice output later for real pause).
- Optionally expose PCM over the protocol so a host can play audio itself.
- Turn summarizer.py into a SummaryEngine with a configurable instruction per source kind, replacing the fixed "coding assistant" text.
M5 (core and shell)
- Add SONARA_HOME and a lockfile override.
- Add config flags hotkeys_enabled and webui_enabled (or a capability a host claims), so an embedding host can own its player UI.
- Move setup_health and app_copy under the Claude adapter.
M6 (packaging)
- Publish a stdlib-only `sonara-client` (protocol.py, transport.py, client.py; cross-platform) plus a TypeScript client for Electron hosts such as PrismTerminal.
- The daemon wheel keeps the windows and kokoro extras.
Codex adapter as the first proof (verified external facts below):
- Codex `notify` fires only `agent-turn-complete`, with `thread-id`, `turn-id`, `cwd`, `input-messages` and `last-assistant-message`. That maps to SPEAK (or session_open + PROSE final + turn_end) with session "codex:<thread-id>".
- Codex also has lifecycle hooks (hooks.json; PreToolUse is shown) for richer events later.
- Today's codebase can already serve this through SPEAK. The real gaps are C3 (no streaming) and C6 (installation).

== 4. protocol.md as a public versioned contract – what is missing (verified against the doc and server.py) ==
- No handshake or capabilities. "v" is advisory and never checked (protocol.py). STATUS carries no daemon or protocol version. A host cannot discover whether SPEAK, SUBSCRIBE or summaries exist, except by unknown-type silence.
  Proposal: an additive HELLO request {client:{name,version}, protocol:[1]} with the reply {daemon_version, protocol, capabilities:[speak, prose_stream, subscribe.state, summaries, hotkeys, webui], limits:{subscribers:4, ...}}. This is the LSP-style capability-flag model (inference).
- No versioning policy beyond "only grows". There is no major-version bump rule, deprecation window, or statement of what "v":2 would mean. Because "v" is never validated, a breaking change cannot be signalled. Proposal: semver for the protocol, reject an unknown major with an error when the request opts in, and keep v1 forever-compatible.
- No errors on requests. Malformed or unknown messages are silently dropped (server.py `continue`; ingest returns None). Only SUBSCRIBE has an `error` event, with free text and no code.
  Proposal: an optional `id` echoed in replies, plus {type:"error", id, code, message}. Codes could follow JSON-RPC (-32700 parse, -32600 invalid request, -32601 unknown method, -32602 invalid params, server range -32000..-32099) or a string-code enum (bad_json, unknown_type, invalid_field, busy, unsupported, unauthorized). Errors would be sent only when an `id` is present, which keeps today's fire-and-forget clients compatible.
- No acknowledgement or item lifecycle. SPEAK returns nothing (no item id). The state stream has no utterance_started, utterance_finished or cancelled events tied to the host's request, so a host cannot show "your text was read" or sync highlighting.
- Auth gives third parties full power. One per-user 64-hex token, the same as the web UI token in the URL, grants everything: shutdown{stay_down}, set_voice, forget_session and keymap reload.
  There is no client identity, no scopes (subscribe-only vs control), and no per-host token or revocation.
  The lockfile path is fixed, so discovery for non-default installs is undefined.
  The same-user threat model is acceptable; the contract should still say so explicitly and add client naming and scopes.
- Limits and transport are partly specified. The 4-subscriber cap, the single event kind, and "state changes may surface at the next check" (coalesced, no per-change guarantee) are documented. Missing: a maximum message size, a maximum text length for SPEAK, encoding of the token line, behaviour on a daemon restart mid-request, and no stdio or child-process transport for apps that want to embed a private instance.
- Hook messages are part of the public doc, but their semantics (await_choice suppression, the plugin fields) are Claude internals. They should move to an adapter appendix, separate from the stable host surface.
- There is no machine-readable schema (JSON Schema) and no conformance suite. The golden text fixtures exist; a protocol conformance fixture set would let TS and other clients be validated.

== Verified vs inference ==
Verified: everything in sections 1 and 2, and the protocol gaps in section 4, from reading the files listed in sources. The Codex notify payload and hooks are from official OpenAI docs (learn.chatgpt.com). The JSON-RPC error codes and the LSP capability handshake are from their specs.
Inference: the M1-M6 split, its ordering, the HELLO and error shapes, the sonara-client and TS client packaging, and treating Codex notify as the first adapter.

### Sources

- C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/docs/protocol.md
- C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/docs/architecture.md
- C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/docs/plans/embedding-research.md
- C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/src/sonara/protocol.py
- C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/src/sonara/daemon/server.py
- C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/src/sonara/daemon/tokens.py
- C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/src/sonara/daemon/ingest.py
- C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/src/sonara/daemon/__init__.py
- C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/src/sonara/daemon/setup_health.py
- C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/src/sonara/hooks_entry.py
- C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/src/sonara/sessions.py
- C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/src/sonara/router.py
- C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/src/sonara/speaker.py
- C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/src/sonara/summarizer.py
- C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/src/sonara/config_schema.py
- C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/src/sonara/paths.py
- C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/src/sonara/lifecycle.py
- C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/src/sonara/client.py
- C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/src/sonara/platform/__init__.py
- C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/src/sonara/platform/base.py
- C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/src/sonara/platform/windows/tts.py
- C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/src/sonara/platform/windows/supervisor.py
- C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/src/sonara/install/app_copy.py
- C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/src/sonara/install/installer.py
- C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/hooks/hooks.json
- C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/.claude-plugin/plugin.json
- C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/pyproject.toml
- https://learn.chatgpt.com/docs/config-file/config-advanced (Codex notify agent-turn-complete payload and hooks; accessed 2026-10-02)
- https://www.jsonrpc.org/specification (JSON-RPC 2.0 error codes; accessed 2026-10-02)
- https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/ (initialize capability negotiation; accessed 2026-10-02)

## Language and runtime for the core

### Recommendation

Make the shipped product a single-instance reader daemon (sidecar) with a versioned local protocol and thin client SDKs; do not make it a library each host links in. Write the long-term core in Rust:
- Crates: windows-rs for ducking, GSMTC pausing and OneCore; cpal or rodio for audio output; global-hotkey for hotkeys.
- TTS: Kokoro through sherpa-onnx's C/Rust API, or ort + misaki-rs with no espeak, choosing after a 1–2 day test of quality and size.
- Clients: a pure TypeScript npm client and a pure Python client first; napi-rs, maturin and C ABI builds only later, if hosts ask for in-process use.

Get there in stages, without breaking the Python plugin:
1. Freeze protocol v1 and turn the existing tests into a black-box conformance suite.
2. Ship today's Python daemon frozen (Nuitka standalone or PyInstaller onedir) together with the thin clients, so PrismTerminal and Codex users can bundle it now.
3. Write the hook client in Rust.
4. Port the daemon to Rust module by module until it passes the suite, and ship it alongside the Python daemon behind a switch.
5. Make Rust the default.

Never bundle the Kokoro model in host installers. Download it on first use with a SHA-256 pin. Drop the GPL eSpeak NG dependency, which kokoro-onnx 0.5.0 pulls in today, before advertising Sonara as something any app can bundle. Rule out Go and C++ for the core, and use Node only for the client SDK.

### Findings

RUNTIME ANGLE: language/runtime for a bundleable Sonara reader core. Repo: C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final (0.8.3). This was a read-only check: nothing was edited or installed.

## Verified facts (repo)
- Size: src/sonara is 12,937 Python lines and tests are about 10.6k (23,543 in all). The largest modules are daemon/summary/pipeline.py (745), platform/windows/tts.py (605), platform/windows/supervisor.py (526), daemon/__init__.py (502), kokoro.py (415), cli.py (408) and daemon/ingest.py (390).
- Windows-specific code sits behind src/sonara/platform/base.py (207 lines) and platform/windows/*:
  - ducking.py uses pycaw/comtypes and IAudioSessionManager2 to set per-app volume.
  - pausing.py uses WinRT GlobalSystemMediaTransportControls.
  - hotkeys.py and keytables.py handle hotkeys.
  - tts.py is OneCore through PyWinRT.
  - earcon.py, and Kokoro playback, go through winsound, which has a single channel (daemon/previews.py:38).
  - So the platform seam already exists, but playback is tied to winsound.
- The Kokoro stack is pinned in src/sonara/requirements-kokoro.txt: kokoro-onnx 0.5.0, onnxruntime 1.27.0, numpy 2.4.6, plus winrt-* and pycaw.
- From PyPI JSON: kokoro-onnx 0.5.0 requires espeakng-loader, phonemizer-fork, numpy and onnxruntime. The onnxruntime 1.27.0 win_amd64 wheel is about 13.4 MB.
- License risk: the current chain already loads eSpeak NG, which is GPL-3. This is a real problem for bundling Sonara into closed or commercial host apps, whatever language the core is written in.
- docs/plans/embedding-research.md:54 already estimates a frozen PyInstaller Sonara at over 200 MB and lists "sherpa-onnx program" as an open question (:67, :115).
- PrismTerminal (C:/Users/Admin/Documents/Claude/Github/PrismTerminal) is Electron + electron-vite + electron-builder. It already ships a pinned native sidecar (whisper, via `fetch:bin`), so a pinned sidecar binary is a pattern the user already uses.

## Verified facts (external, accessed 2026-10-02)
- **sherpa-onnx**:
  - Apache-2.0, with APIs for C, C++, Python, JS, Java, C#, Kotlin, Swift, Go, Dart, Rust and Pascal.
  - Runs on Windows, macOS, Linux, Android, iOS and WASM, and supports Kokoro TTS (C, Go and Node examples).
  - The npm package sherpa-onnx-node ships prebuilt binaries for win-x64, mac x64/arm64 and linux x64/arm64, with the DLLs inside node_modules.
  - The Kokoro multi-lang model is about 345 MB (decibri.com).
- **Rust Kokoro ecosystem**: ort 2.0.0-rc.13, updated 2026-07-28, is still a release candidate. Kokoro crates built on it include kokoro-ort, kokoroxide, kokoros, kokoro-tiny and kokoro-cli.
  - misaki-rs is a Rust port of the Misaki G2P (text-to-phoneme step). Its espeak fallback can be turned off with default-features=false, which leaves no GPL dependency; unknown words are then spelled letter by letter, or handled by a custom Fallback trait.
  - TinyTTS is an existing "Rust + sherpa-onnx + Kokoro" offline TTS for Windows.
- **ort linking**: prebuilt ONNX Runtime binaries by default; static linking is preferred; the `load-dynamic` feature picks the DLL path at runtime; `minimal-build` greatly reduces size.
- **Windows APIs in Rust**: windows-rs has typed IAudioSessionControl2 (Win32::Media::Audio) and GlobalSystemMediaTransportControlsSessionManager (Media::Control), and there is a gsmtc wrapper crate. This covers what ducking.py and pausing.py do today.
- **Global hotkeys in Rust**: the Tauri global-hotkey crate covers Windows, macOS and Linux (X11 only). It needs a Win32 event loop on Windows and the main thread on macOS.
- **Packaging tools**:
  - napi-rs: one small root npm package plus one optional prebuilt package per platform, with CI publishing for Windows, macOS, Linux and FreeBSD.
  - PyO3/maturin abi3: one wheel per platform covers all Python versions.
  - UniFFI: Kotlin, Swift, Python and Ruby built in, with third-party C# and Go.

## Inference: the main point
What developers bundle should be a **sidecar process plus a protocol**, not a library linked into the host.

Ducking and pausing other apps, global hotkeys, "one message, always the last" across sessions, and a single audio output are all machine-wide, one-instance concerns. If Codex, Claude Code and PrismTerminal each embedded their own in-process reader, they would talk over each other and fight over hotkeys. A single daemon per user that every host connects to (this is what Phase 0's SUBSCRIBE/SPEAK/host_tab already is) is the right design whatever the language. So the language choice affects install size, startup time, licensing and maintenance, not the integration model.

## Options compared (effort estimates are inference)
1. **Keep Python, ship it frozen as a sidecar.**
   - Effort: days.
   - Size: 150–250 MB (CPython + numpy + onnxruntime, docs say over 200 MB), plus the model downloaded on first use.
   - Startup: the daemon is long-lived, so this mostly doesn't matter. But every hook today spawns a Python interpreter, which costs roughly 100–300 ms.
   - Cross-platform: winsound, pycaw and PyWinRT all need replacing for macOS/Linux anyway.
   - Downsides: PyInstaller onefile unpacks on every launch and is often flagged by antivirus; prefer onedir or Nuitka standalone. Hosts must still ship a large folder of files.
   - Best as a short-term bridge.
2. **Rust core (recommended target).**
   - Shape: one daemon binary `sonarad` (about 5–15 MB plus the onnxruntime DLL, around 13 MB) and a tiny hook client of roughly 1 MB that starts in milliseconds.
   - Crates: windows-rs (ducking, GSMTC pausing, OneCore SpeechSynthesis), global-hotkey, and cpal or rodio for audio output through WASAPI, CoreAudio and ALSA. cpal/rodio would also replace the single-channel winsound limit.
   - TTS options: sherpa-onnx's Rust/C API, or ort + misaki-rs.
   - Bindings: napi-rs for an npm client, maturin for PyPI, and a C ABI (cbindgen) or UniFFI for native, Go and Swift hosts.
   - Effort: porting about 9–10k lines of core logic (router, channels, ingest, playback, summary pipeline, settings, supervisor) is roughly 4–8 weeks part-time with AI help. The existing roughly 10k lines of Python tests can serve as the spec.
   - Risk: ort is still at rc. The Kokoro text-to-phoneme step must match today's speech quality, or reading quality regresses.
3. **Go.** Easy static binaries and cross-compiling. But Windows COM/WinRT means go-ole or hand-written bindings; audio and Kokoro need cgo (sherpa-onnx-go-windows exists); and a Go library cannot cleanly be loaded into-process by Node or Python hosts. Weaker than Rust on every axis that matters here.
4. **TypeScript/Node.** sherpa-onnx-node and kokoro-js exist, so it is natural for Electron and PrismTerminal. But it only serves Node hosts, would still need native addons for ducking, pausing and hotkeys, and would ship a Node runtime as a sidecar for everyone else. Good only as a thin client SDK, not as the core.
5. **C++ via sherpa-onnx directly.** Smallest and fastest, but the worst fit for one developer to maintain (memory safety, build system, bindings by hand). The useful parts are already reachable from Rust through sherpa-onnx's C API.

## TTS engine and licensing (part verified, part inference)
- Whatever runtime is chosen, the Kokoro model (310–345 MB) dwarfs the binary. It must be downloaded on first use with a SHA-256 pin, never bundled into host installers. OneCore stays the zero-download default on Windows.
- Moving away from the GPL eSpeak NG fallback (to misaki-rs with no espeak, or sherpa-onnx with lexicon files) is a requirement for being "easy to bundle".
- Not verified: whether sherpa-onnx's Kokoro English path itself relies on espeak-ng-data. This needs checking before choosing it.

## Migration path that keeps the Python plugin working
- **M0 (now):** freeze protocol v1 (docs/protocol.md, with versioning) as the product boundary. Turn the existing daemon tests into a black-box conformance suite that talks to the daemon over TCP plus the lockfile token.
- **M1 (weeks 1–2):**
  - Ship the Python daemon frozen as `sonara-daemon` (Nuitka standalone or PyInstaller onedir).
  - Publish thin pure clients that find the lockfile, launch the daemon if it isn't running, and call SPEAK/SUBSCRIBE: `@sonara/client` (npm, pure TypeScript) and `sonara-client` (PyPI).
  - PrismTerminal bundles the sidecar through electron-builder extraResources.
  - The Claude Code plugin is unchanged.
- **M2:** write the hook client (hooks_entry) in Rust first: small, safe, an immediate latency win, and the protocol stays the same.
- **M3:** build the Rust `sonarad` module by module (transport and router, then playback with cpal, then ducking and pausing with windows-rs, then hotkeys, then TTS) until it passes the conformance suite.
  - Ship it alongside the Python daemon behind a config or launcher switch.
  - The installed copy at ~/.sonara/app keeps the Python daemon until the Rust one reaches parity.
  - The settings page (settings.html) and LLM summaries (subprocess calls to claude/codex) port as they are.
- **M4:** make Rust the default and retire the frozen Python. The Python package becomes the plugin, hooks and client only.
  - Optionally publish `sonara-core` as a crate with a C ABI, napi-rs and maturin builds for hosts that want to run without a daemon. Document that it then gives up cross-app arbitration.

### Sources

- C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/pyproject.toml
- C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/src/sonara/requirements-kokoro.txt
- C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/src/sonara/platform/windows/ducking.py
- C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/src/sonara/platform/windows/pausing.py
- C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/src/sonara/platform/windows/earcon.py
- C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/src/sonara/kokoro.py
- C:/Users/Admin/Documents/Claude/Github/Sonara-wt-final/docs/plans/embedding-research.md
- C:/Users/Admin/Documents/Claude/Github/PrismTerminal/package.json
- https://github.com/k2-fsa/sherpa-onnx (accessed 2026-10-02)
- https://k2-fsa.github.io/sherpa/onnx/tts/index.html (accessed 2026-10-02)
- https://www.npmjs.com/package/sherpa-onnx (accessed 2026-10-02)
- https://decibri.com/docs/integrations/tts/sherpa-onnx (accessed 2026-10-02)
- https://pkg.go.dev/github.com/k2-fsa/sherpa-onnx-go-windows (accessed 2026-10-02)
- https://github.com/styayur/TinyTTS (accessed 2026-10-02)
- https://ort.pyke.io/setup/linking (accessed 2026-10-02)
- https://crates.io/api/v1/crates/ort (ort 2.0.0-rc.13, accessed 2026-10-02)
- https://lib.rs/crates/misaki-rs (accessed 2026-10-02)
- https://crates.io/crates/kokoro-cli (accessed 2026-10-02)
- https://github.com/lucasjinreal/Kokoros (accessed 2026-10-02)
- https://news.ycombinator.com/item?id=46642602 (espeak-ng GPL discussion, accessed 2026-10-02)
- https://pypi.org/pypi/kokoro-onnx/0.5.0/json (accessed 2026-10-02)
- https://pypi.org/pypi/onnxruntime/1.27.0/json (accessed 2026-10-02)
- https://microsoft.github.io/windows-docs-rs/doc/windows/Media/Control/struct.GlobalSystemMediaTransportControlsSessionManager.html (accessed 2026-10-02)
- https://microsoft.github.io/windows-docs-rs/doc/windows/Win32/Media/Audio/struct.IAudioSessionControl2.html (accessed 2026-10-02)
- https://lib.rs/crates/global-hotkey (accessed 2026-10-02)
- https://napi.rs/docs/cli/pre-publish (accessed 2026-10-02)
- https://pyo3.rs/v0.29.2/building-and-distribution.html (accessed 2026-10-02)
- https://mozilla.github.io/uniffi-rs/ (accessed 2026-10-02)
