// Bundle src/main.jsx into public/app.js for the browser.
import { build } from "esbuild";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const require = createRequire(join(here, "package.json"));

// @sonara/player is linked from clients/player, which has its own React for
// its tests. Resolve React from this app only, so the page runs one copy.
const oneReact = {
  name: "one-react",
  setup(b) {
    b.onResolve({ filter: /^react(-dom)?(\/.*)?$/ }, (args) => ({ path: require.resolve(args.path) }));
  },
};

await build({
  entryPoints: [join(here, "src", "main.jsx")],
  outfile: join(here, "public", "app.js"),
  bundle: true,
  format: "esm",
  target: "es2021",
  jsx: "automatic",
  minify: true,
  sourcemap: false,
  define: { "process.env.NODE_ENV": '"production"' },
  plugins: [oneReact],
  logLevel: "info",
});
