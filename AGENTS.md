# MutsukiBotPlugins Agents Guide

MutsukiBotPlugins is a Bot domain plugin collection. Keep it separate from Mutsuki Core and from application Hosts.

## Required Reading Order

Before changing behavior, read:

1. `README.md`
2. `docs/architecture.md`
3. `docs/protocol.md`
4. The crate and tests you are touching

For runtime boundary questions, also read NanoBot `plans/architecture.md`, `plans/engineering.md`, and `plans/contracts.md`.

## Hard Rules

- Do not add `BotHost`, `QQBotHost`, or any host runtime here. Use `MutsukiServiceHost`, `MutsukiCliHost`, or `MutsukiTauriHost` from the host layer.
- Do not put Bot protocols, QQBot APIs, command parsing, subscriptions, or business bot state into Mutsuki Core.
- QQBot is only a platform Adapter. Business plugins must use `mutsuki.bot.*` by default.
- Platform-specific behavior must stay under `mutsuki.bot.qqbot.*` or adapter internals.
- Raw QQBot payloads and media bytes must travel as resource descriptors when they are large or long lived.
- Do not pass sockets, SDK clients, database connections, Rust pointers, `Arc<T>`, or language objects across runtime boundaries.
- Secrets belong to host/config services. Do not place real tokens in manifests, examples, fixtures, or tests.
- Tests must verify behavior. Do not add tests that only hard-match logs, formatting, or placeholder strings.

## Naming

- Pure contracts: `Protocol`
- Plugin author helpers: `SDK`
- Replaceable behavior: `Plugin`
- External platform translation: `Adapter`
- Permission or side-effect exits: `Gateway`
- Persistent state abstractions: `Store` or `Repository`

## Verification

For Rust changes in this repository, run:

```powershell
cargo fmt --check
cargo test
```

If a change also modifies NanoBot runtime contracts/core/host/sdk, run the same commands in `C:\Files\workspace\NanoBot`.

## Git

Use short Chinese commit titles when committing. Check `git status --short` and targeted diffs before staging.
