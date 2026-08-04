# Event Router

The event router owns `mutsuki.bot.event/ingest@1`.

It receives a standard `BotEvent`, evaluates subscriptions, and emits targeted tasks for business handlers. Core does not perform fan-out.

The runner consumes row-layout `WorkBatch` values and can route multiple events in one batch. Event decode or dispatch failure is recorded only on the corresponding `EntryCompletion`; other entries continue. Emitted handler tasks inherit the active `registry_generation`.

The Handler pipeline evaluates descriptors in descending priority with a stable handler-id tie
break. It composes built-in filters and asynchronous custom predicates, then applies permission
and token-bucket checks before invoking a handler. Duplicate claims and per-handler concurrency
are ledger-backed; handler, hook, timeout and cancellation failures are recorded for that entry
and do not prevent lower-priority handlers from running. Propagation is explicit through
`continue`, `stop` and `consume` outcomes.

Provided task protocols:

- `mutsuki.bot.event/ingest@1`

Emitted task protocols:

- `mutsuki.bot.event/handle@1`
