# QQBot Adapter

QQBot adapter owns Tencent QQ transport, event translation and OpenAPI effects. Business plugins
consume only `mutsuki.bot.*`; ServiceHost owns lifecycle and secrets; product configuration selects
the adapter through the configured plugin catalog.

## Supported surface

| Area | Support |
| --- | --- |
| Gateway | HTTPS discovery, WSS, Hello, Identify, Ready, heartbeat/ACK, Resume and reconnect |
| Inbound | Group @, C2C, guild/channel @ and direct messages; available delete, member and reaction events. Content `<@id>` / `@all` is inlined as mention segments. `<faceType>` tags are stripped so image placeholders do not appear as text. Attachments (CDN URL) map to `PlatformSpecific`. Custom markdown `content` maps to `MessageSegment::Markdown`; template markdown plus `ark` / `embed` / `keyboard` map to `PlatformSpecific`. A media provider may additionally add `Image`/`File`/`Audio`/`Video` ResourceRef segments. Group/C2C actors without `author.avatar` receive `https://q.qlogo.cn/qqapp/{app_id}/{user_openid}/640`; `group_openid` is not used. Guild `author.avatar` is kept as-is. QQ CDN `http://` avatar and attachment URLs are stored and fetched as `https://`. Gateway payloads that include `group_name` (or `name` on `GROUP_*` events) copy it to `qqbot.group_name`. |
| Standard effects | Group/C2C text, mention, reply, custom Markdown (optional keyboard on the same payload) and message recall |
| QQ-specific effects | Account query (mapped `user` plus raw `openapi_user`), Gateway query and relative-path raw call |
| Media | Validated image/audio/video/file `ResourceRef` input and Group/C2C upload/send, only when the product injects a real provider |
| Message edit / media download | Not provided |

The default configured factory is text-only. Its manifest does not claim
`mutsuki.bot.media/upload@1`. Products that own a resource provider may build
`QqBotPluginBundle::new(config)?.with_media_provider(factory)` and then install it explicitly.
Guild/channel messages are inbound-only in the standard adapter: the capability matrix does not
claim channel send, upload or recall endpoints.

`mutsuki.bot.qqbot.capability/get@1` returns the account-scoped source of truth. It includes inbound
and outbound conversation kinds and segment kinds, active-message kinds, edit/recall/reply/quote/
mention flags, media limits and MIME types, streaming strategy, the configured numeric intents and
shard, recognized intent names, required QQ permission categories, and the effective bounded retry/
rate-limit policy. QQ remains the quota authority; the adapter declares that it honors
`Retry-After` rather than publishing a guessed fixed request quota. Media segment and upload
permissions appear only when a real resource provider is configured. The matrix describes QQ only;
it is not a cross-platform compatibility promise.

First-class event kinds (and Flow sources) are message create/update/delete, reaction add/remove,
member join/leave, bot connected/disconnected, plus a catch-all platform source. Default Identify
is documented `GUILDS`, `GROUP_AND_C2C`, `INTERACTION`, `MESSAGE_AUDIT` and `PUBLIC_GUILD_MESSAGES`;
undocumented bit 24 is omitted. Official events for those intents, and forum/audio names if later
subscribed, are ingested. Unmapped kinds stay `BotEventKind::PlatformSpecific` and enter Flow through
`mutsuki.bot.qq.platform` (kind `platform_specific`). Filter them with `mutsuki.bot.match.qq_event`
on `qqbot.event_type` (or the platform-specific kind name): `INTERACTION_CREATE`,
`GROUP_ADD_ROBOT`/`DEL_ROBOT`, active-message switches, guild/channel create-update-delete, message
audit, forum, audio and `GUILD_MEMBER_UPDATE`. They are not promoted to first-class kinds.
`PUT /interactions/{id}` is not provided. Gateway session end emits `BotDisconnected` so
`mutsuki.bot.qq.bot.disconnected` can start a chain.

Inbound gaps: Group/C2C cards use `ark_data` (only `ark` is read); `message_type` 3/101/102/103
and `msg_elements` are ignored; face/attachment/keyboard/embed stay `PlatformSpecific`;
`message_scene` is not copied. `author.member_role` is mapped into `qqbot.actor_role` (`owner` /
`administrator` / `member`) together with `member.roles`. Outbound gaps: no Ark, Embed,
`msg_type=6`, channel send, or `event_id` replies to non-message events.

## Configuration

Select the native plugins under `[[plugins.configured]]`:

- `mutsuki.bot.router.flow`: empty owner config; graph state is stored in the Bot repository.
- `mutsuki.bot.command`: optional empty owner config contributing the Command Match node.
- `mutsuki.bot.adapter.qqbot`: owner config is `QqBotConfig`.

After startup, publish a graph that connects the QQ Source to Match/Processor/Sink nodes. There is
no TOML subscription list, global command prefix or adapter-owned Handler switch.

QQ fields are decoded strictly; unknown fields fail startup. Required fields are `account_id` and
`app_id` when the adapter is enabled. `client_secret_key` identifies a Host secret and defaults to
`QQBOT_CLIENT_SECRET`. The product Config / Web Console login form exposes enablement, App ID,
Client Secret, and two receive switches (private/group and guild). Those switches update only the
corresponding Gateway intent bits; remaining network, intent, shard, timeout, retry, queue, dedup
and reconnect fields stay in the hidden runtime document and use the defaults returned by
`QqBotConfig::new` unless overridden. Disabling the adapter does not require a complete login.

Never place a client secret or access token in configured plugin data. For a key named
`QQBOT_CLIENT_SECRET`, ServiceHost reads `MUTSUKI_SECRET_QQBOT_CLIENT_SECRET` by default. The value
stays inside the EventSource/OpenAPI boundary and is cleared on stop.

The repository deliberately does not commit a complete runnable TOML. Product repositories create
their local configuration outside Git or generate it in a temporary directory during tests.

## Runtime and health

`configured_bot_plugin_catalog()` returns factories for the Flow router, command node and text-only
QQ adapter. Register it on `ServiceRuntimeBuilder`; configured plugins are installed before
RuntimeProfile/LoadPlan freeze. Unknown catalog IDs, raw credential fields, missing Host secrets and
invalid QQ URLs fail before the service becomes healthy.

Health exposes `connected`, `identified`, last heartbeat/ACK/event timestamps, reconnect count,
last error, and the bot self profile from `/users/@me` (READY only fills empty fields).
`event_source_list` shows the source lifecycle; `event_source_restart` performs an
explicit supervised restart. Logs use account ID, a session digest, event type, sequence and
correlation ID, never credentials or authorization headers.

Sandbox live group titles never display `group_openid`. The EventSource copies `qqbot.group_name`
when the Gateway payload has it, then asynchronously calls `GET /v2/groups/{group_openid}/info`
(30 QPM). Permission error 11253 and other non-retryable failures are cached for the process
lifetime; the console shows `群聊` until a real name is known.

Failures are classified as recoverable disconnect, Gateway rate limit, Identify/Resume
rejection, auth/config rejection or permanent protocol/account rejection. Opcode 9
(Invalid Session) reconnects with `qq.gateway.identify_rejected`. Waiting for READY
after Identify uses the HELLO timeout. Close 4006/4007 discard the session and
Identify (`qq.gateway.session_invalid`); 4009 and opcode 7 preserve the session and
Resume. Close 4001/4002/4010-4014 and 4914/4915 stop the Gateway. OpenAPI task
failures expose stable `qqbot.openapi.*` codes plus
`classification`, `retryable`, optional `retry_after_ms` and HTTP status evidence. HTTP 401 refreshes
once; 429/5xx retries are bounded by config and server headers. Gateway queues and event
deduplication windows are bounded. Dedup keys include event kind plus message ID, so a reconnect
replay is suppressed without hiding a later delete event for the same message. Normalized events
carry `qqbot.sequence` as ordering information.

Standard outbound `Reply` and `Quote` segments lower to QQ's `msg_id`, the same as
`BotMessage.reply_to`. Empty or conflicting message IDs return `qqbot.openapi.invalid_request`
before any network request; they never fall back to a new active message. Omitting `reply_to`
sends a Group/C2C active message (no `msg_id`). Standard `message/send` sends Group/C2C custom
Markdown as `msg_type=2` (`MessageSegment::Markdown`). A single `qqbot` `keyboard`
`PlatformSpecific` segment may attach to that same payload. Custom markdown `content` must use the
first-class segment; QQ template markdown (`custom_template_id`) stays `PlatformSpecific`. Mixing
Markdown with text, mention or Ark in one send group fails before any network request. Ark is still
not sent. The sandbox allows unquoted live send by default, and
records `GROUP_MSG_REJECT` / `C2C_MSG_REJECT` (or bot removal) as closed.

## Verification

- Unit tests cover config validation, token expiry/refresh, 401/429/5xx, response limits, redaction,
  batch isolation, event mapping and capability surfaces.
- `mutsuki-bot-testkit::FakeQqServer` provides real local HTTP/WebSocket boundaries for product E2E,
  including Identify, heartbeat, reconnect and Resume. Its scripted mode drives private, group,
  channel, replay and deletion events through a real `ServiceRuntime`.
- The inbound media loopback test downloads and validates image/audio/video/file bytes for private,
  group and channel messages before publishing sealed, hashed `ResourceRef` values.
- `examples/service-host-example` starts the real `ServiceRuntime` through configured factories and
  verifies `/echo`, `/ping`, health, task correlation, secret isolation and graceful shutdown.
- A real-account smoke uses an ignored local config and Host environment secret. It is successful
  only after a real group `/ping` and `/echo` produce successful QQ OpenAPI tasks; fake results must
  not be reported as a real smoke.

Protocol behavior should be checked against the current QQ Open Platform documentation and
Tencent's official reference implementations before changing opcodes, close-code handling, event
names or message payloads.

The Issue #141 audit checked Tencent `tencent-connect/openclaw-qqbot` revision
`47142c997bdbc9e72d92b817ff378941b3be7d4c` for current Group/C2C rich-media behavior and
`tencent-connect/botgo` revision `fe31c0dfe469001e0f783d2f07e7de7bd08b403f` for the official
Gateway event/intent/opcode tables, including guild/channel events.
