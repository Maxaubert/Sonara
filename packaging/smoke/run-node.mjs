// Smoke test of the npm packages as an app would get them: pack
// @sonara/client and @sonara/runtime-win32-x64 (both built first), install
// the tarballs into an empty temp project (no registry needed: neither has
// dependencies), and run host.mjs there against a temp SONARA_HOME with the
// fake engine.
//
//   cargo build -p sonarad --release
//   (cd clients/ts && npm ci && npm run build)
//   (cd packaging/npm-runtime && npm run build)
//   node packaging/smoke/run-node.mjs
import { execSync, spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = join(here, "..", "..");
const work = mkdtempSync(join(tmpdir(), "sonara-smoke-"));
// npm is npm.cmd on Windows, which needs a shell; the arguments are quoted here.
const q = (s) => `"${s}"`;
const npm = (args, cwd) => execSync(`npm ${args.join(" ")}`, { cwd, encoding: "utf8" }).trim();

try {
  const tarballs = [join(repo, "clients", "ts"), join(repo, "packaging", "npm-runtime")].map((dir) => {
    // --ignore-scripts: the packages are built already (prepack would rebuild).
    const name = npm(["pack", "--ignore-scripts", "--pack-destination", q(work)], dir).split("\n").pop();
    return join(work, name);
  });
  const app = join(work, "app");
  mkdirSync(app);
  writeFileSync(join(app, "package.json"), JSON.stringify({ name: "smoke-app", private: true, type: "module" }));
  npm(["install", "--no-audit", "--no-fund", ...tarballs.map(q)], app);
  const installed = JSON.parse(readFileSync(join(app, "package.json"), "utf8")).dependencies;
  console.log(`installed: ${Object.keys(installed).join(", ")}`);
  copyFileSync(join(here, "host.mjs"), join(app, "host.mjs"));
  const home = join(work, "home");
  const r = spawnSync(process.execPath, ["host.mjs"], {
    cwd: app,
    stdio: "inherit",
    env: { ...process.env, SONARA_HOME: home, SONARA_RUNTIME_ARGS: "--engine fake --idle-exit 2" },
  });
  if (r.status !== 0) process.exit(r.status ?? 1);
  console.log("smoke (node): ok");
} finally {
  // The runtime exits 2 s after the host left; its home may still be busy.
  setTimeout(() => rmSync(work, { recursive: true, force: true, maxRetries: 5, retryDelay: 500 }), 3000);
}
