# QQBot Adapter

QQBot adapter owns Tencent QQ transport, event translation and OpenAPI effects. Business plugins
consume only `mutsuki.bot.*`; ServiceHost owns lifecycle and secrets; product configuration selects
the adapter through the configured plugin catalog.

## Supported surface

| Area | Support |
| --- | --- |
| Gateway | HTTPS discovery, WSS, Hello, Identify, Ready, heartbeat/ACK, Resume and reconnect |
| Inbound | Group @ messages, C2C messages, robot/friend/member lifecycle and reactions |
| Standard effects | Text send and message recall |
| QQ-specific effects | Account query, Gateway query and relative-path raw call |
| Media upload | Only when the product injects a real `QqMediaProvider` |
| Message edit / media download | Not provided |

The default configured factory is text-only. Its manifest does not claim
`mutsuki.bot.media/upload@1`. Products that own a resource provider may build
`QqBotPluginBundle::new(config)?.with_media_provider(factory)` and then install it explicitly.

## Configuration

Select the three native plugins under `[[plugins.configured]]`:

- `mutsuki.bot.router.event`: owner config contains non-empty `subscriptions`.
- `mutsuki.bot.command`: owner config contains non-empty `prefixes`.
- `mutsuki.bot.adapter.qqbot`: owner config is `QqBotConfig`.

QQ fields are decoded strictly; unknown fields fail startup. Required fields are `account_id` and
`app_id`. `client_secret_key` identifies a Host secret and defaults to
`QQBOT_CLIENT_SECRET`. Network, intent, shard, timeout, retry, queue, dedup and reconnect fields use
the defaults returned by `QqBotConfig::new` unless overridden.

Never place a client secret or access token in configured plugin data. For a key named
`QQBOT_CLIENT_SECRET`, ServiceHost reads `MUTSUKI_SECRET_QQBOT_CLIENT_SECRET` by default. The value
stays inside the EventSource/OpenAPI boundary and is cleared on stop.

The repository deliberately does not commit a complete runnable TOML. Product repositories create
their local configuration outside Git or generate it in a temporary directory during tests.

## Runtime and health

`configured_bot_plugin_catalog()` returns factories for the router, command parser and text-only
QQ adapter. Register it on `ServiceRuntimeBuilder`; configured plugins are installed before
RuntimeProfile/LoadPlan freeze. Unknown catalog IDs, raw credential fields, missing Host secrets and
invalid QQ URLs fail before the service becomes healthy.

Health exposes `connected`, `identified`, last heartbeat/ACK/event timestamps, reconnect count and
last error. `event_source_list` shows the source lifecycle; `event_source_restart` performs an
explicit supervised restart. Logs use account ID, a session digest, event type, sequence and
correlation ID, never credentials or authorization headers.

Failures are classified as recoverable disconnect, Gateway rate limit, auth/config rejection or
permanent account rejection. HTTP 401 refreshes once; 429/5xx retries are bounded by config and
server headers. Gateway queues and event deduplication windows are bounded.

## Verification

- Unit tests cover config validation, token expiry/refresh, 401/429/5xx, response limits, redaction,
  batch isolation, event mapping and capability surfaces.
- `mutsuki-bot-testkit::FakeQqServer` provides real local HTTP/WebSocket boundaries for product E2E,
  including Identify, heartbeat, reconnect and Resume.
- `examples/service-host-example` starts the real `ServiceRuntime` through configured factories and
  verifies `/echo`, `/ping`, health, task correlation, secret isolation and graceful shutdown.
- A real-account smoke uses an ignored local config and Host environment secret. It is successful
  only after a real group `/ping` and `/echo` produce successful QQ OpenAPI tasks; fake results must
  not be reported as a real smoke.

Protocol behavior should be checked against the current QQ Open Platform documentation and
Tencent's official reference implementations before changing opcodes, close-code handling, event
names or message payloads.
