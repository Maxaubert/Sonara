// An example host, written the way an Electron main process would use the
// published packages: connect (autostarting the bundled runtime), follow the
// state, speak, wait for the item to finish. run-node.mjs installs the packed
// packages next to a copy of this file and runs it.
//
// SONARA_HOME and SONARA_RUNTIME_ARGS (for example "--engine fake") come
// from the smoke runner so the run is silent and isolated; an app sets
// neither.
import { connect } from "@sonara/client";
import { runtimePath } from "@sonara/runtime-win32-x64";

const runtimeArgs = (process.env.SONARA_RUNTIME_ARGS || "").split(" ").filter(Boolean);
const sonara = await connect({ clientName: "smoke-host", runtimePath: runtimePath(), runtimeArgs });
console.log(`connected to Sonara ${sonara.info.version} (pid ${sonara.runtime.pid})`);

let label = null;
sonara.onState((s) => {
  if (s.now_playing) label = s.now_playing.label;
});
const done = new Promise((resolve, reject) => {
  const timer = setTimeout(() => reject(new Error("the item did not finish within 15 s")), 15000);
  sonara.onItem((e) => {
    if (e.phase !== "started") {
      clearTimeout(timer);
      resolve(e);
    }
  });
});
await sonara.eventsReady();
const id = await sonara.speak("Hello from an app that bundles Sonara.", { label: "smoke" });
const end = await done;
await sonara.close();
if (end.item_id !== id || end.phase !== "finished" || label !== "smoke") {
  console.error(`unexpected end: ${JSON.stringify(end)}, label ${label}`);
  process.exit(1);
}
console.log(`item ${id} finished`);
