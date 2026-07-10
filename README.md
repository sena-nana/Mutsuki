# MutsukiBotPlugins

MutsukiBotPlugins is the batch-first Bot domain plugin collection for Mutsuki. It is not a Host and it is not a Core extension.

The repository owns Bot protocol objects, Bot authoring helpers, Bot event routing, Bot command parsing, and platform adapter plugins such as QQBot. Runtime scheduling, runner lifecycle, host startup, Python runner execution, plugin marketplace behavior, and product-specific business bots stay outside this repository.

## MVP Crates

- `mutsuki-bot-protocol`: common `BotEvent`, `BotMessage`, `MessageSegment`, `BotTarget`, account, permission, and error contracts.
- `mutsuki-bot-sdk`: author-facing helpers that lower to Mutsuki task protocols.
- `mutsuki-plugin-bot-event-router`: standard `mutsuki.bot.event/ingest@1` router plugin.
- `mutsuki-plugin-bot-command`: generic message command parser plugin.
- `mutsuki-plugin-bot-adapter-qqbot`: QQBot platform adapter for gateway events and message/media OpenAPI tasks.

## Plugin Discovery

The substantive native plugin crates carry `plugin.toml` manifests:

- `mutsuki-plugin-bot-event-router`: provides `mutsuki.bot.event/ingest@1`.
- `mutsuki-plugin-bot-command`: provides `mutsuki.bot.command/parse@1`.
- `mutsuki-plugin-bot-adapter-qqbot`: provides standard Bot message/media tasks and QQBot-specific account, gateway status, and raw call tasks.

`mutsuki-bot-protocol` and `mutsuki-bot-sdk` are library crates and are not host-loadable plugins.

## Runtime Relationship

```text
MutsukiServiceHost / MutsukiCliHost / MutsukiTauriHost
  -> MutsukiCore
  -> MutsukiBotPlugins
```

Do not introduce `BotHost`. A standalone Bot service should run through `MutsukiServiceHost`.

All native runners implement the current MutsukiCore `Runner::run_batch` contract. A single task is represented as a one-entry `WorkBatch`; there is no separate scalar `step` execution path. Row payload tasks are mapped back to their matching `BatchEntry`, and each entry produces its own `EntryCompletion` inside a `CompletionBatch`.

## Boundary Rule

Business bot plugins should depend on `mutsuki.bot.*` protocols. They should not call QQBot APIs directly. QQBot-specific escape hatches must use `mutsuki.bot.qqbot.*` protocols and remain adapter-specific.
