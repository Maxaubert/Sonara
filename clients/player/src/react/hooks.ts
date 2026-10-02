import { useMemo, useSyncExternalStore } from "react";
import { PlayerController } from "../controller.js";
import type { PlayerClient, PlayerView } from "../types.js";

/**
 * A `PlayerController` for `client`, kept for as long as the client stays
 * the same. The controller listens to the client only while something
 * renders its view, so nothing needs disposing.
 */
export function usePlayerController(client: PlayerClient): PlayerController {
  return useMemo(() => new PlayerController(client), [client]);
}

/** The controller's view model, re-rendering on every change. */
export function usePlayerView(controller: PlayerController): PlayerView {
  return useSyncExternalStore(controller.subscribe, controller.getSnapshot, controller.getSnapshot);
}
