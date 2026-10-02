const assert = require("node:assert/strict");
const { test } = require("node:test");
const path = require("node:path");
const { runtimePath, runtimeDir } = require("../index.js");
const pkg = require("../package.json");

test("runtimePath points at bin/sonarad.exe next to the package", () => {
  assert.equal(runtimePath(), path.join(__dirname, "..", "bin", "sonarad.exe"));
  assert.equal(runtimeDir(), path.join(__dirname, "..", "bin"));
});

test("a path inside app.asar is rewritten to app.asar.unpacked", () => {
  const dir = "C:\\Apps\\Prism\\resources\\app.asar\\node_modules\\@sonara\\runtime-win32-x64";
  assert.equal(
    runtimePath(dir),
    "C:\\Apps\\Prism\\resources\\app.asar.unpacked\\node_modules\\@sonara\\runtime-win32-x64\\bin\\sonarad.exe",
  );
  const other = "C:\\Apps\\app.asarx\\pkg";
  assert.equal(runtimePath(other), path.join(other, "bin", "sonarad.exe"));
});

test("the package installs only on Windows x64 and has no dependencies", () => {
  assert.deepEqual(pkg.os, ["win32"]);
  assert.deepEqual(pkg.cpu, ["x64"]);
  assert.deepEqual(pkg.dependencies ?? {}, {});
  for (const f of ["bin/", "THIRD_PARTY_NOTICES.md", "LICENSE"]) assert.ok(pkg.files.includes(f), f);
});
