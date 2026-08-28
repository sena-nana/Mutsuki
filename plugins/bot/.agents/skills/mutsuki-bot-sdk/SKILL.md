---
name: mutsuki-bot-sdk
description: Maintain Bot plugin author helpers. Use when changing `mutsuki-bot-sdk`, BotContext, MessageBuilder, CommandContext, prelude exports, or SDK examples for business bot authors.
---

# Mutsuki Bot SDK

## Scope

- Provide author-facing helpers over Bot protocols: `BotContext`, `MessageBuilder`,
  `CommandContext`, and prelude exports.
- Lower helpers to protocol IDs, JSON payloads, `Task`, `ResourceRef`, descriptor builders, or runner results.
- Keep APIs honest: a helper that looks executable must build or submit a real protocol payload.

## Rules

- Do not implement scheduling, registry mutation, host startup, network clients, QQBot API behavior, or gateway sessions.
- Do not hide platform-specific behavior behind generic helpers unless it still lowers to standard `mutsuki.bot.*` contracts.
- Prefer small builders and typed wrappers over new runtime abstractions.
- Do not reintroduce `EventHandlerSpec`. Envelope IDs (`event/ingest`, `command/handle`) are not
  runner protocols.

## Validation

Run `cargo fmt --check` and `cargo test` from the repository root after Rust changes.
