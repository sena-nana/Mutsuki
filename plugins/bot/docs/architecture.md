# Architecture

MutsukiBotPlugins sits above Mutsuki Core. It contributes ordinary plugins and protocol crates that a Host may load through a `RuntimeLoadPlan`.

```text
QQBot Gateway
  -> mutsuki-plugin-bot-adapter-qqbot
  -> mutsuki.bot.flow/ingress@1
  -> immutable applied Bot Flow configuration revision
  -> Match / Processor nodes through exact bindings
  -> Delivery / QQ Sink node
  -> mutsuki-plugin-bot-adapter-qqbot
  -> QQBot OpenAPI
```

Bot Agent replies take a durable route rather than calling the send protocol directly:

```text
Bot Agent final turn
  -> Flow Agent node emits mutsuki.bot.delivery/reply@1 request on the reply port and
     Reserves a Pending bundle (same reply_id)
  -> quote / mention / segment / QQ forward-fold may change parts
  -> Flow delivery node Submit clears occupancy-only, replaces unsent parts, then sends
  -> CAS claim one part
  -> mutsuki.bot.message/send@1
  -> persist attempt + platform receipt
```

Direct `mutsuki.bot.agent/submit@1` (outside a Flow node) still `Submit`s delivery itself. The
Flow Agent node Reserves occupancy-only parts so `ResumeDue` will not send an unpresented draft.
Delivery `Submit` clears occupancy, replaces unsent parts, then sends. Stable reply and part ids
make replay an inspection of the existing bundle. After Submit,
`BotReplyDeliveryRecoveryEventSource` claims due parts without replaying the Agent. A crash
between Agent and delivery still has reserved receipts, but those drafts stay unsent until
Submit. A transient failure only schedules the unconfirmed part; already succeeded parts remain
terminal. Cancellation or a Runtime timeout after the send boundary becomes `ReconcileRequired`,
because automatic resend could duplicate an externally accepted message.

Core still sees only tasks, runner descriptors, results, events, resource refs, and effect requests. It does not know Bot, QQBot, commands, sessions, or permissions.

`QqBotPluginBundle` lives in `mutsuki-bot-service-host-integration`, the explicit bridge between
the platform adapter and ServiceHost. It registers the adapter
manifest and recreatable native batch runners with `ServiceRuntimeBuilder`, and
registers `QqGatewayEventSource` as a Host-managed long-lived source. The source
receives its client secret through `HostEventSourceConfig::secret` and can enter
Core only through the injected `TaskSubmitter`; it has no Core internals,
business command parser, or direct runner invocation.
The bundle also registers a domain-neutral ServiceHost health component that
publishes the Gateway connection, identification, heartbeat, ACK, event,
reconnect and last-error snapshot through the standard health control surface.

Simulate-mode sandbox traffic stays on the same QQ-shaped Bot contracts, but
`sandbox:` conversation ids are intercepted by the ServiceHost integration
before `mutsuki.bot.message/send@1` reaches QQ OpenAPI:

```text
Sandbox console (virtual user)
  -> mutsuki.bot.flow/ingress@1
  -> Match / Processor nodes
  -> mutsuki.bot.message/send@1 or delivery/reply@1
  -> sandbox outbound sink
  -> sandbox console (bot bubble)
```

Messages the bot itself sends are projected as bot bubbles and never start Flow Source chains.

## Crate Responsibilities

Libraries (not host-loadable plugins):

- `mutsuki-bot-protocol`: pure Bot data contracts. Envelope IDs (`event/ingest`, `command/handle`)
  are not runner protocols.
- `mutsuki-bot-sdk`: author helpers over Bot protocol tasks.
- `mutsuki-bot-flow`: Bot-owned catalog validation and atomic active snapshot provider.
- `mutsuki-bot-conversation`: conversation/session repository traits, binding keys, and
  `ConversationContextStore` (ICL/identifier persistence contract).
- `mutsuki-bot-persona`: `PersonaStore` persistence contract. Plugin runners live in
  `mutsuki-plugin-bot-persona`.
- `mutsuki-bot-link-parser`: shared URL/card extraction for link-match flows.
- `mutsuki-bot-management`: headless QQ/Bilibili management API traits.
- `mutsuki-bot-testkit`: fake QQ HTTP/WS boundary for E2E.
- `mutsuki-bot-benchmarks`: owner performance workloads. Not a production plugin.
- `mutsuki-plugin-catalog`: module upgrade catalog CLI. Not Bot protocol; lives under this
  package only as a workspace member.

Domain plugins and durable services:

- `mutsuki-plugin-bot-event-router`: revision-pinned DAG execution and graph-owned match nodes.
- `mutsuki-plugin-bot-command`: graph-configured command Match node (`command/parse@1`).
- `mutsuki-plugin-bot-agent`: Agent turn/session bridge and durable reply request producer.
- `mutsuki-plugin-bot-conversation-context`: record/attach group ICL and session identifiers.
- `mutsuki-plugin-bot-persona`: persona command and attach-bound-persona processors.
- `mutsuki-plugin-bot-reply`: quote/mention/segment presentation; QQ forward-fold stays in Adapter.
- `mutsuki-plugin-bot-media`: Agent media transcribe/synthesize bridge node.
- `mutsuki-bot-delivery`: attempt, receipt, retry, CAS claim and delivery services.
  `DeliveryGateway` is platform-neutral (`BotTarget`); QQ OpenAPI mapping stays in Adapter/integration.
- `mutsuki-plugin-bot-delivery`: `delivery/submit@1`, `delivery/reply@1` and scheduled-result plugin manifests.
- `mutsuki-bot-interaction`: durable multi-step waiter service and repository traits.
- `mutsuki-plugin-bot-interaction`: `interaction/handle@1` plus match/create node plugin manifests.
- `mutsuki-plugin-bot-bilibili` / `mutsuki-plugin-bot-bilibili-workshop` / `mutsuki-plugin-bot-mihuashi`:
  platform processors that consume `bot.link.url`.
- `mutsuki-bot-state-db`: durable session, delivery, interaction, persona, conversation-context
  and sandbox history repository; implements store traits from library crates rather than
  depending on plugin packages. Historical Flow tables are neither read nor destructively removed.
  Sandbox live/simulate conversations, roster users, messages, content-addressed
  image assets, custom stickers and official face IDs persist in `bot_sandbox_*`
  tables. Other plugins query those tables through
  `BotStateDbRepository` or `inspect_rows`; sandbox startup hydrates from a snapshot.
- `mutsuki-bot-sandbox`: QQ conversation sandbox with durable history in `BotStateDb`.
  Simulate mode is a Koishi-style closed loop (virtual users always enter `mutsuki.bot.flow/ingress@1`,
  outbound `message/send` for `sandbox:` conversations is intercepted back into the
  console). Simulate roster includes a non-editable bot identity; speaking as the bot
  only appends a bubble and does not enter Flow. Roster OpenID/nickname can be edited, and observed live members
  can be imported into simulate together with their avatar URL. Simulate compose
  can send mentions, local media (file picker or clipboard paste),
  custom stickers and official faces into Flow. Compose puts image, file, sticker,
  Markdown and keyboard icons left of the input in both simulate and live.
  Markdown sends `MessageSegment::Markdown`; keyboard is optional `qqbot`
  PlatformSpecific on the same message. Reply is an icon on the bubble. The console
  still renders live inbound attachments/ark/markdown/keyboard. Live outbound stays on Adapter-supported
  text, mention, media and custom Markdown (optional keyboard). Messages persist as
  plain text plus a compact hash index.
  Image bytes are stored once in `bot_sandbox_asset` keyed by `sha256:`; QQ wrapping
  the same file in a new CDN URL / `ResourceRef` only writes an index hit and
  refreshes the display URL. Custom stickers live in `bot_sandbox_sticker` (separate
  blobs, not subject to image GC). Official faces are recorded in `bot_sandbox_face`
  as `qq:{faceType}:{faceId}` without a blob so history restore and the sticker
  picker can list seen IDs. Live mode projects real Gateway inbound events and
  confirmed bot sends, including live member avatar URLs. Bot bubbles use the connected bot's
  name and avatar when known, otherwise `机器人`. Live group session titles prefer
  `qqbot.group_name` from the event, then `GET /v2/groups/{group_openid}/info`
  (30 QPM, whitelist-only; 11253 is cached as unavailable). Failures and missing names
  display `群聊` rather than the OpenID. Group sessions show a
  member photo. The right-hand roster shows nicknames only. The member context menu
  inserts `@nickname` into compose, copies OpenID, and can import a live member into
  the simulate roster. Typing `@` in compose opens a compact mention menu above the
  input; an unmatched query closes it.
  Console CSP allows QQ image CDNs (`*.qlogo.cn`, `*.qpic.cn`,
  `*.gtimg.cn`, `*.qq.com.cn`, `*.nt.qq.com.cn`) over HTTPS only. Adapter
  and sandbox upgrade QQ CDN `http://` URLs to `https://` before persist or
  display; sandbox `<img>` uses `referrerpolicy=no-referrer`.
  Unquoted live send is allowed by default. `GROUP_MSG_REJECT` / `C2C_MSG_REJECT`
  (or bot removal) turn it off; `GROUP_MSG_RECEIVE` / `C2C_MSG_RECEIVE` turn it
  back on. Failed receipts are surfaced instead of a local bubble.
- `mutsuki-plugin-bot-adapter-qqbot`: QQBot platform translation and OpenAPI side effects.

WebExtensions and product-facing assembly:

- `mutsuki-plugin-bot-sandbox-web`: simulate/live conversation client.
- `mutsuki-plugin-bot-agent-web`: Agent connection management.
- `mutsuki-plugin-bot-control-web`: ServiceHost control RPC.
- `mutsuki-plugin-bot-overview-web`: overview dashboard.
- `mutsuki-plugin-bot-database-web`: BotStateDb browser.
- `mutsuki-plugin-bot-flow-web`: Flow node editor.
- `mutsuki-plugin-bot-qq-web`: QQ management console.
- `mutsuki-plugin-bot-bilibili-web`: Bilibili management console.
- `mutsuki-plugin-bot-upgrade-web`: module upgrade UI.
- `mutsuki-bot-web-console`: Bot-package WebHost assembly helper that embeds the admin
  WebExtensions. Products may opt in; this crate is not a Host and not a product entry.
- `mutsuki-bot-service-host-integration`: explicit ServiceHost assembly (EventSource, health,
  catalog factories, sandbox outbound intercept). It may ship first-party default Flow graphs
  (`qq_ai_orchestrated_flow`); user graphs live in ConfigService. Do not add business runners
  here. QQ Adapter still only translates official protocol.
- `examples/bot-echo`: platform-neutral example business plugin over `mutsuki.bot.*` only.

First-party product (`products/bot`) may compile against owner plugin config schemas. That is
schema ownership, not a hardcoded backend substitute path. `mutsuki-bot-runtime-reference` is a
domain-topology bench with reference runners; it is not a production entry.

## Deferred Plugins

Session and permission plugins are intentionally not part of the MVP workspace until a concrete
behavior path needs them. Their protocol IDs stay reserved constants without runners. Rate-limit
matching is the graph-owned `mutsuki.bot.flow.match/rate-limit@1` node, not a standalone
`rate_limit/check` protocol.
