# MutsukiBotPlugins

## Bilibili / Workshop / Mihuashi

Mutsuki-native 迁移提供以下 builtin Rust 协议：

- `mutsuki.bot.bilibili.poll/live@1`
- `mutsuki.bot.bilibili.poll/dynamic@1`
- `mutsuki.bot.bilibili.poll/video@1`
- `mutsuki.bot.bilibili.link/resolve@1`
- `mutsuki.bot.bilibili.workshop.link/resolve@1`
- `mutsuki.bot.mihuashi.link/resolve@1`

`mutsuki-bot-link-parser` 是共享库而非 Host 插件，负责卡片 JSON 展开、URL 提取去重
与冷却辅助。Bilibili 状态固定写入 ServiceHost `data_dir/bilibili/state.sqlite3`；首次
轮询只建立 cursor，不补发历史。Cookie 只通过 `cookie_secret_key` 进入共享 credential
boundary，WBI 请求使用运行时获取的 mixin key 和注入式签名函数。

图片通过显式 `media_provider_id` 创建 `ResourceRef`，单资源上限 8 MiB。QQ adapter
从 Host registry 打开最新版 descriptor、读取并校验摘要、分块上传，随后按 segment
顺序发送 image/text。米画师 runner 使用 `AsyncRunnerAdapter` 调用
`mutsuki.browser.snapshot`，不拥有 Chromium 生命周期。

第一版不包含扫码登录、聊天管理/自助绑定、HTML 卡片截图或 Bilibili 352 浏览器回退。

MutsukiBotPlugins is the batch-first Bot domain plugin collection for Mutsuki. It is not a Host and it is not a Core extension.

The repository owns Bot protocol objects, Bot authoring helpers, Bot event routing, Bot command parsing, and platform adapter plugins such as QQBot. Runtime scheduling, runner lifecycle, host startup, Python runner execution, plugin marketplace behavior, and product-specific business bots stay outside this repository.

## MVP Crates

- `mutsuki-bot-protocol`: common `BotEvent`, `BotMessage`, `MessageSegment`, `BotTarget`, account, permission, and error contracts.
- `mutsuki-bot-sdk`: author-facing helpers that lower to Mutsuki task protocols.
- `mutsuki-plugin-bot-event-router`: standard `mutsuki.bot.event/ingest@1` router plugin.
- `mutsuki-plugin-bot-command`: generic message command parser plugin.
- `mutsuki-plugin-bot-adapter-qqbot`: QQBot platform adapter for gateway events and message/media OpenAPI tasks.
- `mutsuki-bot-service-host-integration`: configured native factories and QQ EventSource bundle.
- `mutsuki-bot-testkit`: reusable fake QQ HTTP/WebSocket boundary for downstream product E2E.
- `examples/bot-echo`: platform-neutral example business plugin that depends only on Bot protocols and SDK helpers.

## Plugin Discovery

The substantive native plugin crates generate current `PluginManifest` values from their runner
descriptors through the Mutsuki SDK `PluginBuilder`:

- `mutsuki-plugin-bot-event-router`: provides `mutsuki.bot.event/ingest@1`.
- `mutsuki-plugin-bot-command`: provides `mutsuki.bot.command/parse@1`.
- `mutsuki-plugin-bot-adapter-qqbot`: provides standard Bot message/media tasks and QQBot-specific account, gateway status, and raw call tasks.

The generated manifest is the only host-loadable source of truth. This repository does not keep
the legacy `[plugin]` / `[[provides]]` authoring format alongside it.

`mutsuki-plugin-bot-command` also builds as a Core ABI v2 `cdylib`. Its builtin configured factory
and ABI `plugin.initialize` path both parse `BotCommandConfig` and instantiate the same
`BotCommandRunner`; their deployment-neutral business surfaces are tested for equality.

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
The configured factory is text-only. Media upload is declared only when product code explicitly
adds a real media provider; no unavailable production fallback is registered.
At source startup and reconnect, the adapter validates the configured account
through `/users/@me`, obtains `/gateway/bot`, and lets Gateway reject invalid or
disallowed intent/shard configurations as permanent structured failures.

See `docs/qqbot-adapter.md` and `examples/service-host-example` for configured ServiceHost
assembly, fake-server E2E and real-account smoke boundaries. `configured_bot_plugin_catalog()`
exports owner-defined config factories without moving QQ fields into ServiceHost.

`examples/qqbot-echo` is only the deterministic product assembly. Its Echo
business runner lives in the separate `examples/bot-echo` crate and has no
QQBot, HTTP, WebSocket, or ServiceHost dependency.

## Boundary Rule

Business bot plugins should depend on `mutsuki.bot.*` protocols. They should not call QQBot APIs directly. QQBot-specific escape hatches must use `mutsuki.bot.qqbot.*` protocols and remain adapter-specific.
