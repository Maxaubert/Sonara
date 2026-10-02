// Mark each build folder with its module system. The package itself has no
// "type", so without these Node would read dist/esm as CommonJS.
import { writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const dist = join(dirname(fileURLToPath(import.meta.url)), "..", "dist");
writeFileSync(join(dist, "esm", "package.json"), JSON.stringify({ type: "module" }) + "\n");
writeFileSync(join(dist, "cjs", "package.json"), JSON.stringify({ type: "commonjs" }) + "\n");
