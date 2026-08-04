# Command Plugin

The command plugin parses commands from standard message events.

It is platform-neutral and must not inspect QQBot raw payloads. It emits command events or handler tasks that business plugins can consume.

The parser runner consumes row-layout `WorkBatch` values and parses every entry independently. A malformed event or invalid command fails only its own `EntryCompletion`; other commands in the batch continue. Emitted handler tasks inherit the active `registry_generation`.

Configured descriptors support command groups, aliases, typed arguments, optional/default and
variadic values, quoted/escaped tokens, structured parse failures and discoverable help. The
emitted `BotCommandEvent` keeps both the canonical `command_path` and `typed_args`; the Bot SDK's
`CommandContext` preserves those fields for business handlers.

Provided task protocols:

- `mutsuki.bot.command/parse@1`

Emitted task protocols:

- `mutsuki.bot.command/handle@1`
