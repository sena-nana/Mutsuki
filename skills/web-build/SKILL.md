---
name: web-build
description: Change @mutsuki/web-build, web-sdk, or web-shell packaging, externals, manifest, or HMR tooling.
---

# Web Build

Use for `packages/web-{build,sdk,shell}`.

## Boundary

- Build tooling runs in development/release pipelines only.
- Production WebHost never installs npm, starts Vite, compiles `.vue`, or downloads Node.

## Rules

- Externalize `vue`, `vue-router`, `pinia`, `@mutsuki/web-sdk`, `@mutsuki/ui`.
- Emit versioned manifest with content hashes and protocol version.
- HMR is opt-in via `MUTSUKI_WEB_HMR=1` and must not be required at runtime.
- Extensions register through stable registries and return disposables.
