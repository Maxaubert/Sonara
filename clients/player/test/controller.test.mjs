// PlayerController against a fake client: state to view model, buttons to
// controls, optimistic values, coalescing and rapid presses.
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { describe, it } from "node:test";
import { fileURLToPath } from "node:url";
import { PlayerController, toView } from "../dist/esm/index.js";
import { FakeClient, IDLE, playing, sleep, tick } from "./fake-client.mjs";

function setup(opts = {}) {
  const client = new FakeClient();
  const player = new PlayerController(client, { settleMs: 30, ...opts });
  const views = [];
  const off = player.subscribe((v) => views.push(v));
  return { client, player, views, off };
}

describe("view model", () => {
  it("starts idle and not ready, with default values", () => {
    const { player } = setup();
    const v = player.view;
    assert.equal(v.ready, false);
    assert.equal(v.connected, true);
    assert.equal(v.status, "idle");
    assert.equal(v.progress, 0);
    assert.equal(v.canPrevious, false);
    assert.equal(v.canNext, false);
    assert.equal(v.error, null);
  });

  it("maps a playing state", () => {
    const { client, player } = setup();
    client.emit(playing({ volume: 60, rate: 250, voice: "zira", queued: 1 }, { chunk: 1, chunks: 4, text: "Two warnings." }));
    const v = player.view;
    assert.equal(v.ready, true);
    assert.equal(v.status, "playing");
    assert.equal(v.label, "build");
    assert.equal(v.text, "Two warnings.");
    assert.equal(v.itemId, 7);
    assert.equal(v.chunk, 1);
    assert.equal(v.chunks, 4);
    assert.equal(v.progress, 0.5);
    assert.equal(v.queued, 1);
    assert.equal(v.canPrevious, true);
    assert.equal(v.canNext, true);
    assert.equal(v.volume, 60);
    assert.equal(v.rate, 250);
    assert.equal(v.voice, "zira");
  });

  it("maps paused, idle and a missing label", () => {
    const { client, player } = setup();
    client.emit(playing({ paused: true }, { label: null }));
    assert.equal(player.view.status, "paused");
    assert.equal(player.view.label, null);
    client.emit({ ...IDLE, paused: true });
    assert.equal(player.view.status, "idle", "idle wins over a held pause");
    assert.equal(player.view.text, "");
    assert.equal(player.view.itemId, null);
    assert.equal(player.view.progress, 0);
  });

  it("enables previous after the first chunk and next while there is more to read", () => {
    const { client, player } = setup();
    client.emit(playing({}, { chunk: 0, chunks: 2 }));
    assert.deepEqual([player.view.canPrevious, player.view.canNext], [false, true]);
    client.emit(playing({}, { chunk: 1, chunks: 2 }));
    assert.deepEqual([player.view.canPrevious, player.view.canNext], [true, false]);
    client.emit(playing({ queued: 2 }, { chunk: 1, chunks: 2 }));
    assert.equal(player.view.canNext, true, "next moves on to a queued item");
    assert.equal(player.view.progress, 1);
  });

  it("keeps the chunk inside the item and progress inside 0..1", () => {
    const v = toView(playing({}, { chunk: 9, chunks: 3 }), {}, true, null);
    assert.equal(v.chunk, 2);
    assert.equal(v.progress, 1);
  });

  it("notifies only on change, with a new object each time", () => {
    const { client, views } = setup();
    client.emit(playing());
    client.emit(playing());
    assert.equal(views.length, 1);
    client.emit(playing({}, { chunk: 1 }));
    assert.equal(views.length, 2);
    assert.notEqual(views[0], views[1]);
  });

  it("shows a lost runtime as disconnected", () => {
    const { client, player } = setup();
    client.emit(playing());
    client.close();
    assert.equal(player.view.connected, false);
  });
});

describe("actions", () => {
  it("map buttons to protocol controls", async () => {
    const { client, player } = setup();
    client.emit(playing());
    await player.play();
    await player.pause();
    await player.toggle();
    await player.previous();
    await player.next();
    await player.restart();
    await player.skip();
    await player.stop();
    await player.mute();
    await player.unmute();
    assert.deepEqual(client.actions(), [
      "play", "pause", "toggle", "previous", "next", "restart", "skip", "stop", "mute", "unmute",
    ]);
  });

  it("toggleMute follows the shown mute", async () => {
    const { client, player } = setup();
    client.emit(IDLE);
    await player.toggleMute();
    client.emit({ ...IDLE, muted: true });
    await player.toggleMute();
    assert.deepEqual(client.actions(), ["mute", "unmute"]);
  });

  it("rounds and clamps volume and rate", async () => {
    const { client, player } = setup();
    await player.setVolume(150);
    await player.setVolume(-3);
    await player.setVolume(41.6);
    await player.setRate(50);
    await player.setRate(999);
    await player.setVolume(Number.NaN);
    assert.deepEqual(client.calls, [
      ["set", "volume", 100],
      ["set", "volume", 0],
      ["set", "volume", 42],
      ["set", "rate", 100],
      ["set", "rate", 400],
    ]);
  });

  it("shows pause at once, then follows the runtime", async () => {
    const { client, player } = setup();
    client.emit(playing());
    client.auto = false;
    const done = player.pause();
    assert.equal(player.view.status, "paused", "shown before the reply");
    client.release();
    await done;
    client.emit(playing({ paused: true }));
    assert.equal(player.view.status, "paused");
    client.emit(playing({ paused: false }));
    assert.equal(player.view.status, "playing", "a later change by someone else is shown");
  });

  it("drops a shown value the runtime never confirms", async () => {
    const { client, player } = setup();
    client.emit(playing());
    await player.pause();
    assert.equal(player.view.status, "paused");
    await sleep(80);
    assert.equal(player.view.status, "playing");
  });

  it("does not show a pause with nothing to read", async () => {
    const { client, player } = setup();
    client.emit(IDLE);
    client.auto = false;
    const done = player.toggle();
    assert.equal(player.view.status, "idle");
    client.release();
    await done;
    assert.deepEqual(client.actions(), ["toggle"]);
  });

  it("sends every rapid toggle in order and ends on the runtime's state", async () => {
    const { client, player } = setup();
    client.emit(playing());
    client.auto = false;
    const shown = [];
    const presses = [];
    for (let i = 0; i < 5; i++) {
      presses.push(player.toggle());
      shown.push(player.view.status);
    }
    assert.deepEqual(client.actions(), ["toggle", "toggle", "toggle", "toggle", "toggle"]);
    assert.deepEqual(shown, ["paused", "playing", "paused", "playing", "paused"]);
    // The runtime applies them one by one; states for the first ones arrive
    // while later presses are still on their way and must not flip the view.
    client.release();
    client.emit(playing({ paused: true }));
    client.release();
    client.emit(playing({ paused: false }));
    assert.equal(player.view.status, "paused", "still the last press while three are in flight");
    client.releaseAll();
    await Promise.all(presses);
    client.emit(playing({ paused: true }));
    assert.equal(player.view.status, "paused");
    await sleep(60);
    assert.equal(player.view.status, "paused");
  });

  it("coalesces a dragged volume into one request in flight and the last value", async () => {
    const { client, player } = setup();
    client.emit(IDLE);
    client.auto = false;
    const all = [10, 20, 30, 40].map((v) => player.setVolume(v));
    assert.equal(player.view.volume, 40, "the slider shows the newest value at once");
    assert.deepEqual(client.calls, [["set", "volume", 10]]);
    client.release();
    await tick();
    assert.deepEqual(client.calls, [["set", "volume", 10], ["set", "volume", 40]]);
    assert.equal(player.view.volume, 40);
    client.release();
    await Promise.all(all);
    client.emit({ ...IDLE, volume: 40 });
    assert.equal(player.view.volume, 40);
    client.emit({ ...IDLE, volume: 70 });
    assert.equal(player.view.volume, 70, "the override is gone once confirmed");
  });

  it("reports a failed action in the view, reverts it, and clears it on the next success", async () => {
    const { client, player } = setup();
    client.emit(playing());
    client.failWith = Object.assign(new Error("runtime gone"), { code: "E_CLOSED" });
    await player.pause(); // never rejects
    assert.equal(player.view.error.message, "runtime gone");
    assert.equal(player.view.status, "playing", "the shown pause is reverted");
    await player.setVolume(10);
    assert.equal(player.view.volume, 100);
    client.failWith = null;
    await player.next();
    assert.equal(player.view.error, null);
  });

  it("does nothing after dispose", async () => {
    const { client, player, views } = setup();
    client.emit(playing());
    player.dispose();
    await player.toggle();
    await player.setVolume(5);
    client.emit(playing({ paused: true }));
    assert.equal(client.calls.length, 0);
    assert.equal(views.length, 1);
    assert.equal(client.stateListeners.size, 0);
  });
});

describe("client subscription", () => {
  it("listens to the client only while it has subscribers", async () => {
    const client = new FakeClient();
    const player = new PlayerController(client);
    assert.equal(client.stateListeners.size, 0);
    const off = player.subscribe(() => undefined);
    assert.equal(client.stateListeners.size, 1);
    assert.equal(client.closeListeners.size, 1);
    client.emit(playing());
    off();
    assert.equal(client.stateListeners.size, 0);
    assert.equal(client.closeListeners.size, 0);
    // Subscribing again (React StrictMode does this) picks up the current state.
    client.last = { ...client.last, now_playing: { ...client.last.now_playing, chunk: 1 } };
    player.subscribe(() => undefined);
    await tick();
    assert.equal(player.view.chunk, 1);
  });

  it("works with a client without onClose", () => {
    const client = new FakeClient();
    client.onClose = undefined;
    const player = new PlayerController(client);
    player.subscribe(() => undefined);
    client.emit(IDLE);
    assert.equal(player.view.ready, true);
  });

  it("refuses something that is not a client", () => {
    assert.throws(() => new PlayerController({}), TypeError);
    assert.throws(() => new PlayerController(null), TypeError);
  });
});

describe("package", () => {
  const dist = join(dirname(fileURLToPath(import.meta.url)), "..", "dist");

  it("keeps the headless part free of React and the DOM", () => {
    for (const sys of ["esm", "cjs"]) {
      for (const f of readdirSync(join(dist, sys)).filter((f) => f.endsWith(".js"))) {
        const src = readFileSync(join(dist, sys, f), "utf8");
        assert.doesNotMatch(src, /["']react/, `${sys}/${f} imports react`);
        assert.doesNotMatch(src, /\b(document|window)\./, `${sys}/${f} touches the DOM`);
      }
    }
  });

  it("loads from CommonJS", () => {
    const require = createRequire(import.meta.url);
    const cjs = require("../dist/cjs/index.js");
    assert.equal(typeof cjs.PlayerController, "function");
  });
});
