// A stand-in for sonarad.exe in unit tests: a JSON-lines server that writes
// runtime.json, records every request it gets and answers from a small
// script. Run as a child process so its pid can end (an accepted takeover).
//
//   node fake-runtime.mjs <options as JSON>
//
// Options: home (required), helloError (a code every plain hello fails
// with), busyTakeovers (how many takeover hellos answer E_BUSY first; -1:
// always), protocolMajor (in the hello reply).
// Requests are appended to <home>/requests.jsonl.
import { appendFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { join } from "node:path";

const opts = JSON.parse(process.argv[2]);
const token = "t0k3n";
let busy = opts.busyTakeovers ?? 0;
let nextItem = 1;
const settings = { volume: 100, rate: 200, voice: null, engine: "fake" };
const capabilities = ["core", "speak", "control", "set", "get", "voices", "subscribe", "events.state", "events.items", "events.log"];

function helloFields() {
  return {
    version: "9.9.9",
    protocol: { major: opts.protocolMajor ?? 1, minor: 0 },
    capabilities,
    extensions: [],
    unavailable: [],
  };
}

function handle(conn, msg) {
  const reply = (fields) => conn.write(JSON.stringify({ id: msg.id, ok: true, ...fields }) + "\n");
  const fail = (code, message = code) =>
    conn.write(JSON.stringify({ id: msg.id, ok: false, error: { code, message } }) + "\n");
  switch (msg.type) {
    case "hello":
      if (msg.token !== token) return fail("E_AUTH");
      if (msg.takeover) {
        if (busy !== 0) {
          if (busy > 0) busy -= 1;
          return fail("E_BUSY", "something is playing");
        }
        reply({ ...helloFields(), takeover: true });
        conn.end();
        setTimeout(() => {
          rmSync(join(opts.home, "runtime.json"), { force: true });
          process.exit(0);
        }, 50);
        return;
      }
      if (opts.helloError) return fail(opts.helloError, "scripted hello failure");
      return reply(helloFields());
    case "speak":
      return reply({ item_id: nextItem++ });
    case "control":
      return reply({});
    case "set":
      settings[msg.key] = msg.value;
      return reply({ key: msg.key, value: msg.value });
    case "get":
      if (!(msg.key in settings)) return fail("E_UNSUPPORTED", `key ${msg.key}`);
      return reply({ key: msg.key, value: settings[msg.key] });
    case "voices":
      return reply({
        voices: [{ id: "fake-1", name: "Fake", language: "en-US", engine: "fake", license_class: "permissive", installed: true }],
      });
    case "subscribe":
      reply({ events: msg.events });
      conn.write(JSON.stringify({ event: "state", seq: 1, now_playing: null, queued: 0, paused: false, muted: false, volume: 100, rate: 200, voice: null, engine_status: { engine: "fake" } }) + "\n");
      conn.write(JSON.stringify({ event: "item", item_id: 1, phase: "started" }) + "\n");
      conn.write(JSON.stringify({ event: "log", message: "hello log" }) + "\n");
      return;
    case "fail_me":
      return fail(msg.code, "scripted failure");
    case "close_me":
      conn.destroy();
      return;
    default:
      // Extension types: answered so the test sees what was sent.
      return reply({ echoed: msg.type });
  }
}

const server = createServer((conn) => {
  conn.setEncoding("utf8");
  let buf = "";
  conn.on("data", (chunk) => {
    buf += chunk;
    let nl;
    while ((nl = buf.indexOf("\n")) >= 0) {
      const line = buf.slice(0, nl);
      buf = buf.slice(nl + 1);
      const msg = JSON.parse(line);
      appendFileSync(join(opts.home, "requests.jsonl"), JSON.stringify(msg) + "\n");
      handle(conn, msg);
    }
  });
  conn.on("error", () => {});
});

server.listen(0, "127.0.0.1", () => {
  const info = {
    pid: process.pid,
    port: server.address().port,
    http_port: 0,
    token,
    version: "9.9.9",
    protocol: { major: opts.protocolMajor ?? 1, minor: 0 },
    capabilities,
    started_at: new Date().toISOString(),
  };
  const tmp = join(opts.home, "runtime.json.tmp");
  writeFileSync(tmp, JSON.stringify(info));
  renameSync(tmp, join(opts.home, "runtime.json"));
});
