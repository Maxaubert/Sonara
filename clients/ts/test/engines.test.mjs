// Unit tests of `client.engines` against the fake runtime: the request
// shapes of the external engine messages (protocol 1.2).
import assert from "node:assert/strict";
import { afterEach, beforeEach, describe, it } from "node:test";
import { connect, EnginesApi } from "../dist/esm/index.js";
import { removeHome, startFake, tmpHome } from "./helpers.mjs";

let home;
let fake;
let client;

beforeEach(async () => {
  home = tmpHome();
  fake = await startFake(home);
  client = await connect({ clientName: "unit", home, autostart: false });
});

afterEach(async () => {
  await client?.close();
  fake?.stop();
  removeHome(home);
});

describe("engines", () => {
  it("is an EnginesApi on the client", () => {
    assert.ok(client.engines instanceof EnginesApi);
  });

  it("sends the engine_* messages with the wire field names", async () => {
    const profile = { id: "kgpu", kind: "openai-compatible", url: "http://127.0.0.1:8880/v1", options: { preset: "kokoro-fastapi" } };
    await client.engines.list();
    await client.engines.add(profile);
    await client.engines.add(profile, { secret: "sk-x", replace: true });
    await client.engines.setKey("kgpu", "sk-y");
    await client.engines.setKey("kgpu", null);
    await client.engines.test("kgpu");
    await client.engines.test("kgpu", { text: "Hi.", voice: "af_sky", play: false });
    await client.engines.voices("kgpu", { refresh: true });
    await client.voices("kgpu", { refresh: true });
    await client.engines.remove("kgpu");
    await client.engines.remove("kgpu", { forgetKey: false });
    await client.engines.reload();
    const sent = fake.requests().slice(1).map(({ id, ...rest }) => rest);
    assert.deepEqual(sent, [
      { type: "engine_list" },
      { type: "engine_add", engine: profile },
      { type: "engine_add", engine: profile, secret: "sk-x", replace: true },
      { type: "engine_key", engine: "kgpu", secret: "sk-y" },
      { type: "engine_key", engine: "kgpu", secret: null },
      { type: "engine_test", engine: "kgpu" },
      { type: "engine_test", engine: "kgpu", text: "Hi.", voice: "af_sky", play: false },
      { type: "voices", engine: "kgpu", refresh: true },
      { type: "voices", engine: "kgpu", refresh: true },
      { type: "engine_remove", engine: "kgpu" },
      { type: "engine_remove", engine: "kgpu", forget_key: false },
      { type: "engine_reload" },
    ]);
  });
});
