---
name: mutsuki-bot-command
description: Maintain the platform-neutral Bot command plugin. Use when changing `mutsuki-plugin-bot-command`, command parsing, command matching, BotCommandEvent, command dispatch, or command fixtures.
---

# Mutsuki Bot Command

## Scope

- Parse commands from standard message events.
- Emit command events or handler tasks for business plugins.
- Keep command behavior deterministic and independent from any platform adapter.

## Rules

- Do not depend on QQBot raw payload shapes.
- Do not mix permission or session policy into command parsing unless the task explicitly targets those future plugins.
- Do not add low-value tests that hard-match formatting; test command behavior and emitted task payloads.

## Validation

Run `cargo fmt --check` and `cargo test` from the repository root after Rust changes.
