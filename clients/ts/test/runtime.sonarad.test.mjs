// End to end against a real sonarad.exe built in this workspace
// (`cargo build -p sonarad`, or $SONARAD), with the fake engine and the
// silent output. Skipped when no build is found.
import assert from "node:assert/strict";
import { join } from "node:path";
import { afterEach, beforeEach, describe, it } from "node:test";
import { connect, SonaraError } from "../dist/esm/index.js";
import { alive, findSonarad, kill, readJson, removeHome, sleep, tmpHome, waitFor } from "./helpers.mjs";

const SONARAD = findSonarad();
const skip = SONARAD ? false : "sonarad.exe not built (cargo build -p sonarad) and SONARAD not set";
// The fake engine reads 10 ms per character: this lasts about 3 s per sentence.
const LONG = "This sentence is long so that it keeps playing " + "and playing ".repeat(20) + "for a while.";

let home;
let clients;
let pids;

beforeEach(() => {
  home = tmpHome();
  clients = [];
  pids = new Set();
});

afterEach(async () => {
  for (const c of clients) await c.close();
  const info = readJson(join(home, "runtime.json"));
  if (info) pids.add(info.pid);
  for (const pid of pids) kill(pid);
  await waitFor(() => [...pids].every((p) => !alive(p)), 5000);
  removeHome(home);
});

async function open(opts = {}) {
  const c = await connect({
    clientName: "ts-e2e",
    home,
    runtimePath: SONARAD,
    runtimeArgs: ["--engine", "fake", "--idle-exit", "5"],
    ...opts,
  });
  clients.push(c);
  pids.add(c.runtime.pid);
  return c;
}

describe("against sonarad", { skip }, () => {
  it("autostarts the bundled runtime, speaks and reports the item to its end", async () => {
    const c = await open();
    const info = readJson(join(home, "runtime.json"));
    assert.equal(info.pid, c.runtime.pid);
    assert.ok(c.info.capabilities.includes("speak"));
    const items = [];
    c.onItem((e) => items.push(e));
    await c.eventsReady();
    const id = await c.speak("Hello from the TypeScript client.", { label: "hello" });
    assert.equal(typeof id, "number");
    assert.ok(await waitFor(() => items.some((e) => e.item_id === id && e.phase === "finished"), 10000), JSON.stringify(items));
    assert.deepEqual(
      items.filter((e) => e.item_id === id).map((e) => e.phase),
      ["started", "finished"],
    );
  });

  it("a second client shares the running instance", async () => {
    const a = await open();
    const b = await open();
    assert.equal(a.runtime.pid, b.runtime.pid);
  });

  it("onState follows now_playing, pause and stop", async () => {
    const c = await open();
    const states = [];
    c.onState((s) => states.push(s));
    await c.eventsReady();
    const seen = (pred) => waitFor(() => states.some(pred), 10000);
    assert.ok(await seen(() => true));
    assert.equal(states[0].now_playing, null, "the first event is the current state");
    const id = await c.speak(LONG, { label: "long" });
    assert.ok(await seen((s) => s.now_playing?.item_id === id && s.now_playing.label === "long"));
    await c.control("pause");
    assert.ok(await seen((s) => s.paused === true));
    await c.control("stop");
    const n = states.length;
    assert.ok(await waitFor(() => states.slice(n - 1).some((s) => s.now_playing === null), 10000));
    assert.ok(states.every((s, i) => i === 0 || s.seq > states[i - 1].seq), "seq strictly increases");
  });

  it("set, get and voices", async () => {
    const c = await open();
    assert.equal(await c.set("volume", 35), 35);
    assert.equal(await c.get("volume"), 35);
    assert.equal(await c.set("rate", 250), 250);
    const voices = await c.voices("fake");
    assert.ok(voices.length > 0);
    assert.equal(voices[0].engine, "fake");
    await assert.rejects(c.set("volume", 101), (e) => e instanceof SonaraError && e.code === "E_BAD_REQUEST");
  });

  it("extension namespaces reach the runtime; a namespace not requested answers E_UNSUPPORTED", async () => {
    const c = await open({ extensions: ["channels", "made-up"] });
    assert.deepEqual(c.info.unavailable, ["made-up"]);
    await c.channels.open("tab");
    await assert.rejects(c.system.setAudioMode("duck"), (e) => e.code === "E_UNSUPPORTED");
  });

  it("takes over an idle incompatible instance and starts the bundled one", async () => {
    const a = await open();
    const oldPid = a.runtime.pid;
    // The bundled runtime is the same build, so it lacks `engine.kokoro` too: the
    // old one steps down, a new one starts, and the client then gives up.
    await assert.rejects(
      connect({
        clientName: "needs-kokoro",
        home,
        runtimePath: SONARAD,
        runtimeArgs: ["--engine", "fake", "--idle-exit", "5"],
        require: ["engine.kokoro"],
      }),
      (e) => e.code === "E_INCOMPATIBLE",
    );
    assert.ok(await waitFor(() => !alive(oldPid), 5000), "the idle instance exited for the takeover");
    const fresh = readJson(join(home, "runtime.json"));
    assert.ok(fresh && fresh.pid !== oldPid, "the bundled runtime runs in its place");
    pids.add(fresh.pid);
    assert.ok(a.closed || (await waitFor(() => a.closed, 3000)), "the old instance closed its clients");
  });

  it("never takes over a busy instance; gives up with E_INCOMPATIBLE", async () => {
    const a = await open();
    await a.speak(LONG);
    await sleep(100);
    await assert.rejects(
      connect({
        clientName: "needs-kokoro",
        home,
        runtimePath: SONARAD,
        require: ["engine.kokoro"],
        takeoverTimeoutMs: 800,
      }),
      (e) => e.code === "E_INCOMPATIBLE",
    );
    assert.ok(alive(a.runtime.pid));
    assert.equal(readJson(join(home, "runtime.json")).pid, a.runtime.pid);
    await a.control("stop");
  });

  it("is E_NOT_RUNNING with autostart off and nothing running", async () => {
    await assert.rejects(
      connect({ clientName: "x", home, runtimePath: SONARAD, autostart: false }),
      (e) => e.code === "E_NOT_RUNNING",
    );
  });
});
