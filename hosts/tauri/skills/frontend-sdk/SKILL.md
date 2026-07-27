---
name: mutsuki-tauri-frontend-sdk
description: Use when changing @mutsuki/tauri-client TypeScript APIs for calls, streams, resources, approvals, plugins, runners, logs or trace.
---

# Frontend SDK Skill

Use this for `packages/tauri-client`.

## Boundary

- Provide a lightweight Tauri client SDK only.
- Do not add React/Vue/Svelte components, page routing, LiliaUI widgets, editors, Bot panels or marketplace UI.

## Implementation Rules

- Wrap Tauri `invoke` and event `listen` behind typed APIs.
- `callStream` returns a task handle with async event iteration and cancellation.
- Resource APIs expose handles, text/bytes helpers and object URL lifecycle helpers.
- Approval APIs register handlers and submit decisions to the backend command.
- Keep runtime dependencies minimal and browser/Tauri compatible.
