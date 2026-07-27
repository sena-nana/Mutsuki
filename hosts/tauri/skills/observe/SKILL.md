---
name: mutsuki-tauri-observe
description: Use when changing log, trace, health, panic/crash, runner stdout/stderr, event sink, or frontend observability streaming.
---

# Observe Skill

Use this for host observability and frontend event forwarding.

## Boundary

- TauriHost aggregates runtime-level logs, traces, health and runner process output.
- Plugin domain health can be forwarded, but Host does not invent plugin-specific semantics.

## Implementation Rules

- Logs and trace events must be structured.
- Secret/token values must be redacted before ordinary logging or frontend emission.
- Panic/crash capture should preserve existing hooks and write under the configured log directory.
- Health should expose app, host, core, plugins, runners and recent error state.
