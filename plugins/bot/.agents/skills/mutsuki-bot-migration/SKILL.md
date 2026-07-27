---
name: mutsuki-bot-migration
description: Migrate Bot or QQBot code between NanoBot and MutsukiBotPlugins. Use when moving old plugin code, deleting old core/postponed plugin content, mapping `mutsuki.im.qqbot.*` surfaces, or checking migration residue.
---

# Mutsuki Bot Migration

## Scope

- Split old Bot/QQBot code into protocol, SDK, event-router, command, adapter, and future service-plugin responsibilities.
- Convert old `mutsuki.im.qqbot.*` surfaces to standard `mutsuki.bot.*` or adapter-specific `mutsuki.bot.qqbot.*`.
- Delete old NanoBot plugin content only after equivalent behavior exists in this repository.

## Rules

- Do not migrate by copying a monolithic business bot into the adapter.
- Do not introduce `BotHost`; standalone Bot services run through `MutsukiServiceHost`.
- Confirm NanoBot Core remains domain-neutral after deletion.
- Preserve user changes in dirty worktrees; stage or remove only the migration scope.

## Verification

- In MutsukiBotPlugins: run `cargo fmt --check` and `cargo test`.
- In NanoBot after deletions or runtime boundary changes: run `cargo fmt --check`, `cargo test`, and a residue search for `qqbot`, `mutsuki.im.qqbot`, and old plugin paths.
