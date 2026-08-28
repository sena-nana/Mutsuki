---
name: mutsuki-bot-command
description: Maintain the platform-neutral Bot command plugin. Use when changing `mutsuki-plugin-bot-command`, command parsing, command matching, BotCommandEvent, command dispatch, or command fixtures.
---

# Mutsuki Bot Command

## Scope

- Own the graph-configured Command Match node bound to `mutsuki.bot.command/parse@1`.
- Parse commands from standard message events using node config (prefix, path, aliases, typed args).
- Emit `matched` as a `mutsuki.bot.command/handle@1` envelope (`BotCommandEvent`) or `unmatched`
  with the unchanged input. `command/handle` is an envelope ID, not a runner protocol.

## Rules

- Do not depend on QQBot raw payload shapes.
- Do not mix permission or session policy into command parsing. Those plugins are deferred.
- Do not add low-value tests that hard-match formatting; test command behavior and emitted task payloads.

## Validation

Run `cargo fmt --check` and `cargo test` from the repository root after Rust changes.
