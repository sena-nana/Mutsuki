#!/usr/bin/env node
/** Builds `browser/shared/*`: the runtime modules the shell maps for every extension. */
import { copyFile, mkdir, rm, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "vite";

import { SHARED_BUDGETS, SHARED_MODULES, SHARED_STYLESHEET } from "../src/modules.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, "..");
const outDir = path.join(root, "browser/shared");
const entries = path.join(root, ".entries");
const require = createRequire(import.meta.url);
const specifiers = SHARED_MODULES.map((module) => module.specifier);

await rm(outDir, { recursive: true, force: true });
await rm(entries, { recursive: true, force: true });
await mkdir(outDir, { recursive: true });
await mkdir(entries, { recursive: true });

for (const module of SHARED_MODULES) {
  const entry = path.join(entries, module.file);
  await writeFile(entry, `export * from ${JSON.stringify(module.specifier)};\n`);
  const isVue = module.specifier === "vue";
  // @mutsuki/ui ships Vue SFCs; its own Vite build (dist) is the compiled form.
  const fromDist = module.specifier === "@mutsuki/ui";
  await build({
    configFile: false,
    root,
    logLevel: "warn",
    resolve: {
      // Workspace packages build from TypeScript sources.
      conditions: fromDist
        ? ["module", "browser", "production"]
        : ["source", "module", "browser", "production"],
      alias: isVue
        ? [{ find: /^vue$/, replacement: require.resolve("vue/dist/vue.runtime.esm-bundler.js") }]
        : [],
    },
    define: {
      "process.env.NODE_ENV": JSON.stringify("production"),
      __VUE_OPTIONS_API__: "true",
      __VUE_PROD_DEVTOOLS__: "false",
      __VUE_PROD_HYDRATION_MISMATCH_DETAILS__: "false",
    },
    build: {
      outDir,
      emptyOutDir: false,
      target: "es2022",
      minify: true,
      copyPublicDir: false,
      lib: { entry, formats: ["es"], fileName: () => module.file },
      rolldownOptions: {
        // Every other shared specifier stays a bare import resolved by the import map.
        external: (id) => specifiers.includes(id) && id !== module.specifier,
        // Vite keeps whitespace in ES library builds; the runtime ships minified.
        output: { minify: true },
      },
    },
  });
}
await rm(entries, { recursive: true, force: true });

// The design-system stylesheet is the inlined @mutsuki/ui bundle.
await copyFile(
  require.resolve("@mutsuki/ui/ui.css"),
  path.join(outDir, SHARED_STYLESHEET),
);

const { checkBundle, formatViolations } = await import("@mutsuki/web-build");
const violations = await checkBundle(outDir, {
  allowedImports: specifiers,
  allowVueRuntime: true,
  allowHexColors: true,
  budgets: SHARED_BUDGETS,
});
if (violations.length) {
  console.error(formatViolations(violations));
  process.exit(1);
}
console.log(`wrote ${SHARED_MODULES.length} shared modules and ${SHARED_STYLESHEET}`);
