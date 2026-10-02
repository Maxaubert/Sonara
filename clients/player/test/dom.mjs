// A jsdom window as the global environment for the React tests. Import it
// before React, react-dom or Testing Library.
import { JSDOM } from "jsdom";

const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://localhost/",
  pretendToBeVisual: true,
});
const win = dom.window;
for (const key of Object.getOwnPropertyNames(win)) {
  if (key in globalThis) continue;
  Object.defineProperty(globalThis, key, {
    configurable: true,
    get: () => win[key],
  });
}
// Node 21+ has its own navigator; Testing Library needs the window's.
for (const key of ["window", "document", "navigator"]) {
  Object.defineProperty(globalThis, key, { configurable: true, value: key === "window" ? win : win[key] });
}
globalThis.IS_REACT_ACT_ENVIRONMENT = true;
