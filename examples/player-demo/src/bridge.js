// A PlayerClient for the browser: forwards the core API to the demo server
// (server.mjs), which holds the real @sonara/client connection. A page
// never gets the runtime's token, and an Electron renderer would do the
// same over IPC to its main process.

async function post(type, body) {
  const res = await fetch(`/api/${type}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
  const reply = await res.json();
  if (!reply.ok) throw Object.assign(new Error(reply.error.message), { code: reply.error.code });
  return reply;
}

export function bridgeClient() {
  const stateListeners = new Set();
  const closeListeners = new Set();
  let events = null;
  let last = null;
  let closed = false;

  const close = () => {
    if (closed) return;
    closed = true;
    events?.close();
    for (const cb of [...closeListeners]) cb();
  };

  const open = () => {
    events = new EventSource("/api/events");
    events.addEventListener("state", (e) => {
      last = JSON.parse(e.data);
      for (const cb of [...stateListeners]) cb(last);
    });
    // The server lost its runtime, or the server itself went away.
    events.addEventListener("closed", close);
    events.addEventListener("error", () => {
      if (events.readyState === EventSource.CLOSED) close();
    });
  };

  return {
    control: (action) => post("control", { action }),
    set: (key, value) => post("set", { key, value }),
    speak: (text, opts = {}) => post("speak", { text, ...opts }),
    onState(cb) {
      stateListeners.add(cb);
      if (!events) open();
      // Like @sonara/client: a listener added later gets the current state first.
      else if (last) queueMicrotask(() => stateListeners.has(cb) && cb(last));
      return () => stateListeners.delete(cb);
    },
    onClose(cb) {
      closeListeners.add(cb);
      return () => closeListeners.delete(cb);
    },
  };
}
