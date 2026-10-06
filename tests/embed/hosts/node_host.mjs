// An app that uses Sonara for speech the way a Node or Electron developer
// would (#274): only the public API of the installed packages
// @sonara/client and @sonara/runtime-win32-x64. tests/embed/test_node_host.py
// packs both, installs the tarballs into an empty project next to a copy
// of this file and runs it.
//
// Environment (from the test; an app sets none of these):
//   SONARA_HOME            a temp home
//   SONARA_EMBED_ARGS      JSON list of runtime arguments (the fake engine
//                          and --output wav:<dir> for the scenario)
//   SONARA_EMBED_MODE      "scenario" (the whole API, fake engine) or
//                          "voice" (one sentence with a real engine)
//   SONARA_EMBED_PROVIDER  a fake OpenAI-compatible server's /v1 URL
//   SONARA_EMBED_RUNTIME   another sonarad.exe (the released zip), else the
//                          one in @sonara/runtime-win32-x64
//
// It checks the protocol side itself (phases, state) and prints a JSON
// report as its last line; the test checks the WAV files against it.
import { connect } from "@sonara/client";
import { runtimePath } from "@sonara/runtime-win32-x64";

const args = JSON.parse(process.env.SONARA_EMBED_ARGS || "[]");
const mode = process.env.SONARA_EMBED_MODE || "scenario";
const runtime = process.env.SONARA_EMBED_RUNTIME || runtimePath();
const SECRET = "sk-embed-node-0123456789";

function check(cond, what) {
  if (!cond) throw new Error(`check failed: ${what}`);
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** Follows item phases and state snapshots of one client. */
function tracker(client) {
  const phases = new Map();
  const states = [];
  const waiters = new Set();
  const poke = () => {
    for (const w of [...waiters]) if (w.test()) { waiters.delete(w); clearTimeout(w.timer); w.resolve(); }
  };
  client.onItem((e) => {
    if (!phases.has(e.item_id)) phases.set(e.item_id, []);
    phases.get(e.item_id).push(e.phase);
    poke();
  });
  client.onState((s) => { states.push(s); poke(); });
  const until = (test, what, ms = 15000) => {
    if (test()) return Promise.resolve();
    return new Promise((resolve, reject) => {
      const w = { test, resolve };
      waiters.add(w);
      // Cleared once met, so a finished host exits at once.
      w.timer = setTimeout(() => { if (waiters.delete(w)) reject(new Error(`timed out: ${what}`)); }, ms);
    });
  };
  const ended = (id) => (phases.get(id) || []).find((p) => p !== "started");
  return {
    phases,
    states,
    until,
    last: () => states[states.length - 1],
    started: (id) => until(() => (phases.get(id) || []).includes("started"), `item ${id} started`),
    end: async (id, ms) => { await until(() => ended(id), `item ${id} ended`, ms); return ended(id); },
  };
}

async function scenario() {
  const sonara = await connect({ clientName: "embed-node", runtimePath: runtime, runtimeArgs: args, extensions: ["channels"] });
  const t = tracker(sonara);
  await sonara.eventsReady();
  check(sonara.info.protocol.major === 1, "protocol 1");
  for (const cap of ["speak", "control", "set", "get", "voices", "subscribe", "engines"]) {
    check(sonara.info.capabilities.includes(cap), `capability ${cap}`);
  }
  check(sonara.info.extensions.includes("channels"), "channels enabled");
  // A second app discovers the running instance (runtime.json) instead
  // of starting another.
  const other = await connect({ clientName: "embed-node-other", runtimePath: runtime, runtimeArgs: args });
  check(other.runtime.pid === sonara.runtime.pid && other.runtime.port === sonara.runtime.port, "shared instance");
  await other.close();
  await t.until(() => t.states.length > 0, "first state");
  const items = {};
  const texts = {
    first: "First sentence here.",
    second: "Second one.",
    long: "This long item keeps the reader busy. It has three sentences. The third one ends it.",
    queued: "Queued and then replaced.",
    replacement: "The replacement.",
    cut: "This item is cut short by another one. It never gets to the end of its text.",
    cut_in: "Cut in.",
    paused: "This item is paused and resumed. Then it goes on to its end.",
    skipped: "This item is skipped. Its second sentence is never heard.",
    after_skip: "After the skip.",
    stopped: "This item is stopped. Nothing after it is read.",
    never_read: "Never read.",
    silent: "Silent words.",
    fast: "Hello world.",
    provider: "From the provider.",
  };

  // Settings: rate 200 makes the fake engine 10 ms per character.
  check((await sonara.set("rate", 200)) === 200, "set rate");
  check((await sonara.get("rate")) === 200, "get rate");
  check((await sonara.set("voice", "tone")) === "tone", "set voice");
  const voices = await sonara.voices();
  check(["tone", "silence"].every((v) => voices.some((x) => x.id === v)), "fake voices listed");

  // Append: read in order.
  items.first = await sonara.speak(texts.first, { label: "first" });
  items.second = await sonara.speak(texts.second);
  await t.started(items.first);
  check((await t.end(items.first)) === "finished", "first finished");
  check((await t.end(items.second)) === "finished", "second finished");
  check(t.states.some((s) => s.now_playing && s.now_playing.label === "first"), "now_playing.label");

  // Replace: the unread item is dropped, the current one goes on.
  items.long = await sonara.speak(texts.long);
  await t.started(items.long);
  items.queued = await sonara.speak(texts.queued);
  items.replacement = await sonara.speak(texts.replacement, { mode: "replace" });
  check((await t.end(items.queued)) === "skipped", "queued skipped by replace");
  check((await t.end(items.long)) === "finished", "the current item survives replace");
  check((await t.end(items.replacement)) === "finished", "replacement finished");

  // Interrupt: the current item is cut.
  items.cut = await sonara.speak(texts.cut);
  await t.started(items.cut);
  items.cut_in = await sonara.speak(texts.cut_in, { interrupt: true });
  check((await t.end(items.cut)) === "skipped", "interrupted item skipped");
  check((await t.end(items.cut_in)) === "finished", "cut_in finished");

  // Pause and resume.
  items.paused = await sonara.speak(texts.paused);
  await t.started(items.paused);
  await sonara.control("pause");
  await t.until(() => t.last().paused === true, "paused state");
  await sleep(800);
  check(!t.phases.get(items.paused).includes("finished"), "a paused item does not finish");
  await sonara.control("play");
  await t.until(() => t.last().paused === false, "resumed state");
  check((await t.end(items.paused)) === "finished", "paused item finished after resume");

  // Skip.
  items.skipped = await sonara.speak(texts.skipped);
  items.after_skip = await sonara.speak(texts.after_skip);
  await t.started(items.skipped);
  await sonara.control("skip");
  check((await t.end(items.skipped)) === "skipped", "skip");
  check((await t.end(items.after_skip)) === "finished", "the next item after skip");

  // Stop: the current item and the queue.
  items.stopped = await sonara.speak(texts.stopped);
  items.never_read = await sonara.speak(texts.never_read);
  await t.started(items.stopped);
  await sonara.control("stop");
  check((await t.end(items.stopped)) === "skipped", "stop cuts the current item");
  check((await t.end(items.never_read)) === "skipped", "stop clears the queue");
  await t.until(() => t.last().now_playing === null && t.last().queued === 0, "idle after stop");

  // Voice and rate reach the engine (the test checks the WAV files).
  await sonara.set("voice", "silence");
  check((await sonara.get("voice")) === "silence", "get voice");
  items.silent = await sonara.speak(texts.silent);
  check((await t.end(items.silent)) === "finished", "silent finished");
  await sonara.set("voice", "tone");
  await sonara.set("rate", 400);
  items.fast = await sonara.speak(texts.fast);
  check((await t.end(items.fast)) === "finished", "fast finished");
  await sonara.set("rate", 200);

  // Mute and volume.
  await sonara.control("mute");
  await t.until(() => t.last().muted === true, "muted");
  await sonara.control("unmute");
  await t.until(() => t.last().muted === false, "unmuted");
  check((await sonara.set("volume", 80)) === 80, "volume");
  await t.until(() => t.last().volume === 80, "volume in state");

  // Channels: two sources share the reader, one at a time.
  const opened = await sonara.channels.open("tab-build", { label: "Build" });
  check(opened.created === true, "channel created");
  await sonara.channels.open("tab-tests", { label: "Tests", policy: "queue" });
  await sonara.channels.speak("tab-build", "Build done.");
  await sonara.channels.speak("tab-tests", "Tests passed.");
  const channelItems = {};
  const seen = () => {
    for (const s of t.states) {
      const n = s.now_playing;
      if (n && n.text === "Build done." && n.channel === "tab-build") channelItems.channel_build = n.item_id;
      if (n && n.text === "Tests passed." && n.channel === "tab-tests") channelItems.channel_tests = n.item_id;
    }
    return channelItems.channel_build && channelItems.channel_tests;
  };
  await t.until(seen, "both channels read");
  await t.end(channelItems.channel_tests);
  check(t.states.some((s) => s.now_playing && s.now_playing.channel === "tab-tests" && /^Tests\.?$/.test(s.now_playing.text)),
    "the hand-off to another channel is announced with its label");
  await sonara.channels.nextChannel();
  await t.until(() => t.last().now_playing !== null, "next_channel reads");
  await sonara.control("stop");
  await sonara.channels.close("tab-build");
  await sonara.channels.close("tab-tests");
  await t.until(() => t.last().now_playing === null, "idle after channels");

  // External engine: a provider the user adds (here a local fake server).
  const list = await sonara.engines.list();
  for (const k of ["openai-compatible", "elevenlabs", "azure", "google", "gemini", "cartesia", "deepgram", "command"]) {
    check(list.kinds.includes(k), `kind ${k}`);
  }
  const profile = {
    id: "embed-provider",
    kind: "openai-compatible",
    url: process.env.SONARA_EMBED_PROVIDER,
    voice: "af_heart",
    options: { preset: "kokoro-fastapi", timeout_ms: 5000 },
  };
  const added = await sonara.engines.add(profile, { secret: SECRET });
  check(added.engine.key_present === true && !JSON.stringify(added).includes(SECRET), "engine added, key hidden");
  const tested = await sonara.engines.test("embed-provider", { play: false });
  check(tested.sample_rate === 24000 && tested.duration_ms > 0, `engine_test ${JSON.stringify(tested)}`);
  await sonara.engines.test("embed-provider", { text: "A preview.", play: true });
  const pv = await sonara.voices("embed-provider", { refresh: true });
  check(pv.some((v) => v.id === "af_heart" && v.license_class === "external"), "provider voices");
  check((await sonara.set("engine", "embed-provider")) === "embed-provider", "set engine");
  items.provider = await sonara.speak(texts.provider);
  check((await t.end(items.provider)) === "finished", "provider item finished");
  const st = t.last().engine_status;
  check(st.engine === "embed-provider" && st.ready === true, `engine_status ${JSON.stringify(st)}`);
  await sonara.set("engine", "fake");
  const removed = await sonara.engines.remove("embed-provider");
  check(removed.removed === "embed-provider", "engine removed");

  const version = sonara.info.version;
  await sonara.close();
  return { host: "node", version, items, texts, channel_items: channelItems, secret: SECRET, phases: Object.fromEntries(t.phases) };
}

/** One sentence with a real engine: wait until it speaks with its own voice. */
async function voice() {
  const sonara = await connect({ clientName: "embed-node-voice", runtimePath: runtime, runtimeArgs: args });
  const t = tracker(sonara);
  await sonara.eventsReady();
  await t.until(() => t.states.length > 0, "first state");
  // Kokoro loads its model first (a pre-seeded home: no download).
  // "unavailable": it cannot run here; the test decides (only OneCore may skip).
  const settled = () => {
    const st = t.last().engine_status;
    return st && (st.ready === true || st.status === "unavailable");
  };
  await t.until(settled, "the engine is ready", 180000);
  const engineStatus = t.last().engine_status;
  if (!engineStatus.ready) {
    await sonara.close();
    return { host: "node", unavailable: true, engine_status: engineStatus };
  }
  const item = await sonara.speak(process.env.SONARA_EMBED_SENTENCE);
  const phase = await t.end(item, 60000);
  // The status once the item finished: the same engine, still ready, no
  // fallback, so the item was this engine's voice.
  const after = t.last().engine_status;
  await sonara.close();
  return { host: "node", version: sonara.info.version, item, phase, engine_status: engineStatus,
    engine_status_after: after };
}

const report = await (mode === "voice" ? voice() : scenario());
console.log(JSON.stringify(report));
