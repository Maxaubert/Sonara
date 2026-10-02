// Type check only (npm run typecheck): a SonaraClient from @sonara/client
// is a PlayerClient as is, and its state events fit PlayerState.
import type { SonaraClient, State } from "../../ts/src/index.js";
import { PlayerController } from "../src/index.js";
import type { PlayerClient, PlayerState } from "../src/index.js";

declare const client: SonaraClient;
declare const state: State;

export const asPlayerClient: PlayerClient = client;
export const asPlayerState: PlayerState = state;
export const controller = new PlayerController(client);
