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

QQBot-specific protocols:

- `mutsuki.bot.qqbot.raw/call@1`
- `mutsuki.bot.qqbot.account/get@1`
- `mutsuki.bot.qqbot.gateway/status@1`

Business plugins should prefer the standard protocols. Adapter-specific protocols are escape hatches.

Reserved standard protocol IDs:

- `mutsuki.bot.message/edit@1`
- `mutsuki.bot.media/download@1`

Reserved IDs are protocol crate constants, but a plugin only promises support when its manifest and runner descriptor list the protocol. The QQBot adapter does not provide message edit or media download until there is a concrete QQBot endpoint and resource-writer contract for those behaviors.
