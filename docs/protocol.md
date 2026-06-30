# Bot Protocol

Protocol IDs use `namespace.domain/op@major`.

Standard Bot protocols:

- `mutsuki.bot.event/ingest@1`
- `mutsuki.bot.event/handle@1`
- `mutsuki.bot.message/send@1`
- `mutsuki.bot.message/edit@1`
- `mutsuki.bot.message/recall@1`
- `mutsuki.bot.media/upload@1`
- `mutsuki.bot.media/download@1`
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
