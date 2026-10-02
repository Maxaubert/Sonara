// The CommonJS build loads with require() (Electron main processes that
// are not ESM) and matches the ESM build's surface.
const assert = require("node:assert/strict");
const { test } = require("node:test");
const { join } = require("node:path");

test("require() gives the same API as import", async () => {
  const cjs = require("../dist/cjs/index.js");
  const esm = await import("../dist/esm/index.js");
  assert.deepEqual(Object.keys(cjs).sort(), Object.keys(esm).sort());
  assert.equal(typeof cjs.connect, "function");
  const err = new cjs.SonaraError("E_BUSY", "x");
  assert.ok(err instanceof Error);
  assert.equal(err.code, "E_BUSY");
});

test("resolveHome: option, then SONARA_HOME, then LOCALAPPDATA\\Sonara", () => {
  const { resolveHome } = require("../dist/cjs/index.js");
  const env = { SONARA_HOME: "C:\\s", LOCALAPPDATA: "C:\\l" };
  assert.equal(resolveHome("C:\\h", env), "C:\\h");
  assert.equal(resolveHome(undefined, env), "C:\\s");
  assert.equal(resolveHome(undefined, { LOCALAPPDATA: "C:\\l" }), join("C:\\l", "Sonara"));
  assert.throws(() => resolveHome(undefined, {}), (e) => e.code === "E_NOT_RUNNING");
});

test("the package exports map points at both builds", () => {
  const pkg = require("../package.json");
  assert.equal(pkg.exports["."].require.default, "./dist/cjs/index.js");
  assert.equal(pkg.exports["."].import.default, "./dist/esm/index.js");
  assert.deepEqual(pkg.dependencies ?? {}, {}, "zero runtime dependencies (spec R6)");
  assert.equal(require("../dist/cjs/index.js").VERSION, pkg.version);
});
