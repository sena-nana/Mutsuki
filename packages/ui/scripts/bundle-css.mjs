#!/usr/bin/env node
/**
 * 把 @mutsuki/ui 的 CSS 拼成 dist/mutsuki-ui.css，供静态宿主与 BotPlugins 物化。
 *
 * 视觉基座从 pinned 依赖 @lilia/theme 解析（唯一事实源），不再复制 Lilia CSS。
 * 递归内联 @import，使产物是可 include_str! 的单文件；@font-face 的 url(/fonts/*.woff2)
 * 保持不变（host-served 路径契约）。console.css 追加在最后（Mutsuki 产品 chrome）。
 */
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

function resolveSpecifier(spec, fromFile) {
  if (spec.startsWith(".") || spec.startsWith("/")) {
    return path.resolve(path.dirname(fromFile), spec);
  }
  return require.resolve(spec);
}

const inlined = new Set();

function inline(file) {
  const absolute = path.resolve(file);
  if (inlined.has(absolute)) return "";
  inlined.add(absolute);
  const source = readFileSync(absolute, "utf8");
  return source.replace(importPattern, (_match, spec) => {
    const target = resolveSpecifier(spec, absolute);
    return inline(target);
  });
}

// 顺序权威：base（字体+tokens+state-layer+reset）→ 布局 → console。
const entries = [
  require.resolve("@lilia/theme/base.css"),
  require.resolve("@lilia/theme/styles/workspace.css"),
  require.resolve("@lilia/theme/styles/sidebar.css"),
  require.resolve("@lilia/theme/styles/page.css"),
  path.join(stylesDir, "console.css"),
];

const bundled = entries
  .map((file) => {
    const label = path.basename(file);
    const body = inline(file);
    return body.trim() ? `/* —— ${label} —— */\n${body.trim()}\n` : "";
  })
  .filter(Boolean)
  .join("\n");

writeFileSync(path.join(outDir, "mutsuki-ui.css"), `${bundled}\n`);
console.log(`wrote dist/mutsuki-ui.css (${bundled.length} bytes pre-newline)`);
