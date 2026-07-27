---
name: mutsuki-tauri-plugin-runner
description: Use when changing builtin plugin registration, plugin.toml discovery, dev reload, external runner launch, runner link, stdout/stderr collection, or plugin list/reload APIs.
---

# Plugin And Runner Skill

Use this for plugin and runner capability in `crates/mutsuki-tauri-host`.

## Boundary

- TauriHost scans desktop app plugin directories and starts desktop-scoped runners.
- It may share manifest/load-plan concepts with ServiceHost.
- Do not copy daemon behavior: no system service install, boot autostart, unattended recovery or multi-instance service supervision.

## Implementation Rules

- Builtin plugin registration must call real plugin crates behind features or report unavailable.
- Plugin reload goes through scan, validate, generation/load-plan comparison, drain and swap.
- External runners get session token, controlled cwd and allowlisted environment.
- Resolve that exact environment in TauriHost and launch through Core `SpawnedJsonlRunner`.
- stdout/stderr must be drained and forwarded to observe/event bridge.
