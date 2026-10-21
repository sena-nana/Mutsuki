---
name: mutsuki-tauri-plugin-runner
description: Use when changing builtin plugin registration, plugin.toml discovery, dev reload, external runner launch, runner link, stdout/stderr collection, or plugin list/reload APIs.
---

# Plugin And Runner Skill

Use this for plugin and runner capability in `crates/mutsuki-tauri-host`.

## Boundary

- TauriHost scans desktop app plugin directories and starts desktop-scoped runners.
- `plugin.toml` (`[manifest]` + optional `[runtime]`) is the same format ServiceHost reads; parse and validate it only through `mutsuki-runtime-plugin-loader`. There is no separate `runner.toml`.
- Do not copy daemon behavior: no system service install, boot autostart, unattended recovery or multi-instance service supervision.

## Implementation Rules

- Builtin plugin registration must call real plugin crates behind features or report unavailable.
- Plugin reload goes through scan, validate, generation/load-plan comparison, drain and swap.
- External runners get session token, controlled cwd and allowlisted environment: `PATH` and the Windows system variables plus `[runtime].env`. Plugins cannot request further host variables.
- Only `runner_link = "binary-stdio"` is supported on desktop; one process is spawned per runner descriptor.
- Resolve that exact environment in TauriHost and launch through Core `SpawnedBinaryRunner`.
- stdout/stderr must be drained and forwarded to observe/event bridge.
