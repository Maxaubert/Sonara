// External engines end to end: a real sonarad.exe (`--engine fake --keys
// fake`, so no Credential Manager) and a local fake OpenAI-compatible
// server. Skipped when no sonarad build is found.
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { join } from "node:path";
import { afterEach, beforeEach, describe, it } from "node:test";
import { connect, SonaraError } from "../dist/esm/index.js";
import { alive, findSonarad, kill, readJson, removeHome, tmpHome, waitFor } from "./helpers.mjs";

const SONARAD = findSonarad();
const skip = SONARAD ? false : "sonarad.exe not built (cargo build -p sonarad) and SONARAD not set";
const SECRET = "sk-ts-client-secret-0123456789";

function wav(samples = 4800, rate = 24000) {
  const data = Buffer.alloc(samples * 2, 0);
  for (let i = 0; i < samples; i++) data.writeInt16LE(2000, i * 2);
  const h = Buffer.alloc(44);
  h.write("RIFF", 0);
  h.writeUInt32LE(36 + data.length, 4);
  h.write("WAVEfmt ", 8);
  h.writeUInt32LE(16, 16);
  h.writeUInt16LE(1, 20);
  h.writeUInt16LE(1, 22);
  h.writeUInt32LE(rate, 24);
  h.writeUInt32LE(rate * 2, 28);
  h.writeUInt16LE(2, 32);
  h.writeUInt16LE(16, 34);
  h.write("data", 36);
  h.writeUInt32LE(data.length, 40);
  return Buffer.concat([h, data]);
}

/** A fake provider: POST /v1/audio/speech answers `answer`. */
async function provider() {
  const seen = [];
  const state = { status: 200, type: "audio/wav", body: wav() };
  const server = createServer((req, res) => {
    const chunks = [];
    req.on("data", (c) => chunks.push(c));
    req.on("end", () => {
      seen.push({ url: req.url, headers: req.headers, body: Buffer.concat(chunks).toString() });
      const r = req.url.endsWith("/audio/voices")
        ? { status: 200, type: "application/json", body: JSON.stringify({ voices: ["af_heart"] }) }
        : state;
      res.writeHead(r.status, { "Content-Type": r.type, Connection: "close" });
      res.end(r.body);
    });
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  return {
    url: `http://127.0.0.1:${server.address().port}/v1`,
    seen,
    fail(status, error) {
      Object.assign(state, { status, type: "application/json", body: JSON.stringify({ error }) });
    },
    close: () => new Promise((resolve) => server.close(resolve)),
  };
}

let home;
let clients;
let fakeProvider;

beforeEach(async () => {
  home = tmpHome();
  clients = [];
  fakeProvider = await provider();
});

afterEach(async () => {
  for (const c of clients) await c.close();
  const info = readJson(join(home, "runtime.json"));
  if (info) {
    kill(info.pid);
    await waitFor(() => !alive(info.pid), 5000);
  }
  await fakeProvider.close();
  removeHome(home);
});

async function open() {
  const c = await connect({
    clientName: "ts-engines",
    home,
    runtimePath: SONARAD,
    runtimeArgs: ["--engine", "fake", "--keys", "fake", "--idle-exit", "5"],
  });
  clients.push(c);
  return c;
}

describe("external engines against sonarad", { skip }, () => {
  it("adds a profile with a key, tests it, lists it and removes it", async () => {
    const c = await open();
    assert.ok(c.info.capabilities.includes("engines"));
    const profile = {
      id: "local",
      kind: "openai-compatible",
      url: fakeProvider.url,
      key_ref: "credman",
      options: { preset: "kokoro-fastapi", timeout_ms: 5000 },
    };
    const added = await c.engines.add(profile, { secret: SECRET });
    assert.equal(added.engine.key_present, true);
    assert.equal(JSON.stringify(added).includes(SECRET), false);
    const t = await c.engines.test("local", { play: false });
    assert.equal(t.sample_rate, 24000);
    const speech = fakeProvider.seen.filter((s) => s.url.endsWith("/audio/speech"));
    assert.equal(speech[0].headers.authorization, `Bearer ${SECRET}`);
    const voices = await c.voices("local", { refresh: true });
    assert.deepEqual(voices.map((v) => v.id), ["af_heart"]);
    assert.equal(voices[0].license_class, "external");
    const list = await c.engines.list();
    assert.deepEqual(list.engines.map((e) => e.id), ["local"]);
    fakeProvider.fail(401, { message: "Incorrect API key provided" });
    await assert.rejects(c.engines.test("local", { play: false }), (err) => {
      assert.ok(err instanceof SonaraError);
      assert.equal(err.code, "E_ENGINE");
      assert.equal(err.reason, "auth");
      return true;
    });
    const removed = await c.engines.remove("local");
    assert.equal(removed.removed, "local");
    assert.deepEqual((await c.engines.list()).engines, []);
  });
});
