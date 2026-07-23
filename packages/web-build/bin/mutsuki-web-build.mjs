#!/usr/bin/env node
import { buildExtension, checkExternals, createViteHmrConfig } from "../dist/index.js";
import path from "node:path";
import { writeFile } from "node:fs/promises";

const [command, ...rest] = process.argv.slice(2);

async function main() {
  if (command === "build") {
    const entry = rest[0] ?? "src/index.ts";
    const outDir = rest[1] ?? "dist/web";
    const extensionId = process.env.MUTSUKI_EXTENSION_ID ?? "mutsuki.example.web";
    const result = await buildExtension({
      entry: path.resolve(entry),
      outDir: path.resolve(outDir),
      extensionId,
      version: process.env.MUTSUKI_EXTENSION_VERSION ?? "0.1.0",
      capabilities: (process.env.MUTSUKI_EXTENSION_CAPABILITIES ?? "")
        .split(",")
        .map((item) => item.trim())
        .filter(Boolean),
      hmr: process.env.MUTSUKI_WEB_HMR === "1",
    });
    const warnings = await checkExternals(path.resolve(outDir));
    if (warnings.length) {
      for (const warning of warnings) {
        console.warn(warning);
      }
      process.exitCode = 1;
      return;
    }
    console.log(JSON.stringify(result, null, 2));
    return;
  }

  if (command === "check") {
    const outDir = rest[0] ?? "dist/web";
    const warnings = await checkExternals(path.resolve(outDir));
    for (const warning of warnings) {
      console.warn(warning);
    }
    process.exitCode = warnings.length ? 1 : 0;
    return;
  }

  if (command === "dev") {
    const entry = rest[0] ?? "src/index.ts";
    const configPath = path.resolve(rest[1] ?? "vite.config.mutsuki.mjs");
    const config = createViteHmrConfig({ entry: path.resolve(entry) });
    await writeFile(configPath, config);
    console.log(`Wrote development Vite HMR config to ${configPath}`);
    console.log("Run: pnpm exec vite --config", configPath);
    console.log("Production WebHost never executes this path.");
    return;
  }

  console.log(`Usage:
  mutsuki-web-build build [entry] [outDir]
  mutsuki-web-build check [outDir]
  mutsuki-web-build dev [entry] [viteConfigOut]

Optional env:
  MUTSUKI_EXTENSION_ID
  MUTSUKI_EXTENSION_VERSION
  MUTSUKI_EXTENSION_CAPABILITIES
  MUTSUKI_WEB_HMR=1   # development only; production WebHost never runs this
`);
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
