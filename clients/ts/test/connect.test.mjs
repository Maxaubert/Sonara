// Unit tests against the fake runtime (test/fake-runtime.mjs): the connect
// algorithm, the request shapes and the event connection.
import assert from "node:assert/strict";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, beforeEach, describe, it } from "node:test";
import { connect, SonaraError, VERSION } from "../dist/esm/index.js";
import { alive, removeHome, startFake, tmpHome, waitFor } from "./helpers.mjs";

let home;
let fake;
let clients = [];

beforeEach(() => {
  home = tmpHome();
  fake = null;
  clients = [];
});

afterEach(async () => {
  for (const c of clients) await c.close();
  fake?.stop();
  removeHome(home);
});

async function open(opts = {}) {
  const c = await connect({ clientName: "unit", home, autostart: false, ...opts });
  clients.push(c);
  return c;
}

async function rejectsWith(promise, code) {
  await assert.rejects(promise, (err) => {
    assert.ok(err instanceof SonaraError, `not a SonaraError: ${err}`);
    assert.equal(err.code, code, err.message);
    return true;
  });
}

describe("connect", () => {
  it("uses a running compatible instance and greets it with the token and options", async () => {
    fake = await startFake(home);
    const c = await open({ require: ["core"], extensions: ["channels"], clientVersion: "2.1" });
    assert.equal(c.runtime.pid, fake.pid);
    assert.equal(c.info.version, "9.9.9");
    const hello = fake.requests()[0];
    assert.equal(hello.type, "hello");
    assert.equal(hello.token, "t0k3n");
    assert.deepEqual(hello.client, { name: "unit", version: "2.1" });
    assert.deepEqual(hello.protocol, { major: 1, minor: 0 });
    assert.deepEqual(hello.require, ["core"]);
    assert.deepEqual(hello.extensions, ["channels"]);
    assert.equal(hello.takeover, undefined);
  });

  it("sends this package's version when no clientVersion is given", async () => {
    fake = await startFake(home);
    await open();
    assert.equal(fake.requests()[0].client.version, VERSION);
  });

  it("sends keep_alive only when asked", async () => {
    fake = await startFake(home);
    await open({ keepAlive: true });
    assert.equal(fake.requests()[0].keep_alive, true);
  });

  it("is E_NOT_RUNNING with no runtime.json and nothing to start", async () => {
    await rejectsWith(connect({ clientName: "unit", home }), "E_NOT_RUNNING");
  });

  it("ignores a stale runtime.json whose pid is gone", async () => {
    writeFileSync(
      join(home, "runtime.json"),
      JSON.stringify({ pid: 2 ** 30, port: 1, http_port: 1, token: "x", version: "0", protocol: { major: 1, minor: 0 } }),
    );
    await rejectsWith(connect({ clientName: "unit", home, autostart: false, runtimePath: "x.exe" }), "E_NOT_RUNNING");
  });

  it("needs a clientName", async () => {
    await rejectsWith(connect({ home }), "E_BAD_REQUEST");
  });

  it("is E_INCOMPATIBLE without a takeover when there is nothing to start in its place", async () => {
    fake = await startFake(home, { helloError: "E_UNSUPPORTED" });
    await rejectsWith(connect({ clientName: "unit", home }), "E_INCOMPATIBLE");
    assert.ok(fake.requests().every((r) => !r.takeover), "no takeover may be sent");
    assert.ok(alive(fake.pid));
  });

  it("treats a hello reply with another protocol major as incompatible", async () => {
    fake = await startFake(home, { protocolMajor: 2 });
    await rejectsWith(connect({ clientName: "unit", home, autostart: false }), "E_INCOMPATIBLE");
  });

  it("treats a required capability missing from the reply as incompatible", async () => {
    fake = await startFake(home);
    await rejectsWith(connect({ clientName: "unit", home, autostart: false, require: ["channels"] }), "E_INCOMPATIBLE");
  });

  it("takes over an incompatible idle instance, then starts the bundled runtime", async () => {
    fake = await startFake(home, { helloError: "E_INCOMPATIBLE" });
    const missing = join(home, "no-such-sonarad.exe");
    // The bundled runtime cannot start here, which shows the order: the
    // takeover ran first (the fake exited), then the start was attempted.
    await rejectsWith(connect({ clientName: "unit", home, runtimePath: missing }), "E_START_FAILED");
    const takeover = fake.requests().find((r) => r.takeover);
    assert.ok(takeover, "a takeover hello was sent");
    assert.equal(takeover.token, "t0k3n");
    assert.ok(await waitFor(() => !alive(fake.pid), 3000), "the old instance exited");
  });

  it("retries the takeover while the instance is busy", async () => {
    fake = await startFake(home, { helloError: "E_UNSUPPORTED", busyTakeovers: 2 });
    await rejectsWith(
      connect({ clientName: "unit", home, runtimePath: join(home, "none.exe"), takeoverRetryMs: 20 }),
      "E_START_FAILED",
    );
    assert.equal(fake.requests().filter((r) => r.takeover).length, 3);
    assert.ok(await waitFor(() => !alive(fake.pid), 3000));
  });

  it("gives up with E_INCOMPATIBLE when the instance stays busy past the bound", async () => {
    fake = await startFake(home, { helloError: "E_UNSUPPORTED", busyTakeovers: -1 });
    const t0 = Date.now();
    await rejectsWith(
      connect({
        clientName: "unit",
        home,
        runtimePath: join(home, "none.exe"),
        takeoverTimeoutMs: 400,
        takeoverRetryMs: 50,
      }),
      "E_INCOMPATIBLE",
    );
    assert.ok(Date.now() - t0 >= 400);
    assert.ok(fake.requests().filter((r) => r.takeover).length >= 2);
    assert.ok(alive(fake.pid), "a busy instance is never stopped");
  });

  it("passes a wrong-token E_AUTH through", async () => {
    fake = await startFake(home);
    const info = JSON.parse(readFileSync(join(home, "runtime.json"), "utf8"));
    writeFileSync(join(home, "runtime.json"), JSON.stringify({ ...info, token: "wrong" }));
    await rejectsWith(connect({ clientName: "unit", home, autostart: false }), "E_AUTH");
  });
});

describe("core API", () => {
  it("sends speak, control, set, get and voices as protocol messages", async () => {
    fake = await startFake(home);
    const c = await open();
    assert.equal(await c.speak("Hello.", { mode: "replace", interrupt: true, label: "build" }), 1);
    assert.equal(await c.speak("Again."), 2);
    await c.control("pause");
    assert.equal(await c.set("volume", 40), 40);
    assert.equal(await c.get("volume"), 40);
    const voices = await c.voices("fake");
    assert.equal(voices[0].id, "fake-1");
    const sent = fake.requests().slice(1).map(({ id, ...rest }) => rest);
    assert.deepEqual(sent, [
      { type: "speak", text: "Hello.", mode: "replace", interrupt: true, label: "build" },
      { type: "speak", text: "Again." },
      { type: "control", action: "pause" },
      { type: "set", key: "volume", value: 40 },
      { type: "get", key: "volume" },
      { type: "voices", engine: "fake" },
    ]);
  });

  it("rejects with the runtime's error code", async () => {
    fake = await startFake(home);
    const c = await open();
    await rejectsWith(c.request("fail_me", { code: "E_UNSUPPORTED" }), "E_UNSUPPORTED");
  });

  it("rejects pending and later requests with E_CLOSED when the connection drops", async () => {
    fake = await startFake(home);
    const c = await open();
    let closed = false;
    c.onClose(() => {
      closed = true;
    });
    await rejectsWith(c.request("close_me"), "E_CLOSED");
    assert.ok(c.closed);
    assert.ok(closed);
    await rejectsWith(c.speak("late"), "E_CLOSED");
  });
});

describe("events", () => {
  it("delivers state, item and log on a second, subscribed connection", async () => {
    fake = await startFake(home);
    const c = await open();
    const states = [];
    const items = [];
    const logs = [];
    c.onState((s) => states.push(s));
    c.onItem((i) => items.push(i));
    c.onLog((l) => logs.push(l));
    await c.eventsReady();
    assert.ok(await waitFor(() => states.length && items.length && logs.length, 3000));
    assert.equal(states[0].seq, 1);
    assert.equal(states[0].event, undefined, "the event name is not part of the payload");
    assert.deepEqual(items[0], { item_id: 1, phase: "started" });
    assert.equal(logs[0].message, "hello log");
    const reqs = fake.requests();
    assert.equal(reqs.filter((r) => r.type === "hello").length, 2, "one hello per connection");
    assert.deepEqual(reqs.find((r) => r.type === "subscribe").events, ["state", "items", "log"]);
  });

  it("reports an event connection that cannot open, and retries on the next listener", async () => {
    fake = await startFake(home);
    const c = await open();
    fake.stop();
    await waitFor(() => !alive(fake.pid), 3000);
    const logs = [];
    c.onLog((l) => logs.push(l));
    await rejectsWith(c.eventsReady(), "E_CLOSED");
    assert.match(logs[0].message, /event stream failed/);
    c.onState(() => undefined);
    await rejectsWith(c.eventsReady(), "E_CLOSED");
    assert.equal(logs.length, 2, "the second listener made a second attempt");
  });

  it("stops calling a listener after unsubscribe", async () => {
    fake = await startFake(home);
    const c = await open();
    const seen = [];
    const off = c.onLog((l) => seen.push(l));
    off();
    await c.eventsReady();
    await waitFor(() => false, 200);
    assert.equal(seen.length, 0);
  });
});

describe("extension namespaces", () => {
  it("send the extension message types unchanged", async () => {
    fake = await startFake(home);
    const c = await open();
    await c.channels.open("tab-1", { label: "Tab 1", host_tab: "t1", policy: "latest" });
    await c.channels.focus("tab-1");
    await c.channels.speak("tab-1", "Hi.", { mode: "replace" });
    await c.channels.control("tab-1", "pause");
    await c.channels.nextChannel();
    await c.channels.close("tab-1");
    await c.agent.turnStart("tab-1", 3, { t: 12 });
    await c.agent.stream({ channel: "tab-1", turn: 3, delta: "Hel", index: 0, final: false, t: 12 });
    await c.agent.turnEnd("tab-1", 3);
    await c.agent.ask("tab-1", "permission", "Run it?", ["yes", "no"]);
    await c.agent.earcon("turn_done");
    await c.agent.setMuteLevel(2);
    await c.agent.setSummaries({ enabled: true });
    await c.system.setAudioMode("duck");
    await c.system.setDuckLevel(30);
    await c.system.setHotkeys({ play: "Ctrl+Alt+P" });
    const sent = fake.requests().slice(1).map(({ id, ...rest }) => rest);
    assert.deepEqual(sent, [
      { type: "channel_open", channel: "tab-1", label: "Tab 1", host_tab: "t1", policy: "latest" },
      { type: "focus", channel: "tab-1" },
      { type: "speak", channel: "tab-1", text: "Hi.", mode: "replace" },
      { type: "control", channel: "tab-1", action: "pause" },
      { type: "control", action: "next_channel" },
      { type: "channel_close", channel: "tab-1" },
      { type: "turn_start", channel: "tab-1", turn: 3, t: 12 },
      { type: "stream", channel: "tab-1", turn: 3, delta: "Hel", index: 0, final: false, t: 12 },
      { type: "turn_end", channel: "tab-1", turn: 3 },
      { type: "ask", channel: "tab-1", kind: "permission", text: "Run it?", options: ["yes", "no"] },
      { type: "earcon", kind: "turn_done" },
      { type: "set", key: "mute_level", value: 2 },
      { type: "set", key: "summaries", value: { enabled: true } },
      { type: "set", key: "audio_mode", value: "duck" },
      { type: "set", key: "duck_level", value: 30 },
      { type: "set", key: "hotkeys", value: { play: "Ctrl+Alt+P" } },
    ]);
  });
});
