#!/usr/bin/env node
/** Bundle @lilia/theme + console.css → dist/mutsuki-ui.css (single-file, include_str!-safe). */
import { createRequire } from "node:module";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const here = path.dirname(fileURLToPath(import.meta.url));
const stylesDir = path.join(here, "../src/styles");
const outDir = path.join(here, "../dist");
mkdirSync(outDir, { recursive: true });

const importPattern = /@import\s+(?:url\()?\s*["']([^"']+)["']\s*\)?\s*;[^\n]*/g;
const inlined = new Set();

function resolveSpecifier(spec, fromFile) {
  if (spec.startsWith(".") || spec.startsWith("/")) {
    return path.resolve(path.dirname(fromFile), spec);
  }
  return require.resolve(spec);
}

function inline(file) {
  const absolute = path.resolve(file);
  if (inlined.has(absolute)) return "";
  inlined.add(absolute);
  return readFileSync(absolute, "utf8").replace(importPattern, (_match, spec) =>
    inline(resolveSpecifier(spec, absolute)),
  );
}

const entries = [
  require.resolve("@lilia/theme/base.css"),
  require.resolve("@lilia/theme/styles/workspace.css"),
  require.resolve("@lilia/theme/styles/sidebar.css"),
  require.resolve("@lilia/theme/styles/page.css"),
  path.join(stylesDir, "console.css"),
  path.join(stylesDir, "node-editor.css"),
];

const bundled = entries
  .map((file) => {
    const body = inline(file).trim();
    return body ? `/* —— ${path.basename(file)} —— */\n${body}` : "";
  })
  .filter(Boolean)
  .join("\n\n");

writeFileSync(path.join(outDir, "mutsuki-ui.css"), `${bundled}\n`);
console.log(`wrote dist/mutsuki-ui.css (${bundled.length} bytes pre-newline)`);
