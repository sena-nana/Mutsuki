# MutsukiBotPlugins

MutsukiBotPlugins is the batch-first Bot domain plugin collection for Mutsuki. It is not a Host and it is not a Core extension.

The repository owns Bot protocol objects, Bot authoring helpers, Bot event routing, Bot command parsing, and platform adapter plugins such as QQBot. Runtime scheduling, runner lifecycle, host startup, Python runner execution, plugin marketplace behavior, and product-specific business bots stay outside this repository.

## MVP Crates

- `mutsuki-bot-protocol`: common `BotEvent`, `BotMessage`, `MessageSegment`, `BotTarget`, account, permission, and error contracts.
- `mutsuki-bot-sdk`: author-facing helpers that lower to Mutsuki task protocols.
- `mutsuki-plugin-bot-event-router`: standard `mutsuki.bot.event/ingest@1` router plugin.
- `mutsuki-plugin-bot-command`: generic message command parser plugin.
- `mutsuki-plugin-bot-adapter-qqbot`: QQBot platform adapter for gateway events and message/media OpenAPI tasks.
- `examples/bot-echo`: platform-neutral example business plugin that depends only on Bot protocols and SDK helpers.

## Plugin Discovery

The substantive native plugin crates generate current `PluginManifest` values from their runner
descriptors through the Mutsuki SDK `PluginBuilder`:

- `mutsuki-plugin-bot-event-router`: provides `mutsuki.bot.event/ingest@1`.
- `mutsuki-plugin-bot-command`: provides `mutsuki.bot.command/parse@1`.
- `mutsuki-plugin-bot-adapter-qqbot`: provides standard Bot message/media tasks and QQBot-specific account, gateway status, and raw call tasks.

The generated manifest is the only host-loadable source of truth. This repository does not keep
the legacy `[plugin]` / `[[provides]]` authoring format alongside it.

`mutsuki-bot-protocol` and `mutsuki-bot-sdk` are library crates and are not host-loadable plugins.

## Runtime Relationship

```text
MutsukiServiceHost / MutsukiCliHost / MutsukiTauriHost
  -> MutsukiCore
  -> MutsukiBotPlugins
```

Do not introduce `BotHost`. A standalone Bot service should run through `MutsukiServiceHost`.

All native runners implement the current MutsukiCore `Runner::run_batch` contract. A single task is represented as a one-entry `WorkBatch`; there is no separate scalar `step` execution path. Row payload tasks are mapped back to their matching `BatchEntry`, and each entry produces its own `EntryCompletion` inside a `CompletionBatch`.

## QQBot Production Bundle

`mutsuki-bot-service-host-integration::QqBotPluginBundle` assembles the QQBot manifest,
batch runners and the ServiceHost-managed Gateway EventSource. The adapter crate itself has no
ServiceHost dependency. The production HTTP transport uses
`reqwest` with the Rustls TLS backend; Gateway WebSocket uses
`tokio-tungstenite` with Rustls webpki roots. Product code installs the bundle
into `ServiceRuntimeBuilder`; it does not create a Bot-specific Host.
Construction requires an explicit media provider factory; no unavailable production fallback is
registered.
At source startup and reconnect, the adapter validates the configured account
through `/users/@me`, obtains `/gateway/bot`, and lets Gateway reject invalid or
disallowed intent/shard configurations as permanent structured failures.

See `examples/service-host-example` for the deterministic SDK smoke and the
ServiceHost integration test. Product-specific config loading remains in the
product Host; this repository only accepts an already constructed
`QqBotConfig` at the bundle boundary.

`examples/qqbot-echo` is only the deterministic product assembly. Its Echo
business runner lives in the separate `examples/bot-echo` crate and has no
QQBot, HTTP, WebSocket, or ServiceHost dependency.

## Boundary Rule

Business bot plugins should depend on `mutsuki.bot.*` protocols. They should not call QQBot APIs directly. QQBot-specific escape hatches must use `mutsuki.bot.qqbot.*` protocols and remain adapter-specific.
