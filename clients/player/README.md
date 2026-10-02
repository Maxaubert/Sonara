# @sonara/player

A player for the Sonara text-to-speech runtime, built only on the core API of [`@sonara/client`](../ts): a headless `PlayerController` (no DOM, no framework) and a React player bar, `<SonaraPlayer/>`. No runtime dependencies; React 18 or newer is an optional peer, needed only for `@sonara/player/react`.

## React

```jsx
import { connect } from "@sonara/client";
import { SonaraPlayer } from "@sonara/player/react";

const client = await connect({ clientName: "my-app" });

export function App() {
  return <SonaraPlayer client={client} />;
}
```

The bar has restart, previous sentence, play/pause, next sentence, stop, the now-playing label with the sentence being read and its progress, mute and a volume slider.

- **Keyboard and screen readers:** native buttons and a native slider in visual order, each with an accessible name; Tab moves through and out of the bar (no trap). A control that does not apply right now (previous on the first sentence, play with nothing to read) stays focusable with `aria-disabled`, so focus never jumps away. The item label and its paused state are announced in a polite live region; the sentences themselves are not, so the reader is not talked over. Progress is a `progressbar` with "Sentence 2 of 5"; mute is a toggle button (`aria-pressed`); the slider reads its value in percent.
- **Theme:** light and dark follow `prefers-color-scheme`; `theme="light"` or `"dark"` forces one. Colours, radius and font come from CSS custom properties, set on the player or any ancestor: `--sonara-player-bg`, `-fg`, `-muted-fg`, `-accent`, `-on-accent`, `-track`, `-border`, `-hover`, `-focus`, `-danger`, `-radius`, `-font`. The built-in rules have zero specificity (`:where`), so your own CSS wins; `unstyled` leaves the stylesheet out (style the `sonara-player__*` classes yourself).
- **Motion:** button and progress transitions are off under `prefers-reduced-motion: reduce`; forced-colors mode keeps the progress bar visible.
- **Props:** `client` or `controller` (share one controller with other UI), `theme`, `unstyled`, `labels` (every text, for translation), `className`, `style`.
- **Hooks:** `usePlayerController(client)` and `usePlayerView(controller)` to build your own UI on the same view model.

In a browser or an Electron renderer, `@sonara/client` (a Node API) stays in the server or main process; give the player any object with the same `control`, `set`, `onState` (and optionally `onClose`) calls that forwards to it. The demo does that over HTTP.

## Headless

```js
import { PlayerController } from "@sonara/player";

const player = new PlayerController(client);
const off = player.subscribe((view) => render(view));
player.toggle();
player.setVolume(60);
```

`view` is `{ ready, connected, status: "idle" | "playing" | "paused", label, text, itemId, chunk, chunks, progress (0..1), queued, canPrevious, canNext, muted, volume (0..100), rate (wpm), voice, error }`. Actions: `play`, `pause`, `toggle`, `previous`, `next`, `restart`, `skip`, `stop`, `mute`, `unmute`, `toggleMute`, `setVolume`, `setRate`, `dispose`.

- The controller listens to the client while it has at least one subscriber.
- Actions never reject; a failure shows as `view.error` until the next action succeeds.
- Pause, mute, volume and rate show the new value at once and follow the runtime once it confirms (or after `settleMs`, default 1 s, when it never does). `toggle` is sent as the runtime's `toggle`, so rapid presses keep their count. A dragged volume or rate keeps at most one request in flight and sends the newest value next.

## Demo

`examples/player-demo` renders the player in a browser page. A small Node server holds the `@sonara/client` connection (the page never sees the runtime's token) and forwards the player's calls and the state stream over same-origin HTTP.

```sh
cargo build -p sonarad
(cd clients/ts && npm ci && npm run build)
(cd clients/player && npm ci && npm run build)
# a runtime with the fake engine (silent, in real time); omit --engine for real voices
target/debug/sonarad.exe --engine fake --standalone
# in a second terminal
cd examples/player-demo
npm ci
npm start            # http://127.0.0.1:5174 (PORT to change it)
```

The demo finds the runtime like any `@sonara/client` host (`SONARA_HOME`; with `SONARA_RUNTIME` set it starts one itself). Read the sample text, then play, pause, step sentences, mute and change the volume.

## Development

```sh
npm ci
npm run typecheck   # also checks that a SonaraClient is a PlayerClient
npm run build       # tsc to dist/esm and dist/cjs
npm test            # controller with a fake client; React component in jsdom
```

MIT.
