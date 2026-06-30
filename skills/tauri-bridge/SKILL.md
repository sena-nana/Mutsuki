---
name: mutsuki-tauri-bridge
description: Use when changing Tauri commands, event emission, task invoke, task stream, cancel, status, or command state for MutsukiTauriHost.
---

# Tauri Bridge Skill

Use this for `crates/tauri-plugin-mutsuki` and `crates/mutsuki-tauri-bridge`.

## Boundary

- Frontend calls protocols, not plugin internals.
- Commands convert frontend requests into Core tasks, resource operations, approvals or plugin control operations.
- Events carry task/log/trace/plugin/runner/resource/approval messages to WebView.
- Do not put business UI, React/Vue components or product-specific route logic here.

## Implementation Rules

- Long-running or streaming output must use events, not a single large invoke return.
- Include frontend context fields when available: window label, session id, trace id, correlation id and user action id.
- Command payloads must remain JSON-friendly and bounded; large data moves through ResourceRef.
- All command errors must serialize as `FrontendError`.
