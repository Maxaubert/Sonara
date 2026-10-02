/**
 * @sonara/player: a headless player controller on the core API of
 * `@sonara/client`. No DOM, no framework; the React component lives in
 * `@sonara/player/react`.
 *
 *     const player = new PlayerController(client);
 *     player.subscribe((view) => render(view));
 *     player.toggle();
 */
export { PlayerController } from "./controller.js";
export type { PlayerControllerOptions } from "./controller.js";
export { INITIAL_VIEW, toView } from "./view.js";
export type { Overrides } from "./view.js";
export type {
  PlayerAction,
  PlayerClient,
  PlayerListener,
  PlayerState,
  PlayerStatus,
  PlayerView,
} from "./types.js";
