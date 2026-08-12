# Bot Protocol

Protocol IDs use `namespace.domain/op@major`.

Standard Bot protocols:

- `mutsuki.bot.event/ingest@1`
- `mutsuki.bot.event/handle@1`
- `mutsuki.bot.message/send@1`
- `mutsuki.bot.message/recall@1`
- `mutsuki.bot.media/upload@1`
- `mutsuki.bot.command/parse@1`
- `mutsuki.bot.command/handle@1`
- `mutsuki.bot.session/get@1`
- `mutsuki.bot.session/set@1`
- `mutsuki.bot.permission/check@1`
- `mutsuki.bot.delivery/reply@1`

`mutsuki.bot.delivery/reply@1` owns durable Bot reply delivery. `Submit` reserves one stable
reply id plus its ordered message parts before any platform send; `Inspect`, `RetryPart` and
`CancelPart` operate on persisted receipts, while `ResumeDue` claims due parts for restart
recovery. Each part is an ordinary `mutsuki.bot.message/send@1` task at the side-effect boundary.
Agent turn/session completion is source provenance, not evidence that every part was delivered.

QQBot-specific protocols:

- `mutsuki.bot.qqbot.raw/call@1`
- `mutsuki.bot.qqbot.account/get@1`
- `mutsuki.bot.qqbot.gateway/status@1`
- `mutsuki.bot.qqbot.capability/get@1`

The QQ capability response is account-scoped and additive. It reports the configured intent mask
and shard, required QQ permission categories, truthful inbound/outbound conversation and segment
surfaces, optional media constraints, reply/quote/mention, active delivery, edit/recall and
streaming strategy. The QQ-specific rate-limit policy declares server-governed quotas,
`Retry-After` handling, bounded retry attempts/backoff and the Gateway rate-limit delay. New fields
are serde-defaulted so older recorded responses remain readable.

Business plugins should prefer the standard protocols. Adapter-specific protocols are escape hatches.

Reserved standard protocol IDs:

- `mutsuki.bot.message/edit@1`
- `mutsuki.bot.media/download@1`

Reserved IDs are protocol crate constants, but a plugin only promises support when its manifest and runner descriptor list the protocol. The QQBot adapter does not provide message edit or media download until there is a concrete QQBot endpoint and resource-writer contract for those behaviors.
