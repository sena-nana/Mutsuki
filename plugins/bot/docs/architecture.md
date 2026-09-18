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

Flow is the only initiation surface for Bot business behavior. Chat events, sandbox simulate
traffic and domain push pipelines all enter through `mutsuki.bot.flow/ingress@1`; business
plugins are invoked only through graph node bindings. Business EventSources submit through the
SDK `BotSubmissionGate`, which rejects direct submissions of `mutsuki.bot.message/send@1`,
`mutsuki.bot.message/recall@1`, `mutsuki.bot.delivery/*` and `mutsuki.bot.agent/*`, and their
manifests are validated against that surface at assembly. A graph with no matching Source chain
freezes the business behavior behind the event; the freeze is observable through the ingress
stats (`accepted_total` / `dropped_total`) on the `mutsuki.bot.flow.ingress` health snapshot.
That snapshot also carries `active_flow_nodes` and reports `degraded` when the seed or restore
of the stored Flow document failed (`seed_failure`), or when ingress accepted events while the
active graph is empty (`frozen_reason`). Both run on a detached task after LoadPlan activation,
so without this the operator would see a healthy process routing nothing.
The Flow Agent node Reserves occupancy-only parts so `ResumeDue` will not send an unpresented
draft. Delivery `Submit` clears occupancy, replaces unsent parts, then sends. Stable reply and part ids
make replay an inspection of the existing bundle. After Submit,
`BotReplyDeliveryRecoveryEventSource` claims due parts without replaying the Agent. These
completion paths drain effects that a Flow chain already initiated; they never originate new
business behavior, which is why they stay graph-independent. A crash
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
- `mutsuki-bot-conversation`: conversation/session repository traits, binding keys,
  `ConversationContextStore` (ICL/identifier persistence contract) and `PersonaStore`.
  The persona contract lives here rather than in its own crate: both are plain
  persistence traits over conversation-scoped state and both are implemented by
  `mutsuki-bot-state-db`. Plugin runners live in `mutsuki-plugin-bot-persona`.
- `mutsuki-bot-link-parser`: shared URL/card extraction for link-match flows.
- `mutsuki-bot-secure-fetch`: allowlisted HTTP fetch with manual redirects and a hard
  streamed size budget. Bilibili and Workshop each had their own copy of this loop, and
  two copies of an SSRF guard means a hop-validation fix can land in only one of them.
  Node runners should submit `mutsuki.std.io.http` instead — the std plugin owns that
  policy; this exists for the synchronous transports that hold no task submitter.
- `mutsuki-bot-management`: headless QQ/Bilibili management API traits.
- `mutsuki-bot-testkit`: fakes for E2E — the QQ HTTP/WS boundary, plus the in-memory
  `ConversationContextStore` / `PersonaStore` doubles. Production stores are always
  `BotStateDbRepository`; keeping the doubles here stops a plugin from reaching one.
- `mutsuki-bot-benchmarks`: owner performance workloads. Not a production plugin.
(The module upgrade catalog CLI used to live here as `mutsuki-plugin-catalog`. It owns no Bot
protocol and builds no `PluginBuilder`, so both its name and its location were wrong; it is now
`crates/mutsuki-module-catalog`, still shipping the `mutsuki-plugin` binary.)

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
  Registered by `BotAgentConfiguredPlugin` against the shared `BotStateDb` handle, alongside
  conversation-context, reply and persona. The seeded `qq_full_business_flow` reference graph
  uses `mutsuki.bot.interaction.create` / `.match`, so these manifests are part of the AI chain
  rather than an independently selectable factory.
- `mutsuki-plugin-bot-bilibili` / `mutsuki-plugin-bot-bilibili-workshop` / `mutsuki-plugin-bot-mihuashi`:
  platform processors that consume `bot.link.url`. Bilibili polling only detects fresh items and
  submits `mutsuki.bot.event.bilibili` trigger events into Flow ingress; the push card render and
  delivery live in the `mutsuki.bot.bilibili.notification` → `mutsuki.bot.bilibili.card` graph
  nodes.
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

None of these build a `PluginBuilder`; they implement `WebExtension` and ship an
`ExtensionManifest`. They therefore do not carry the `mutsuki-plugin-*` name, which is
reserved for host-loadable plugin surfaces, and they share their skeleton
(`BundledManifest`, `BundledAssets`, `materialize_bundled_assets`) from
`mutsuki-web-extension-api` rather than restating it per crate.

Bot-owned pages live here:

- `mutsuki-bot-web-extension-sandbox`: simulate/live conversation client.
- `mutsuki-bot-web-extension-database`: BotStateDb browser.
- `mutsuki-bot-web-extension-flow`: Flow node editor.
- `mutsuki-bot-web-extension-qq`: QQ management console.
- `mutsuki-bot-web-extension-bilibili`: Bilibili management console.

Pages with no Bot content live with their owners, and the console composes them:

- `hosts/web/crates/mutsuki-web-extension-control`: ServiceHost control RPC.
- `hosts/web/crates/mutsuki-web-extension-overview`: overview dashboard.
- `hosts/web/crates/mutsuki-web-extension-upgrade`: module upgrade UI, over
  `crates/mutsuki-module-catalog`.
- `kits/agent/crates/mutsuki-agent-web-extension`: Agent connection management.
- `mutsuki-bot-web-host-integration`: Bot-package WebHost assembly helper that embeds the admin
  WebExtensions. Products may opt in; this crate is not a Host and not a product entry.
- `mutsuki-bot-service-host-integration`: explicit ServiceHost assembly (EventSource, health,
  catalog factories, sandbox outbound intercept). It may ship first-party default Flow graphs
  (`qq_ai_orchestrated_flow`, `qq_link_resolve_flow`, `bilibili_push_flow`, merged into
  `qq_full_business_flow` with example `configs/flow-full.example.json`); user graphs live in
  ConfigService, and first-party products seed `qq_full_business_flow` into stores without a
  flow record while never overwriting existing ones. Do not add business runners
  here. QQ Adapter still only translates official protocol.
- `examples/bot-echo`: platform-neutral example business plugin over `mutsuki.bot.*` only.

`QqAiBotPluginBundle` (in `mutsuki-bot-service-host-integration`) is the fully-injected
assembly used by the `qq_ai_pipeline` end-to-end test, not a production entry. Products assemble
through `configured_bot_plugin_catalog_with_agent_and_flow`, whose factories build their own
SQLite handle, Agent client and delivery gateway from saved configuration; the bundle exists
because an E2E has to substitute those three and the configured factories deliberately expose no
injection point. Both register the same `bot_agent_chain_manifests` list, so the test cannot pass
against a node catalog production would never build.

First-party product (`products/bot`) may compile against owner plugin config schemas. That is
schema ownership, not a hardcoded backend substitute path. `mutsuki-bot-runtime-reference` is a
domain-topology bench with reference runners; it is not a production entry.

## Deferred Plugins

Session and permission plugins are intentionally not part of the MVP workspace until a concrete
behavior path needs them. Their protocol IDs stay reserved constants without runners. Rate-limit
matching is the graph-owned `mutsuki.bot.flow.match/rate-limit@1` node, not a standalone
`rate_limit/check` protocol.

`mutsuki-plugin-bot-media` and the scheduled-delivery surface of `mutsuki-plugin-bot-delivery`
are built but deliberately **not** registered in the production catalog: no production type
implements `MediaService`, `ScheduledDeliveryTargetResolver` or `ScheduledDeliveryPolicyProvider`
(the only implementors are test doubles). Registering them would publish
`mutsuki.bot.media.*` / `mutsuki.bot.delivery.scheduled` node types that no backend can serve,
which the package rules forbid. Neither node type appears in the seeded reference graph. They
become registrable the moment a real backend lands in its owner package; until then the absence
is the truthful capability projection, not a gap to paper over.
