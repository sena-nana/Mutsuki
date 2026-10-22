---
name: web-build
description: Change @mutsuki/web-build, web-sdk, or web-shell packaging, externals, manifest, bundle gates, the root pnpm workspace, or HMR tooling.
---

# Web Build

Use for `packages/web-{build,sdk,shell}` and the root pnpm workspace files.

## Boundary

- Build tooling runs in development/release pipelines only.
- Production WebHost never installs npm, starts Vite, compiles `.vue`, or downloads Node.

## Workspace

- One pnpm workspace at the repository root: `pnpm-workspace.yaml`, `package.json`,
  `pnpm-lock.yaml`, `tsconfig.web.base.json`. Members: `hosts/web/packages/*` and
  `{hosts/web,plugins/*,kits/*}/crates/*/web`. Run `pnpm -r typecheck|test|build` from the root.
- Shared versions live in the workspace `catalog:`; packages use `catalog:` for them and
  `workspace:*` for each other. Node is pinned to 26 (`@lilia/theme` engines) in CI.
- Package `exports` put a `source` condition first; `tsconfig.web.base.json` and each
  `vitest.config.ts` resolve it, so typecheck and tests run from TypeScript sources without a
  prior build.
- Tests use vitest (`vitest run`), not `node --test`.
- `check_workspace.py::check_web_packages` enforces all of the above and freezes the hand-written
  `assets/*.js` allowlist (`HANDWRITTEN_WEB_ASSETS`); a Vue migration removes its entry.

## Shared Runtime

- `packages/web-runtime` builds one module per import-map specifier into the committed
  `browser/shared/`: `vue` (runtime-only, production), `vue-router`, `pinia` (both with `vue`
  external), `@mutsuki/web-sdk`, `@mutsuki/ui`, `@mutsuki/web-shell`, plus `ui.css`. Each module
  imports the others only by bare specifier; `checkBundle` runs on the output.
- `crates/mutsuki-web-shared-runtime` embeds exactly that directory (a test fails on drift),
  serves files as `shared/<name>.<sha256:16>.<ext>`, and owns `import_map_json()` (relative URLs),
  `import_map_script()` and `csp_script_hash()`. Consoles inline the import map and put its hash
  in `script-src`; no `'unsafe-inline'` scripts.
- `ExtensionManifest.styles` lists extension stylesheets (hashed assets); the shell links them
  before importing the entry. The design-system CSS is never copied into an extension.

## Rules

- `buildExtension` runs Vite library mode with `@vitejs/plugin-vue`: one `index.js`, at most one
  `index.css`, `vue`/`vue-router`/`pinia`/`@mutsuki/web-sdk`/`@mutsuki/ui` external.
- `checkBundle` (also `mutsuki-web-build check`) rejects bare imports outside the shared runtime,
  a bundled Vue runtime, JS > 48 KiB / CSS > 8 KiB per file (a package raises its own cap with
  `"mutsuki": { "budgets": { "cssBytes": N, "reason": "…" } }` in package.json; no reason, no
  build), the shared runtime's own caps in `web-runtime/src/modules.mjs` (128 KiB JS / 96 KiB CSS),
  hex colors in CSS declarations (use Lilia / `--mk-*` tokens) and `node:` imports.
- Emit a versioned manifest with content hashes, `styles`, and protocol version.
- HMR is opt-in via `MUTSUKI_WEB_HMR=1` and must not be required at runtime.
- Extensions register through stable registries and return disposables.
