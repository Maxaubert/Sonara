"use strict";
// @sonara/runtime-win32-x64: where the bundled sonarad.exe lives.
const path = require("node:path");

/**
 * Absolute path of the bundled sonarad.exe. Inside a packaged Electron app
 * a path in app.asar is not executable, so it points at app.asar.unpacked
 * (list this package in electron-builder `asarUnpack`, or copy bin/ with
 * `extraResources` and use process.resourcesPath instead).
 */
function runtimePath(dir = __dirname) {
  return unpacked(path.join(dir, "bin", "sonarad.exe"));
}

/** Rewrite a path inside app.asar to app.asar.unpacked. */
function unpacked(p) {
  return p.replace(/([\\/])app\.asar([\\/])/, "$1app.asar.unpacked$2");
}

/** Folder with sonarad.exe and the files that ship next to it. */
function runtimeDir(dir = __dirname) {
  return unpacked(path.join(dir, "bin"));
}

module.exports = { runtimePath, runtimeDir };
