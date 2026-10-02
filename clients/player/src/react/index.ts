/**
 * @sonara/player/react: `<SonaraPlayer client={client} />` and the hooks
 * it is built from. React 18 or newer is a peer dependency.
 */
export { SonaraPlayer, DEFAULT_LABELS } from "./SonaraPlayer.js";
export type { PlayerLabels, SonaraPlayerProps } from "./SonaraPlayer.js";
export { usePlayerController, usePlayerView } from "./hooks.js";
export { PLAYER_CSS } from "./styles.js";
