// Shared test helpers: temp homes, the fake runtime, the real sonarad.
import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
export const REPO = join(here, "..", "..", "..");

export function tmpHome() {
  return mkdtempSync(join(tmpdir(), "sonara-ts-"));
}

export function removeHome(home) {
  try {
    rmSync(home, { recursive: true, force: true });
  } catch {
    // A runtime that is still exiting may hold a file for a moment.
  }
}

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

export async function waitFor(pred, timeoutMs = 10000, stepMs = 20) {
  const end = Date.now() + timeoutMs;
  while (Date.now() < end) {
    if (await pred()) return true;
    await sleep(stepMs);
  }
  return !!(await pred());
}

export function readJson(path) {
  try {
    return JSON.parse(readFileSync(path, "utf8"));
  } catch {
    return null;
  }
}

export function alive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (err) {
    return err.code === "EPERM";
  }
}

export function kill(pid) {
  try {
    process.kill(pid);
  } catch {
    // already gone
  }
}

/** Start the fake runtime on `home` and wait for its runtime.json. */
export async function startFake(home, opts = {}) {
  const child = spawn(process.execPath, [join(here, "fake-runtime.mjs"), JSON.stringify({ home, ...opts })], {
    stdio: "ignore",
  });
  const ok = await waitFor(() => readJson(join(home, "runtime.json"))?.pid === child.pid, 5000);
  if (!ok) throw new Error("the fake runtime wrote no runtime.json");
  return {
    child,
    pid: child.pid,
    requests: () =>
      existsSync(join(home, "requests.jsonl"))
        ? readFileSync(join(home, "requests.jsonl"), "utf8").trim().split("\n").map((l) => JSON.parse(l))
        : [],
    stop: () => kill(child.pid),
  };
}

/** `$SONARAD`, else the newest of target/{release,debug}/sonarad.exe, else null. */
export function findSonarad() {
  if (process.env.SONARAD) return process.env.SONARAD;
  const found = ["release", "debug"]
    .map((p) => join(REPO, "target", p, "sonarad.exe"))
    .filter((p) => existsSync(p));
  if (!found.length) return null;
  return found.sort((a, b) => statSync(b).mtimeMs - statSync(a).mtimeMs)[0];
}
