// Fill this package from the repository: the release sonarad.exe, the
// runtime files staged next to it by packaging/runtime_dlls.py
// (onnxruntime.dll and its licence files for the Kokoro engine, the Visual
// C++ runtime DLLs it imports), LICENSE and THIRD_PARTY_NOTICES.md.
// Without onnxruntime.dll the package still builds (CI tests use the fake
// engine) but its runtime has no Kokoro; the build warns.
//
//   cargo build -p sonarad --release
//   python packaging/runtime_dlls.py stage target/release
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
const RUNTIME_FILES = [
  "onnxruntime.dll",
  "onnxruntime-LICENSE.txt",
  "onnxruntime-ThirdPartyNotices.txt",
  "msvcp140.dll",
  "msvcp140_1.dll",
  "vcruntime140.dll",
  "vcruntime140_1.dll",
];
for (const f of RUNTIME_FILES) {
  if (existsSync(join(exeDir, f))) copyFileSync(join(exeDir, f), join(bin, f));
}
if (!existsSync(join(bin, "onnxruntime.dll"))) {
  console.warn("build: no onnxruntime.dll next to sonarad.exe, so this package has no Kokoro engine");
}
for (const f of ["LICENSE", "THIRD_PARTY_NOTICES.md"]) {
  const src = join(repo, f);
  if (!existsSync(src)) fail(`${f} missing at the repository root`);
  copyFileSync(src, join(pkgDir, f));
}
console.log(`build: ${pkg.name}@${pkg.version} <- ${exe}`);
for (const f of readdirSync(bin)) console.log(`  bin/${f}`);
