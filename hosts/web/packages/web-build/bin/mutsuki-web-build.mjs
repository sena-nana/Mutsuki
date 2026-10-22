#!/usr/bin/env node
import { buildExtension, checkBundle, createViteHmrConfig, formatViolations, packageBudgets } from "../dist/index.js";
import path from "node:path";
import { readFile, writeFile } from "node:fs/promises";

/** Budget overrides from the package being built (`mutsuki.budgets` in its package.json). */
async function ownBudgets() {
  try {
    return packageBudgets(JSON.parse(await readFile(path.resolve("package.json"), "utf8")));
  } catch (error) {
    if (error?.code === "ENOENT") return {};
    throw error;
  }
}

const [command, ...args] = process.argv.slice(2);
const flags = new Set(args.filter((arg) => arg.startsWith("--")));
const rest = args.filter((arg) => !arg.startsWith("--"));

function list(value) {
  return (value ?? "")
    .split(",")
    .map((item) => item.trim())
    .filter(Boolean);
}

async function main() {
  if (command === "build") {
    const entry = rest[0] ?? "src/index.ts";
    const outDir = rest[1] ?? "dist/web";
    const result = await buildExtension({
      entry: path.resolve(entry),
      outDir: path.resolve(outDir),
      extensionId: process.env.MUTSUKI_EXTENSION_ID ?? "mutsuki.example.web",
      version: process.env.MUTSUKI_EXTENSION_VERSION ?? "0.1.0",
      capabilities: list(process.env.MUTSUKI_EXTENSION_CAPABILITIES),
      writeManifest: !flags.has("--no-manifest"),
      check: { budgets: await ownBudgets() },
    });
    console.log(JSON.stringify(result, null, 2));
    return;
  }

  if (command === "check") {
    const outDir = rest[0] ?? "dist/web";
    const violations = await checkBundle(path.resolve(outDir), { budgets: await ownBudgets() });
    if (violations.length) console.error(formatViolations(violations));
    process.exitCode = violations.length ? 1 : 0;
    return;
  }

  if (command === "dev") {
    if (process.env.MUTSUKI_WEB_HMR !== "1") {
      console.error("dev config is opt-in: set MUTSUKI_WEB_HMR=1 (development only)");
      process.exitCode = 1;
      return;
    }
    const entry = rest[0] ?? "src/index.ts";
    const configPath = path.resolve(rest[1] ?? "vite.config.mutsuki.mjs");
    await writeFile(configPath, createViteHmrConfig({ entry: path.resolve(entry) }));
    console.log(`Wrote development Vite HMR config to ${configPath}`);
    console.log("Run: pnpm exec vite --config", configPath);
    console.log("Production WebHost never executes this path.");
    return;
  }

  console.log(`Usage:
  mutsuki-web-build build [entry] [outDir] [--no-manifest]
  mutsuki-web-build check [outDir]
  MUTSUKI_WEB_HMR=1 mutsuki-web-build dev [entry] [viteConfigOut]

build/check reject: bare imports outside the shared runtime, bundled Vue runtime,
JS or CSS over budget, raw hex colors in CSS, and node: modules. A package may raise its
budget in package.json: "mutsuki": { "budgets": { "cssBytes": N, "reason": "why" } }.

Optional env:
  MUTSUKI_EXTENSION_ID
  MUTSUKI_EXTENSION_VERSION
  MUTSUKI_EXTENSION_CAPABILITIES
`);
}

main().catch((error) => {
  console.error(error instanceof Error ? error.message : error);
  process.exit(1);
});
