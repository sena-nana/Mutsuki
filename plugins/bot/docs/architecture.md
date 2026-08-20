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
  -> mutsuki.bot.delivery/reply@1
  -> before the Agent node returns, atomically reserve reply bundle + ordered Pending part receipts
  -> CAS claim one part
  -> mutsuki.bot.message/send@1
  -> persist attempt + platform receipt
```

Stable reply and part ids make replay an inspection of the existing bundle. The Flow delivery
node sends an already reserved bundle; if the Host exits between the Agent and delivery nodes, the
recovery source can claim those Pending parts without replaying the Agent. A transient failure
only schedules the unconfirmed part; already succeeded parts remain terminal. Cancellation or a
Runtime timeout after the send boundary becomes `ReconcileRequired`, because automatic resend
could duplicate an externally accepted message. `BotReplyDeliveryRecoveryEventSource` asks the
same delivery plugin to resume due parts after ServiceHost startup, so recovery does not invoke the
Agent turn, media synthesis, or tools again.

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

- `mutsuki-bot-protocol`: pure Bot data contracts.
- `mutsuki-bot-sdk`: author helpers over Bot protocol tasks.
- `mutsuki-bot-flow`: Bot-owned catalog validation and atomic active snapshot provider.
- `mutsuki-plugin-bot-event-router`: revision-pinned DAG execution and graph-owned match nodes.
- `mutsuki-plugin-bot-command`: graph-configured command Match node.
- `mutsuki-plugin-bot-agent`: Agent turn/session bridge and durable reply request producer.
- `mutsuki-bot-delivery`: attempt, receipt, retry, CAS claim and reply-part delivery behavior.
- `mutsuki-bot-state-db`: durable session, delivery, interaction and sandbox history
  repository; historical Flow tables are neither read nor destructively removed.
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
- `mutsuki-plugin-bot-sandbox-web`: WebExtension for the shared simulate/live conversation client
  (Stapxs-style three-pane session / chat / member layout).
- `mutsuki-plugin-bot-adapter-qqbot`: QQBot platform translation and OpenAPI side effects.
- `mutsuki-bot-service-host-integration`: EventSource, health, ServiceRuntime assembly, and
  sandbox outbound intercept for `sandbox:` conversations (Gateway / `message/send`). QQ Adapter
  still only translates official protocol.
- `examples/bot-echo`: platform-neutral example business plugin over `mutsuki.bot.*` only.

## Deferred Plugins

Session and permission plugins are intentionally not part of the MVP workspace until a concrete behavior path needs them.
