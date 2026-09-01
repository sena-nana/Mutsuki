# Bot Protocol

Protocol IDs use `namespace.domain/op@major`.

Envelope IDs tag a `BotFlowEventEnvelope.protocol_id`. They are not runner protocols: a plugin
promises a runner only when its manifest and `RunnerDescriptor` list that ID.

- `mutsuki.bot.event/ingest@1` — Gateway-mapped Bot event envelope. Adapter runners submit
  `mutsuki.bot.flow/ingress@1` tasks that carry this envelope.
- `mutsuki.bot.command/handle@1` — Command Match `matched` output envelope (`BotCommandEvent`).

Domain plugins join Flow the same way: the plugin submits `mutsuki.bot.flow/ingress@1` envelopes
whose `payload.event_type` is a plugin-owned `mutsuki.bot.event.*` specialization, and the plugin
stops there — rendering and delivery belong to the graph. Bilibili polling submits
`mutsuki.bot.event.bilibili` v1 carrying a `BilibiliNotification`; the
`mutsuki.bot.bilibili.notification` Source node matches it by selector alone (it is intentionally
absent from the router kind table because the payload is not a `BotEvent`).

This makes Flow the only initiation surface for business behavior. Business plugins and business
EventSources submit through the SDK `BotSubmissionGate`, which fails loud on direct submissions of
`mutsuki.bot.message/send@1`, `mutsuki.bot.message/recall@1`, `mutsuki.bot.delivery/*` and
`mutsuki.bot.agent/*`; their manifests are validated with
`BotSubmissionGate::ensure_manifest_business_surface` before registration. Only the adapter (via
graph send nodes) and the durable delivery service write past the send boundary, and only
completion paths (delivery recovery, reserved drafts) act outside an active graph.

Standard Bot runner protocols:

- `mutsuki.bot.flow/ingress@1`
- `mutsuki.bot.flow.node/execute@1`
- `mutsuki.bot.flow.match/conversation@1`
- `mutsuki.bot.flow.match/user@1`
- `mutsuki.bot.flow.match/role@1`
- `mutsuki.bot.flow.match/prefix@1`
- `mutsuki.bot.flow.match/keyword@1`
- `mutsuki.bot.flow.match/mention@1`
- `mutsuki.bot.flow.match/empty-mention@1`
- `mutsuki.bot.flow.match/rate-limit@1`
- `mutsuki.bot.flow.match/qq-event@1`
- `mutsuki.bot.flow.match/interaction@1`
- `mutsuki.bot.flow.interaction/create@1`
- `mutsuki.bot.flow.match/probability@1`
- `mutsuki.bot.flow.match/link@1`
- `mutsuki.bot.interaction/handle@1`
- `mutsuki.bot.agent/submit@1`
- `mutsuki.bot.agent/cancel@1`
- `mutsuki.bot.agent/reset@1`
- `mutsuki.bot.agent/fork@1`
- `mutsuki.bot.agent/status@1`
- `mutsuki.bot.agent/regenerate@1`
- `mutsuki.bot.agent/bind-profile@1`
- `mutsuki.bot.agent/persona@1`
- `mutsuki.bot.agent/attach-bound-persona@1`
- `mutsuki.bot.conversation/record-icl@1`
- `mutsuki.bot.conversation/attach-icl@1`
- `mutsuki.bot.conversation/attach-identifiers@1`
- `mutsuki.bot.reply/quote@1`
- `mutsuki.bot.reply/mention@1`
- `mutsuki.bot.reply/segment@1`
- `mutsuki.bot.message/send@1`
- `mutsuki.bot.message/recall@1`
- `mutsuki.bot.media/upload@1`
- `mutsuki.bot.media/transcribe@1`
- `mutsuki.bot.media/synthesize@1`
- `mutsuki.bot.command/parse@1`
- `mutsuki.bot.command/reply@1`
- `mutsuki.bot.delivery/submit@1`
- `mutsuki.bot.delivery/reply@1`

`mutsuki.bot.flow.match/link@1` matches extracted URLs against a host allowlist and writes
`bot.link.url`. Processors such as Bilibili and Mihuashi prefer that URL.

These conversation, persona and reply protocols are independent node bindings, not
`BotAgentConfig` fields. `mutsuki.bot.agent/bind-profile@1` writes `bot.agent.runtime_profile_id`;
Agent loop plugins (compression, RAG, MCP, WebSearch) stay on `AgentRuntimeProfile`. Group ICL is
attached as `bot.conversation.icl` and consumed when the Agent node builds the user message.
Interaction create/match are sync nodes; waiter state lives in
`mutsuki.bot.interaction/handle@1`. V1 does not park the DAG. Graph wiring is in `event-router.md`.

`mutsuki.bot.qq.reply/forward-fold@1` is owned by the QQ Adapter: catalog node,
HandlerBinding, and OpenAPI runner. It is not a domain-neutral reply protocol.

`mutsuki.bot.message/send@1` carries a `BotMessage` whose `segments` are the plugin-composed
outbound types. Plugins do not declare sendable kinds through `PluginProvides`; the Adapter
capability matrix is the account-scoped source of truth. First-class `MessageSegment::Markdown`
is custom markdown content. QQ keyboard and template markdown stay `PlatformSpecific` under
`platform: "qqbot"`.

`PluginProvides.extensions` carries versioned domain-neutral extension payloads. Bot plugins use
`mutsuki.bot.flow.nodes@1` for their node catalog. `BotFlowEventEnvelope`, `BotNodeInvocation` and
`BotNodeResult` preserve typed ports plus Bot/trace/correlation context across graph execution.

`mutsuki.bot.delivery/submit@1` owns active (non-Agent) durable send: persist request, CAS-claim
due receipts, retry, and `mutsuki.bot.message/send@1` at the side-effect boundary.

`mutsuki.bot.delivery/reply@1` owns durable Bot reply delivery. `Reserve` persists one stable reply
id plus its ordered message parts without sending. Agent Flow occupancy sets `occupancy_only` so
`ResumeDue` will not send those drafts. `Submit` clears occupancy, replaces unsent parts, and
sends; `Inspect`, `RetryPart` and `CancelPart` operate on persisted receipts, while `ResumeDue`
claims due sendable parts for restart recovery. Each part is an ordinary
`mutsuki.bot.message/send@1` task at the side-effect boundary. Agent turn/session completion is
source provenance, not evidence that every part was delivered.

QQBot-specific protocols:

- `mutsuki.bot.qqbot.raw/call@1`
- `mutsuki.bot.qqbot.account/get@1`
- `mutsuki.bot.qqbot.gateway/status@1`
- `mutsuki.bot.qqbot.capability/get@1`

`mutsuki.bot.qqbot.account/get@1` returns the configured account id, App ID, the raw `/users/@me`
body as `openapi_user`, and a mapped `user` (`user_id`, `display_name`, `avatar_url`). Missing
avatars synthesize `https://q.qlogo.cn/qqapp/{app_id}/{user_id}/640`.

The QQ capability response is account-scoped and additive. It reports the configured intent mask
and shard, required QQ permission categories, truthful inbound/outbound conversation and segment
surfaces, optional media constraints, reply/quote/mention, active delivery, edit/recall and
streaming strategy. The QQ-specific rate-limit policy declares server-governed quotas,
`Retry-After` handling, bounded retry attempts/backoff and the Gateway rate-limit delay. New fields
are serde-defaulted so older recorded responses remain readable. Which official Gateway events and
message types are first-class versus `PlatformSpecific` or unsupported is recorded in
`docs/qqbot-adapter.md`.

Business plugins should prefer the standard protocols. Adapter-specific protocols are escape hatches.

JSON schemas under `schemas/` cover landed DTO families only. Reserved IDs have no schema.

Reserved IDs live in `mutsuki_bot_protocol::reserved` and have no runners:

- `mutsuki.bot.message/edit@1`, `mutsuki.bot.media/download@1` — no QQ endpoint yet
- `mutsuki.bot.session/get@1`, `mutsuki.bot.session/set@1`, `mutsuki.bot.permission/check@1` —
  session/permission plugins deferred; no public `BotPermission` DTO
- `mutsuki.bot.agent/handle@1` — superseded by `agent/submit@1` and the other Agent admin protocols
- `mutsuki.bot.flow.node/invoke@1` — superseded by `flow.node/execute@1`

Rate-limit matching is `mutsuki.bot.flow.match/rate-limit@1`; there is no parallel
`rate_limit/check` protocol.
