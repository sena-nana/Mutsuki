# QQBot Adapter

The QQBot adapter is a platform adapter, not a business bot plugin.

It owns:

- Gateway frame handling and reconnect/heartbeat actions.
- Raw QQBot event to standard `BotEvent` mapping.
- Standard Bot message/media/recall tasks to QQBot OpenAPI requests.
- QQBot-specific account, gateway status, and raw calls.
- Redaction for errors and returned evidence.

It does not own:

- Business commands.
- Business sessions.
- Permission policy.
- Host startup.
- Core scheduling.

Provided task protocols:

- `mutsuki.bot.message/send@1`
- `mutsuki.bot.message/recall@1`
- `mutsuki.bot.media/upload@1`
- `mutsuki.bot.qqbot.account/get@1`
- `mutsuki.bot.qqbot.gateway/status@1`
- `mutsuki.bot.qqbot.raw/call@1`

Not provided:

- `mutsuki.bot.message/edit@1`: QQBot group and C2C messages do not have an adapter-backed edit path here.
- `mutsuki.bot.media/download@1`: downloading remote media into Mutsuki resources needs an explicit resource-writer contract, not only an OpenAPI call.
