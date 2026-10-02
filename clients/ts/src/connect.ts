import { SonaraClient } from "./client.js";
import { Connection } from "./connection.js";
import { liveRuntime, resolveHome, sleep, startRuntime, waitExit } from "./discovery.js";
import { SonaraError } from "./errors.js";
import type { HelloInfo, RuntimeInfo } from "./types.js";
import { PROTOCOL, VERSION } from "./version.js";

export interface ConnectOptions {
  /** Your app's name, sent in `hello` (informational). */
  clientName: string;
  /** Your app's version (default: this package's version). */
  clientVersion?: string;
  /**
   * The bundled `sonarad.exe`, started when no usable runtime is running.
   * Default: `SONARA_RUNTIME` from the environment. With
   * `@sonara/runtime-win32-x64`: `runtimePath()` from that package.
   */
  runtimePath?: string;
  /** Home folder (default: `SONARA_HOME`, else `%LOCALAPPDATA%\Sonara`). */
  home?: string;
  /** Start the bundled runtime when none is usable (default true). */
  autostart?: boolean;
  /** Capabilities or extensions this client cannot work without. */
  require?: string[];
  /** Extensions to enable (`channels`, `agent`, `system`). */
  extensions?: string[];
  /** Keep the runtime running after the last client left. */
  keepAlive?: boolean;
  /** Extra command-line arguments for a started runtime (tests: `["--engine", "fake"]`). */
  runtimeArgs?: string[];
  /** Wait for a started runtime's `runtime.json` (default 5000 ms). */
  startTimeoutMs?: number;
  /** Retry the takeover of a busy, incompatible runtime this long (default 30000 ms). */
  takeoverTimeoutMs?: number;
  /** Wait between takeover retries (default 250 ms). */
  takeoverRetryMs?: number;
}

const CONNECT_TIMEOUT_MS = 5000;
const EXIT_WAIT_MS = 5000;

/** Why an instance cannot serve this client. */
class Incompatible extends Error {}

interface Greeting {
  hello: Record<string, unknown>;
  require: string[];
}

/**
 * Connect to the shared Sonara runtime (spec section 3):
 *
 * 1. Read `runtime.json` in the home; when its pid is alive, connect and
 *    send `hello`.
 * 2. Use it when it speaks protocol 1 and offers everything in `require`.
 * 3. Otherwise (and with `autostart`), start `runtimePath --home <home>`,
 *    wait up to 5 s for its `runtime.json`, then `hello`.
 * 4. An incompatible running instance is first asked to step down
 *    (`hello` with `takeover: true`); while it is busy the takeover is
 *    retried for up to 30 s, then `E_INCOMPATIBLE`.
 */
export async function connect(opts: ConnectOptions): Promise<SonaraClient> {
  if (!opts || typeof opts.clientName !== "string" || !opts.clientName) {
    throw new SonaraError("E_BAD_REQUEST", "connect() needs a clientName");
  }
  const home = resolveHome(opts.home);
  const runtimePath = opts.runtimePath ?? process.env.SONARA_RUNTIME;
  const canStart = (opts.autostart ?? true) && !!runtimePath;
  const require = opts.require ?? [];
  const hello: Record<string, unknown> = {
    client: { name: opts.clientName, version: opts.clientVersion ?? VERSION },
    protocol: { ...PROTOCOL },
    require,
    extensions: opts.extensions ?? [],
  };
  if (opts.keepAlive) hello.keep_alive = true;
  const greeting: Greeting = { hello, require };

  const running = liveRuntime(home);
  if (running) {
    try {
      const client = await greet(running, greeting);
      if (client) return client;
    } catch (err) {
      if (!(err instanceof Incompatible)) throw err;
      if (!canStart) {
        throw new SonaraError(
          "E_INCOMPATIBLE",
          `the running Sonara ${running.version} cannot serve this client (${err.message}) and there is no runtime to start in its place`,
        );
      }
      await takeOver(home, opts, err.message);
    }
  }

  if (!canStart) {
    throw new SonaraError(
      "E_NOT_RUNNING",
      runtimePath
        ? "no Sonara runtime is running and autostart is off"
        : "no Sonara runtime is running and no runtimePath to start one",
    );
  }
  const started = await startRuntime(runtimePath!, home, opts.runtimeArgs ?? [], opts.startTimeoutMs ?? 5000);
  try {
    const client = await greet(started, greeting);
    if (client) return client;
    throw new SonaraError("E_CLOSED", "the started runtime closed the connection");
  } catch (err) {
    if (err instanceof Incompatible) {
      throw new SonaraError(
        "E_INCOMPATIBLE",
        `the bundled Sonara ${started.version} cannot serve this client: ${err.message}`,
      );
    }
    throw err;
  }
}

/**
 * Connect and send `hello`. Null when the instance cannot be reached (it is
 * exiting); throws Incompatible when it answers but cannot serve us.
 */
async function greet(info: RuntimeInfo, g: Greeting): Promise<SonaraClient | null> {
  const dial = async (): Promise<Connection> => {
    const conn = await Connection.open(info.port, CONNECT_TIMEOUT_MS);
    try {
      await conn.request("hello", { ...g.hello, token: info.token });
    } catch (err) {
      conn.close();
      throw err;
    }
    return conn;
  };

  let conn: Connection;
  try {
    conn = await Connection.open(info.port, CONNECT_TIMEOUT_MS);
  } catch {
    return null;
  }
  let reply: Record<string, unknown>;
  try {
    reply = await conn.request("hello", { ...g.hello, token: info.token });
  } catch (err) {
    conn.close();
    if (err instanceof SonaraError && (err.code === "E_INCOMPATIBLE" || err.code === "E_UNSUPPORTED")) {
      throw new Incompatible(err.message);
    }
    if (err instanceof SonaraError && err.code === "E_CLOSED") return null;
    throw err;
  }
  const helloInfo = reply as unknown as HelloInfo;
  const offered = new Set([...(helloInfo.capabilities ?? []), ...(helloInfo.extensions ?? [])]);
  const missing = g.require.filter((r) => !offered.has(r));
  if (helloInfo.protocol?.major !== PROTOCOL.major || missing.length) {
    conn.close();
    throw new Incompatible(
      missing.length ? `it does not offer ${missing.join(", ")}` : `it speaks protocol ${helloInfo.protocol?.major}`,
    );
  }
  return new SonaraClient(conn, info, helloInfo, dial);
}

/**
 * Ask the running instance to exit so the bundled runtime can start. It
 * accepts only when idle; while it is busy, retry until `takeoverTimeoutMs`.
 * Each attempt uses a fresh connection: the runtime closes one that has no
 * successful `hello` within 5 s.
 */
async function takeOver(home: string, opts: ConnectOptions, why: string): Promise<void> {
  const deadline = Date.now() + (opts.takeoverTimeoutMs ?? 30000);
  const retryMs = opts.takeoverRetryMs ?? 250;
  const client = { name: opts.clientName, version: opts.clientVersion ?? VERSION };
  for (;;) {
    const cur = liveRuntime(home);
    if (!cur) return;
    let conn: Connection | null = null;
    try {
      conn = await Connection.open(cur.port, CONNECT_TIMEOUT_MS);
      await conn.request("hello", { token: cur.token, client, takeover: true });
      conn.close();
      if (await waitExit(cur.pid, EXIT_WAIT_MS)) return;
    } catch (err) {
      conn?.close();
      const code = err instanceof SonaraError ? err.code : "";
      if (code !== "E_BUSY" && code !== "E_CLOSED") {
        throw new SonaraError(
          "E_INCOMPATIBLE",
          `the running Sonara cannot serve this client (${why}) and refused a takeover: ${(err as Error).message}`,
        );
      }
    }
    if (Date.now() >= deadline) {
      throw new SonaraError(
        "E_INCOMPATIBLE",
        `the running Sonara cannot serve this client (${why}) and stayed busy; gave up the takeover`,
      );
    }
    await sleep(retryMs);
  }
}
