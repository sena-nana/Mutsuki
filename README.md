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
轮询只建立 cursor，不补发历史。产品必须显式选择 `backend.type = "web_cookie"` 或
`backend.type = "open_platform"`。Web backend 的 Cookie 只通过
`backend.cookie_secret_key` 进入共享 credential boundary，WBI 请求使用运行时获取的
mixin key 和注入式签名函数。

官方开放平台 backend 使用 OAuth2 access/refresh token 与 v2 HMAC-SHA256 请求签名，
只复用被授权账号的 `poll/live` 和 `poll/video` 协议。官方开放平台没有等价动态查询、
Cookie 扫码管理、WBI/352 或通用链接解析能力；配置这些能力会在启动前失败，不会回退到
Web backend。OAuth credential bundle 和 app secret 使用两个 Host secret key，token
刷新后原子轮换整个 bundle。配置、scope、错误码和 fake transport 验收见
`docs/bilibili-open-platform.md`。

图片通过显式 `media_provider_id` 创建 `ResourceRef`，单资源上限 8 MiB。QQ adapter
从 Host registry 打开最新版 descriptor、读取并校验摘要、分块上传，随后按 segment
顺序发送 image/text。米画师 runner 使用 `TaskAwaitRunnerAdapter` 调用
`mutsuki.browser.snapshot`，不拥有 Chromium 生命周期。

账号与订阅管理通过通用 `mutsuki.bot.command/handle@1` 路径进入同一个 batch-first
Bilibili runner。启用 management 后提供：Host 管理员扫码登录与凭据轮换、签名验证码
自助绑定、订阅列表/暂停/恢复/删除，以及不推进 cursor 的最新动态预览。二维码在 runner
内生成 PNG `ResourceRef`，Cookie 不进入消息、Task payload、manifest、日志或 trace。

管理操作只通过 ServiceHost 的原子 secret/config persistence handle 落盘：扫码成功轮换
`backend.cookie_secret_key` 指向的本地 secret，订阅变更替换产品配置中 Bilibili owner 的 opaque
config。插件 SQLite 只保存 cursor、cooldown 和未完成的 QR/绑定 challenge，不是订阅关系
权威。management 默认关闭；启用时必须从真实产品配置文件启动并配置 Host
`security.secret_file`。

Bilibili 动态 API 的 352 风控回退默认关闭。产品必须同时显式配置
`risk_control.backend = "chromium"` 和 `mutsuki.std.io.browser.chromium`；Bilibili Runner
仅通过通用 `mutsuki.browser.snapshot` 子任务获取 DOM，不拥有 Chromium 生命周期。
Chromium factory 在启动阶段校验 executable，provider 与 Bilibili owner 配置分别限制
domain、timeout、DOM 和读取响应大小。未配置 backend、浏览器任务失败、重定向越域或
响应超限都会结构化失败；成功回退写入 `mutsuki.bot.bilibili.risk_control/status@1`
degradation event。配置与验证层级见 `docs/bilibili-risk-control.md`；账号与订阅管理见
`docs/bilibili-management.md`。

MutsukiBotPlugins is the batch-first Bot domain plugin collection for Mutsuki. It is not a Host and it is not a Core extension.

The repository owns Bot protocol objects, Bot authoring helpers, Bot event routing, Bot command parsing, and platform adapter plugins such as QQBot. Runtime scheduling, runner lifecycle, host startup, Python runner execution, plugin marketplace behavior, and product-specific business bots stay outside this repository.

## MVP Crates

- `mutsuki-bot-config` / `mutsuki-bot-config-derive`: Schema-first ConfigDescriptor + `#[derive(MutsukiConfig)]`
- `mutsuki-plugin-bot-config-web`: 默认 Web 配置插件（Koishi 风格控制台 + LiliaUI tokens）
- `mutsuki-plugin-bot-control-web`: ServiceHost ControlMethod 的 `control.*` Web RPC 代理（`runtime.read` 门禁）
- `mutsuki-plugin-bot-overview-web`: Web 概览（`overview.summary`：经 control-web 聚合状态/结构/计数/uptime）
- `mutsuki-bot-web-console`: 嵌入式 Bot 管理台装配（WebHost + control/overview/config/upgrade extensions）
- `examples/config-demo`: Discord-like 最小可用配置闭环

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

## Performance model

`mutsuki-bot-benchmarks` and `scripts/run-performance-model.py` implement the versioned Bot owner
workload for MutsukiBotPlugins #10 and MutsukiCore #35. The suite uses only deterministic fixtures
and loopback HTTP/WebSocket servers. It covers event bursts, 4/16-adapter fairness, command
hit/miss, link parsing, handler wait/resume, rate limiting, reconnect/resume, duplicate suppression,
an established idle WebSocket window, and bounded long-run retention.

Run a local reference report with:

```text
python scripts/run-performance-model.py \
  --mode reference \
  --process-runs 3 \
  --output artifacts/performance/issue10-reference.json
```

The raw samples, unified report, anomaly analysis, workload boundary, and revision-lock procedure
are documented in `docs/performance-model-issue10.md`.
