// Run every test/*.test.{mjs,cjs} with node:test against the build in dist/.
// Node 18 has no glob support in `node --test`, and npm runs scripts through
// cmd.exe on Windows, which does not expand wildcards, so the file list is
// built here.
import { spawnSync } from "node:child_process";
import { readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const files = readdirSync(join(root, "test"))
  .filter((f) => /\.test\.(mjs|cjs)$/.test(f))
  .sort()
  .map((f) => join("test", f));
const r = spawnSync(process.execPath, ["--test", ...files], { cwd: root, stdio: "inherit" });
process.exit(r.status === null ? 1 : r.status);
