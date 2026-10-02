// Fill this package from the repository: the release sonarad.exe, any
// runtime DLLs built next to it (onnxruntime*.dll, once the Kokoro engine
// ships), LICENSE and THIRD_PARTY_NOTICES.md.
//
//   cargo build -p sonarad --release
//   node scripts/build.mjs [--exe path\to\sonarad.exe]
//
// Earcon sounds are not copied: they belong to the agent extension, which
// this runtime does not offer yet.
import { copyFileSync, existsSync, mkdirSync, readdirSync, readFileSync, rmSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const pkgDir = join(dirname(fileURLToPath(import.meta.url)), "..");
const repo = join(pkgDir, "..", "..");
const args = process.argv.slice(2);
const exeArg = args.indexOf("--exe");
const exe = exeArg >= 0 ? args[exeArg + 1] : join(repo, "target", "release", "sonarad.exe");

function fail(message) {
  console.error(`build: ${message}`);
  process.exit(1);
}

if (!exe || !existsSync(exe)) fail(`${exe} not found: run 'cargo build -p sonarad --release' first`);

// The package ships the runtime of the same release.
const pkg = JSON.parse(readFileSync(join(pkgDir, "package.json"), "utf8"));
const cargo = readFileSync(join(repo, "Cargo.toml"), "utf8");
const ws = /\[workspace\.package\][^[]*?^version = "([^"]+)"/ms.exec(cargo);
if (!ws || ws[1] !== pkg.version) fail(`package.json ${pkg.version} != Cargo workspace ${ws && ws[1]}`);

const bin = join(pkgDir, "bin");
rmSync(bin, { recursive: true, force: true });
mkdirSync(bin, { recursive: true });
copyFileSync(exe, join(bin, "sonarad.exe"));
const exeDir = dirname(exe);
for (const f of readdirSync(exeDir)) {
  if (/^onnxruntime.*\.dll$/i.test(f)) copyFileSync(join(exeDir, f), join(bin, f));
}
for (const f of ["LICENSE", "THIRD_PARTY_NOTICES.md"]) {
  const src = join(repo, f);
  if (!existsSync(src)) fail(`${f} missing at the repository root`);
  copyFileSync(src, join(pkgDir, f));
}
console.log(`build: ${pkg.name}@${pkg.version} <- ${exe}`);
for (const f of readdirSync(bin)) console.log(`  bin/${f}`);
