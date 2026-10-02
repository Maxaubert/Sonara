import { spawn } from "node:child_process";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { SonaraError } from "./errors.js";
import type { RuntimeInfo } from "./types.js";

/** Exit code of a second `sonarad` for the same user and home. */
const EXIT_ALREADY_RUNNING = 3;
const POLL_MS = 50;

/** The home folder: `home`, else `SONARA_HOME`, else `%LOCALAPPDATA%\Sonara`. */
export function resolveHome(home?: string, env: NodeJS.ProcessEnv = process.env): string {
  if (home) return home;
  if (env.SONARA_HOME) return env.SONARA_HOME;
  if (env.LOCALAPPDATA) return join(env.LOCALAPPDATA, "Sonara");
  throw new SonaraError("E_NOT_RUNNING", "no home folder: pass home, or set SONARA_HOME or LOCALAPPDATA");
}

/** `runtime.json` of `home`, or null when it is missing or unreadable. */
export function readRuntime(home: string): RuntimeInfo | null {
  try {
    const info = JSON.parse(readFileSync(join(home, "runtime.json"), "utf8")) as RuntimeInfo;
    if (typeof info.pid !== "number" || typeof info.port !== "number" || typeof info.token !== "string") {
      return null;
    }
    return info;
  } catch {
    return null;
  }
}

/** True while process `pid` exists. */
export function pidAlive(pid: number): boolean {
  if (!Number.isInteger(pid) || pid <= 0) return false;
  try {
    process.kill(pid, 0);
    return true;
  } catch (err) {
    // EPERM: it exists but belongs to someone else.
    return (err as NodeJS.ErrnoException).code === "EPERM";
  }
}

/** `runtime.json` of `home` when its pid is alive. */
export function liveRuntime(home: string): RuntimeInfo | null {
  const info = readRuntime(home);
  return info && pidAlive(info.pid) ? info : null;
}

export const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));

/**
 * Start `runtimePath --home <home> [...args]` without a console window and
 * wait up to `timeoutMs` for a `runtime.json` written by it. When another
 * client started one at the same moment (the new process exits with code 3),
 * the other's `runtime.json` is used instead.
 */
export async function startRuntime(
  runtimePath: string,
  home: string,
  args: readonly string[],
  timeoutMs: number,
): Promise<RuntimeInfo> {
  let exitCode: number | null = null;
  let spawnError: Error | null = null;
  const child = spawn(runtimePath, ["--home", home, ...args], {
    detached: true,
    stdio: "ignore",
    windowsHide: true,
  });
  child.on("error", (err) => {
    spawnError = err;
  });
  child.on("exit", (code) => {
    exitCode = code ?? -1;
  });
  // The runtime outlives this process: it is shared and exits on its own
  // once no client is left.
  child.unref();

  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (spawnError) {
      throw new SonaraError("E_START_FAILED", `cannot start ${runtimePath}: ${(spawnError as Error).message}`);
    }
    const info = readRuntime(home);
    if (info && info.pid === child.pid) return info;
    if (exitCode !== null) {
      if (exitCode !== EXIT_ALREADY_RUNNING) {
        throw new SonaraError("E_START_FAILED", `${runtimePath} exited with code ${exitCode}`);
      }
      const other = liveRuntime(home);
      if (other) return other;
    }
    await sleep(POLL_MS);
  }
  throw new SonaraError("E_START_FAILED", `${runtimePath} wrote no runtime.json within ${timeoutMs} ms`);
}

/**
 * Wait until process `pid` has ended (after an accepted takeover it releases
 * the single-instance lock, removes runtime.json and exits). False when it is
 * still running after `timeoutMs`.
 */
export async function waitExit(pid: number, timeoutMs: number): Promise<boolean> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (!pidAlive(pid)) return true;
    await sleep(POLL_MS);
  }
  return !pidAlive(pid);
}
