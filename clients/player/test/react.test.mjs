// <SonaraPlayer/> in jsdom against a fake client: rendering per state, aria
// attributes, keyboard operation and focus order.
import "./dom.mjs";
import assert from "node:assert/strict";
import { afterEach, describe, it } from "node:test";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement as h } from "react";
import { PlayerController } from "../dist/esm/index.js";
import { PLAYER_CSS, SonaraPlayer } from "../dist/esm/react/index.js";
import { FakeClient, IDLE, playing } from "./fake-client.mjs";

afterEach(cleanup);

async function mount(state, props = {}) {
  const client = new FakeClient();
  if (state) client.last = { ...state, seq: 1 };
  const utils = render(h("div", null, h(SonaraPlayer, { client, ...props }), h("button", null, "after")));
  // The current state reaches the player asynchronously, as from @sonara/client.
  await act(async () => undefined);
  const player = screen.getByRole("group");
  return { client, player, ...utils };
}

const button = (name) => screen.getByRole("button", { name });
const disabled = (el) => el.getAttribute("aria-disabled") === "true";
const emit = (client, state) => act(() => client.emit(state));

describe("rendering", () => {
  it("shows connecting until the first state", async () => {
    const client = new FakeClient();
    render(h(SonaraPlayer, { client }));
    assert.ok(screen.getByText("Connecting"));
    assert.ok(disabled(button("Play")));
    await emit(client, IDLE);
    assert.ok(screen.getByText("Nothing playing"));
  });

  it("shows idle with only restart, mute and volume usable", async () => {
    await mount(IDLE);
    assert.ok(screen.getByRole("group", { name: "Sonara player" }));
    assert.ok(screen.getByText("Nothing playing"));
    assert.ok(!disabled(button("Restart")), "restart replays the last item when idle");
    for (const name of ["Previous sentence", "Play", "Next sentence", "Stop"]) assert.ok(disabled(button(name)), name);
    assert.ok(!disabled(button("Mute")));
    assert.equal(screen.getByRole("slider", { name: "Volume" }).getAttribute("aria-disabled"), null);
  });

  it("shows the item, the sentence and its position while playing", async () => {
    await mount(playing({}, { chunk: 1, chunks: 4, text: "Two warnings." }));
    assert.ok(screen.getByText("build"));
    assert.ok(screen.getByText("Two warnings."));
    assert.ok(button("Pause"), "the play button offers pause while playing");
    const bar = screen.getByRole("progressbar", { name: "Progress" });
    assert.equal(bar.getAttribute("aria-valuenow"), "2");
    assert.equal(bar.getAttribute("aria-valuemax"), "4");
    assert.equal(bar.getAttribute("aria-valuetext"), "Sentence 2 of 4");
    assert.equal(bar.firstElementChild.style.transform, "scaleX(0.5)");
    assert.equal(screen.getByRole("group").dataset.status, "playing");
  });

  it("falls back to a generic title without a label, and marks a pause", async () => {
    const { client } = await mount(playing({}, { label: null }));
    assert.ok(screen.getByText("Reading"));
    await emit(client, playing({ paused: true }, { label: null }));
    assert.ok(screen.getByText("Paused"));
    assert.ok(button("Play"));
  });

  it("announces the item and its pause in a polite live region, not each sentence", async () => {
    const { client } = await mount(playing({ paused: true }));
    const live = document.querySelector("[aria-live]");
    assert.equal(live.getAttribute("aria-live"), "polite");
    assert.equal(live.getAttribute("aria-atomic"), "true");
    assert.equal(live.textContent, "buildPaused");
    assert.ok(!live.textContent.includes("Build finished."));
    await emit(client, playing({}, { chunk: 1, text: "Second sentence." }));
    assert.equal(live.textContent, "build");
  });

  it("disables every control once the runtime is gone", async () => {
    const { client } = await mount(playing());
    await act(() => client.close());
    assert.ok(screen.getByText("Not connected"));
    for (const b of screen.getAllByRole("button").filter((b) => b.textContent !== "after")) assert.ok(disabled(b));
    assert.equal(screen.getByRole("slider").getAttribute("aria-disabled"), "true");
  });

  it("shows a failed action", async () => {
    const { client } = await mount(playing());
    client.failWith = new Error("E_ENGINE: no voice");
    await act(async () => fireEvent.click(button("Pause")));
    assert.equal(screen.getByRole("status").textContent, "E_ENGINE: no voice");
  });

  it("takes translated labels, a theme and a class", async () => {
    await mount(IDLE, { labels: { player: "Lecteur", play: "Lire" }, theme: "dark", className: "mine" });
    const group = screen.getByRole("group", { name: "Lecteur" });
    assert.equal(group.dataset.theme, "dark");
    assert.ok(group.classList.contains("sonara-player"));
    assert.ok(group.classList.contains("mine"));
    assert.ok(button("Lire"));
  });

  it("ships a themeable stylesheet with dark mode and reduced motion, unless unstyled", async () => {
    await mount(IDLE);
    assert.equal(document.querySelector("style").textContent, PLAYER_CSS);
    assert.match(PLAYER_CSS, /prefers-color-scheme: dark/);
    assert.match(PLAYER_CSS, /prefers-reduced-motion: reduce/);
    assert.match(PLAYER_CSS, /var\(--sonara-player-accent/);
    cleanup();
    await mount(IDLE, { unstyled: true });
    assert.equal(document.querySelector("style"), null);
  });

  it("uses a controller it is given", async () => {
    const client = new FakeClient();
    client.last = { ...playing(), seq: 1 };
    const controller = new PlayerController(client);
    render(h(SonaraPlayer, { controller }));
    await act(async () => undefined);
    await act(async () => fireEvent.click(button("Next sentence")));
    assert.deepEqual(client.actions(), ["next"]);
  });
});

describe("keyboard", () => {
  it("tabs through the controls in visual order and out of the player", async () => {
    await mount(playing({ queued: 1 }, { chunk: 1, chunks: 3 }));
    const user = userEvent.setup();
    const order = [];
    for (let i = 0; i < 8; i++) {
      await user.tab();
      const el = document.activeElement;
      order.push(el.getAttribute("aria-label") ?? el.textContent);
    }
    assert.deepEqual(order, [
      "Restart", "Previous sentence", "Pause", "Next sentence", "Stop", "Mute", "Volume", "after",
    ]);
    await user.tab({ shift: true });
    assert.equal(document.activeElement.getAttribute("aria-label"), "Volume", "no trap either way");
  });

  it("keeps a control that does not apply focusable, and inert", async () => {
    const { client } = await mount(IDLE);
    const user = userEvent.setup();
    await user.tab();
    await user.tab();
    assert.equal(document.activeElement, button("Previous sentence"));
    await user.keyboard("{Enter}");
    await user.tab();
    assert.equal(document.activeElement, button("Play"));
    await user.keyboard(" ");
    assert.deepEqual(client.actions(), []);
  });

  it("operates every button with Enter and Space", async () => {
    const { client } = await mount(playing({ queued: 1 }, { chunk: 1, chunks: 3 }));
    const user = userEvent.setup();
    for (const [name, key] of [
      ["Restart", "{Enter}"],
      ["Previous sentence", " "],
      ["Pause", "{Enter}"],
      ["Next sentence", " "],
      ["Stop", "{Enter}"],
      ["Mute", " "],
    ]) {
      button(name).focus();
      await user.keyboard(key);
    }
    assert.deepEqual(client.actions(), ["restart", "previous", "toggle", "next", "stop", "mute"]);
  });

  it("keeps focus on play/pause while its label flips", async () => {
    const { client } = await mount(playing());
    const user = userEvent.setup();
    const pause = button("Pause");
    pause.focus();
    await user.keyboard("{Enter}");
    assert.equal(document.activeElement, pause);
    assert.equal(pause.getAttribute("aria-label"), "Play", "shown at once");
    await emit(client, playing({ paused: true }));
    assert.equal(document.activeElement, button("Play"));
  });

  it("marks mute as a toggle button", async () => {
    const { client } = await mount(IDLE);
    const mute = button("Mute");
    assert.equal(mute.getAttribute("aria-pressed"), "false");
    await act(async () => fireEvent.click(mute));
    assert.equal(mute.getAttribute("aria-pressed"), "true");
    await emit(client, { ...IDLE, muted: true });
    await act(async () => fireEvent.click(mute));
    assert.deepEqual(client.actions(), ["mute", "unmute"]);
  });

  it("sets the volume from the slider and says it in percent", async () => {
    const { client } = await mount({ ...IDLE, volume: 80 });
    const slider = screen.getByRole("slider", { name: "Volume" });
    assert.equal(slider.value, "80");
    assert.equal(slider.getAttribute("aria-valuetext"), "80%");
    await act(async () => fireEvent.change(slider, { target: { value: "35" } }));
    assert.equal(slider.getAttribute("aria-valuetext"), "35%");
    assert.deepEqual(client.calls.at(-1), ["set", "volume", 35]);
  });

  it("puts every icon out of the accessibility tree", async () => {
    const { player } = await mount(playing());
    for (const svg of player.querySelectorAll("svg")) {
      assert.equal(svg.getAttribute("aria-hidden"), "true");
      assert.equal(svg.getAttribute("focusable"), "false");
    }
    for (const b of within(player).getAllByRole("button")) assert.ok(b.getAttribute("aria-label"));
    for (const b of within(player).getAllByRole("button")) assert.equal(b.getAttribute("type"), "button");
  });
});
