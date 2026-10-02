// The demo's server: connects to sonarad with @sonara/client, serves the
// page, and forwards the core API to it over same-origin HTTP (POST
// /api/<type>) plus the state stream (GET /api/events, Server-Sent Events).
// Loopback only. Settings: PORT (default 5174); the runtime is found as any
// @sonara/client host finds it (SONARA_HOME, SONARA_RUNTIME).
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { connect } from "@sonara/client";

const here = dirname(fileURLToPath(import.meta.url));
const port = Number(process.env.PORT ?? 5174);
const origin = `http://127.0.0.1:${port}`;

let sonara;
try {
  sonara = await connect({ clientName: "player-demo" });
} catch (err) {
  console.error(`Could not reach sonarad (${err.code ?? "error"}): ${err.message}`);
  console.error("Start one first, for example: cargo run -p sonarad -- --engine fake --standalone");
  process.exit(1);
}
console.log(`connected to sonarad ${sonara.info.version}`);

const streams = new Set();
sonara.onState((state) => {
  for (const res of streams) res.write(`event: state\ndata: ${JSON.stringify(state)}\n\n`);
});
sonara.onClose(() => {
  for (const res of streams) res.end("event: closed\ndata: {}\n\n");
  console.error("sonarad went away");
  process.exit(1);
});

// What the page may call: the core API the player needs, plus speak.
const calls = {
  control: (b) => sonara.control(b.action),
  set: (b) => sonara.set(b.key, b.value).then((value) => ({ value })),
  speak: (b) => sonara.speak(String(b.text ?? ""), { mode: b.mode, interrupt: b.interrupt, label: b.label }).then((item_id) => ({ item_id })),
};

const files = {
  "/": ["index.html", "text/html; charset=utf-8"],
  "/app.js": ["app.js", "text/javascript; charset=utf-8"],
};

function send(res, status, body, type = "application/json") {
  res.writeHead(status, { "content-type": type, "cache-control": "no-store" });
  res.end(typeof body === "string" || Buffer.isBuffer(body) ? body : JSON.stringify(body));
}

async function readBody(req) {
  let raw = "";
  for await (const chunk of req) {
    raw += chunk;
    if (raw.length > 1 << 20) throw new Error("body too large");
  }
  return raw ? JSON.parse(raw) : {};
}

const server = createServer(async (req, res) => {
  const url = new URL(req.url, origin);
  // Another site in the same browser must not drive the reader.
  if (req.headers.origin && req.headers.origin !== origin) return send(res, 403, { ok: false, error: { code: "E_AUTH", message: "wrong origin" } });

  if (req.method === "GET" && files[url.pathname]) {
    const [name, type] = files[url.pathname];
    try {
      return send(res, 200, await readFile(join(here, "public", name)), type);
    } catch {
      return send(res, 500, "Run npm run build first.", "text/plain");
    }
  }
  if (req.method === "GET" && url.pathname === "/api/events") {
    res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-store", connection: "keep-alive" });
    streams.add(res);
    // The first state a new listener sees is the current one (@sonara/client replays it).
    const off = sonara.onState((state) => {
      off();
      res.write(`event: state\ndata: ${JSON.stringify(state)}\n\n`);
    });
    req.on("close", () => {
      off();
      streams.delete(res);
    });
    return;
  }
  const type = url.pathname.startsWith("/api/") ? url.pathname.slice(5) : null;
  if (req.method === "POST" && type && calls[type]) {
    if (!String(req.headers["content-type"] ?? "").startsWith("application/json")) {
      return send(res, 415, { ok: false, error: { code: "E_BAD_REQUEST", message: "send JSON" } });
    }
    try {
      const reply = await calls[type](await readBody(req));
      return send(res, 200, { ok: true, ...reply });
    } catch (err) {
      return send(res, 400, { ok: false, error: { code: err.code ?? "E_BAD_REQUEST", message: err.message } });
    }
  }
  send(res, 404, { ok: false, error: { code: "E_NOT_FOUND", message: "not found" } });
});

server.listen(port, "127.0.0.1", () => console.log(`player demo on ${origin}`));
