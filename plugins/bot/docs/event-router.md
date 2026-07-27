# Event Router

The event router owns `mutsuki.bot.event/ingest@1`.

It receives a standard `BotEvent`, evaluates subscriptions, and emits targeted tasks for business handlers. Core does not perform fan-out.

The runner consumes row-layout `WorkBatch` values and can route multiple events in one batch. Event decode or dispatch failure is recorded only on the corresponding `EntryCompletion`; other entries continue. Emitted handler tasks inherit the active `registry_generation`.

Provided task protocols:

- `mutsuki.bot.event/ingest@1`

Emitted task protocols:

- `mutsuki.bot.event/handle@1`
