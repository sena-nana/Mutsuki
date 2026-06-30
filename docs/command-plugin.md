# Command Plugin

The command plugin parses commands from standard message events.

It is platform-neutral and must not inspect QQBot raw payloads. It emits command events or handler tasks that business plugins can consume.

Provided task protocols:

- `mutsuki.bot.command/parse@1`

Emitted task protocols:

- `mutsuki.bot.command/handle@1`
