/**
 * @sonara/client: talk to the Sonara runtime (protocol v1) from Node 18+
 * or an Electron main process. Zero dependencies.
 *
 *     const sonara = await connect({ clientName: "my-app", runtimePath });
 *     sonara.onState((s) => render(s));
 *     await sonara.speak("Hello.");
 */
export { connect } from "./connect.js";
export type { ConnectOptions } from "./connect.js";
export { SonaraClient } from "./client.js";
export type { Unsubscribe } from "./client.js";
export { SonaraError } from "./errors.js";
export type { ErrorCode } from "./errors.js";
export { AgentApi, ChannelsApi, SystemApi } from "./extensions.js";
export { EnginesApi } from "./engines.js";
export type {
  EngineAddOptions,
  EngineModelsDraft,
  EngineProfile,
  SendMode,
  EngineTestOptions,
} from "./engines.js";
export type { AskKind, AudioMode, ChannelOpenOptions, ChannelPolicy, StreamMessage } from "./extensions.js";
export { readRuntime, resolveHome } from "./discovery.js";
export { PROTOCOL, VERSION } from "./version.js";
export type {
  ControlAction,
  EngineStatus,
  HelloInfo,
  ItemEvent,
  ItemPhase,
  LogEvent,
  NowPlaying,
  Reply,
  RuntimeInfo,
  SettingKey,
  SpeakMode,
  SpeakOptions,
  State,
  Voice,
} from "./types.js";
