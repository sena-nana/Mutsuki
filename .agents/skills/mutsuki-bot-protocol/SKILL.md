---
name: mutsuki-bot-protocol
description: Maintain Mutsuki Bot pure protocol contracts. Use when changing `mutsuki-bot-protocol`, BotEvent, BotMessage, MessageSegment, BotTarget, account or permission models, JSON schemas under `schemas/bot-*`, or protocol documentation.
---

# Mutsuki Bot Protocol

## Scope

- Define serializable Bot data contracts only.
- Keep shared models platform-neutral: `BotEvent`, `BotMessage`, `MessageSegment`, `BotTarget`, `BotAccount`, `BotPermission`, and Bot error types.
- Use `ResourceRef` for large raw payloads and media descriptors instead of inline bytes or platform objects.

## Rules

- Do not add routing, command parsing, HTTP clients, gateway sessions, stores, SDK facade calls, host startup, or runner scheduling.
- Add fields only when they support a real adapter, router, SDK, or business-plugin behavior path.
- Keep protocol IDs in `namespace.domain/op@major` form, such as `mutsuki.bot.message/send@1`.
- Standard business-facing protocols stay under `mutsuki.bot.*`; platform escape hatches stay under `mutsuki.bot.qqbot.*`.

## Validation

Run `cargo fmt --check` and `cargo test` from the repository root after Rust changes.
