---
name: mutsuki-bot-protocol
description: Maintain Mutsuki Bot pure protocol contracts. Use when changing `mutsuki-bot-protocol`, BotEvent, BotMessage, MessageSegment, BotTarget, account models, JSON schemas under `schemas/bot-*`, or protocol documentation.
---

# Mutsuki Bot Protocol

## Scope

- Define serializable Bot data contracts only.
- Keep shared models platform-neutral: `BotEvent`, `BotMessage`, `MessageSegment`, `BotTarget`,
  `BotAccountRef`, and Bot error types.
- Use `ResourceRef` for large raw payloads and media descriptors instead of inline bytes or platform objects.
- Distinguish envelope protocol IDs (`event/ingest`, `command/handle`) from runner protocols.

## Rules

- Do not add routing, command parsing, HTTP clients, gateway sessions, stores, SDK facade calls, host startup, or runner scheduling.
- Add fields only when they support a real adapter, router, SDK, or business-plugin behavior path.
- Keep protocol IDs in `namespace.domain/op@major` form, such as `mutsuki.bot.message/send@1`.
- Standard business-facing protocols stay under `mutsuki.bot.*`; platform escape hatches stay under `mutsuki.bot.qqbot.*`.
- Reserved IDs live in `mutsuki_bot_protocol::reserved` without runners. Do not reintroduce a
  permission DTO, match node, or session plugin. Schemas cover landed delivery/flow/agent/command
  DTOs only.

## Validation

Run `cargo fmt --check` and `cargo test` from the repository root after Rust changes.
