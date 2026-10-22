---
name: platform-adapters
description: Implement or change QQBot or other platform gateway connections, authentication, payload translation, OpenAPI operations, media handling, reconnect behavior, rate limits, or platform-specific protocols.
---

# Platform Adapters

- Translate platform events into generic Bot protocols and generic operations back into platform requests.
- Declare the adapter contract: every adapter manifest carries a `BotAdapterDescriptor` as the
  `mutsuki.bot.adapter@1` extension (platform, served accounts, the send/recall/upload endpoints,
  its config provider). Hosts, delivery and the console discover adapters through
  `BotAdapterDirectory::from_manifests`, never through hard-coded plugin ids; every endpoint the
  descriptor names must be bound and served by a runner of the same manifest.
- Outbound is account-routed. Never accept `mutsuki.bot.message/send@2`, `recall@2` or
  `media/upload@2` in an adapter: the Host admits one runner per protocol and the Flow outbound
  router owns those ids. Declare adapter-owned endpoint protocols instead (QQ:
  `mutsuki.bot.qqbot.message/send@1`, …; sandbox: `mutsuki.bot.sandbox.message/send@1`), accept
  their `BotAccountRequest` payload and refuse an account you do not serve with
  `bot.adapter.account_mismatch`. Build conversation refs with
  `mutsuki_bot_conversation::conversation_from_event`, which keeps the event's platform.
- An adapter never wraps another adapter's runners. The sandbox is the reference simulated
  adapter (`mutsuki-plugin-bot-adapter-sandbox`): it composes into Flow ingress, answers through
  its endpoint and observes real adapters only through the Flow registry's ingress observers.
- Neutral capabilities (`BotAdapterCapabilities`, `BotSegmentKind`, `BotStreamingStrategy`,
  `BotUploadConstraints`, `BotRateLimitPolicy`) live in `mutsuki-bot-protocol`. Platform-only
  contract (QQ protocol ids, `qqbot.openapi.*` error codes, intents/permissions in
  `QqBotCapabilityMatrix`) lives in `mutsuki-bot-qq-protocol`, which business plugins never import.
- Keep platform-only fields under the platform namespace or adapter internals.
- Obtain credentials from Host secret injection; redact tokens and platform-sensitive payloads.
- Keep transport clients and sockets inside Gateway objects; wrap them as Host EventSources only in the integration crate. The QQ socket loop is `gateway::QqGatewayClient` (driven by `QqGatewayHost`: submitter, shutdown flag, log); change reconnect, heartbeat or close handling there, not in the wrapper. Use ResourceRef for large media.
- Require explicit media/resource providers; never install an unavailable production fallback.
- Platform plugins that fetch the web (Bilibili, Workshop, Mihuashi) hold no HTTP client: they
  await `mutsuki.http.request@2` child tasks with a per-request `domain_allowlist`, pass response
  body resources straight through, and reference credentials only as `secret_headers` /
  `capture_cookies` keys that the product grants to their domain.
- Report disconnect, auth, rate-limit and unsupported operation failures structurally.

Test translation, reconnect, heartbeat, retry/rate limit, media and redaction with external-boundary fakes.

Large media writes from AsyncBatchHandler must use `spawn_blocking` around the synchronous Host bridge, or migrate to AsyncResourceRegistryGateway; never block the async executor on SQLite.
