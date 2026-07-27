---
name: mutsuki-tauri-runtime
description: Use when changing MutsukiTauriHost embedded Core bootstrap, host lifecycle, desktop HostServices, shutdown or drain behavior in a Tauri desktop application.
---

# Runtime Skill

Use this for `crates/mutsuki-tauri-host`.

## Boundary

- Start Mutsuki Core through `mutsuki-runtime-host` / `mutsuki-runtime-core` public APIs.
- Bind app profile, data/cache/log/resource paths and desktop HostServices.
- Own application/window lifecycle, shutdown and drain coordination.
- Do not implement Core scheduling, TaskPool, RunnerRegistry, AgentLoop, Bot routing or model providers.

## Implementation Rules

- Core bootstrap failures must fail loud and return structured errors.
- `MutsukiTauriHost` may aggregate plugins and runners, but plugin domain behavior belongs to plugin crates.
- Shutdown must stop event forwarding, runner supervision and resource cleanup before dropping HostRuntime.
- Desktop services expose OS/app primitives; protocol wrappers belong to std/desktop plugins.
