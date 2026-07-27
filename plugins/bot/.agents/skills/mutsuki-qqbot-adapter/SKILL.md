---
name: mutsuki-qqbot-adapter
description: Maintain the QQBot platform adapter plugin. Use when changing `mutsuki-plugin-bot-adapter-qqbot`, QQBot Gateway frames, event mapping, message/media OpenAPI calls, raw calls, redaction, retry, or adapter tests.
---

# Mutsuki QQBot Adapter

## Scope

- Treat QQBot as a platform Adapter.
- Own gateway frame ingestion, QQBot-to-Bot event mapping, standard Bot send/upload/recall task handling, QQBot raw calls, redaction, retry, and adapter resources.
- Convert QQBot input to `mutsuki.bot.*` standard protocols before business plugins see it.

## Rules

- Do not add business commands, business sessions, permission policy, host startup, or Core scheduling here.
- Keep gateway code unaware of business commands.
- Keep API code unaware of router subscription policy.
- Put platform-specific escape hatches under `mutsuki.bot.qqbot.*`.
- Never place real QQBot tokens or secrets in manifests, fixtures, tests, docs, or examples.

## Validation

Run `cargo fmt --check` and `cargo test` from the repository root after Rust changes. Adapter tests should cover real behavior such as event mapping, token refresh, request construction, media upload, and redaction.
