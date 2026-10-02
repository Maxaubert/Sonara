// A fake PlayerClient: records calls, lets a test hold or fail replies, and
// pushes state snapshots the way @sonara/client does (a new listener gets
// the current state first, asynchronously).

export const IDLE = {
  seq: 1,
  now_playing: null,
  queued: 0,
  paused: false,
  muted: false,
  volume: 100,
  rate: 200,
  voice: null,
  engine_status: { engine: "fake" },
};

export function playing(fields = {}, np = {}) {
  return {
    ...IDLE,
    now_playing: { item_id: 7, label: "build", text: "Build finished.", chunk: 0, chunks: 2, ...np },
    ...fields,
  };
}

export class FakeClient {
  constructor() {
    this.calls = [];
    this.stateListeners = new Set();
    this.closeListeners = new Set();
    this.last = null;
    this.seq = 0;
    /** When false, replies wait for release() / fail(). */
    this.auto = true;
    this.held = [];
    this.failWith = null;
  }

  control(action) {
    this.calls.push(["control", action]);
    return this.reply();
  }

  set(key, value) {
    this.calls.push(["set", key, value]);
    return this.reply({ key, value });
  }

  onState(cb) {
    this.stateListeners.add(cb);
    const last = this.last;
    if (last) queueMicrotask(() => this.stateListeners.has(cb) && cb(last));
    return () => this.stateListeners.delete(cb);
  }

  onClose(cb) {
    this.closeListeners.add(cb);
    return () => this.closeListeners.delete(cb);
  }

  /** Push a state snapshot to every listener. */
  emit(state) {
    this.seq += 1;
    this.last = { ...state, seq: this.seq };
    for (const cb of [...this.stateListeners]) cb(this.last);
  }

  close() {
    for (const cb of [...this.closeListeners]) cb();
  }

  /** Settle the oldest held reply (with an error when given). */
  release(err) {
    const h = this.held.shift();
    if (!h) throw new Error("no held reply");
    if (err) h.reject(err);
    else h.resolve({});
  }

  releaseAll() {
    while (this.held.length) this.release();
  }

  reply(value = {}) {
    if (this.failWith) return Promise.reject(this.failWith);
    if (this.auto) return Promise.resolve(value);
    return new Promise((resolve, reject) => this.held.push({ resolve, reject }));
  }

  actions() {
    return this.calls.filter((c) => c[0] === "control").map((c) => c[1]);
  }
}

export const tick = () => new Promise((r) => setTimeout(r, 0));
export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
