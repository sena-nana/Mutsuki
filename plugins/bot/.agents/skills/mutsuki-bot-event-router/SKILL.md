---
name: mutsuki-bot-event-router
description: Maintain the Bot event routing plugin. Use when changing `mutsuki-plugin-bot-event-router`, `mutsuki.bot.event/ingest@1`, subscriptions, filters, dispatch tasks, or handler fan-out behavior.
---

# Mutsuki Bot Event Router

## Scope

- Own `mutsuki.bot.event/ingest@1`.
- Match standard `BotEvent` values against explicit subscriptions.
- Emit explicit targeted handler tasks for business plugins.

## Rules

- Do not put Bot broadcast or fan-out into Mutsuki Core.
- Do not inspect QQBot raw payloads; route the standard Bot event shape.
- Keep router state explicit and replaceable. Do not hide hot-reload facts in global state.
- Use handler binding IDs only as explicit targeted dispatch descriptors.

## Validation

Run `cargo fmt --check` and `cargo test` from the repository root after Rust changes.
