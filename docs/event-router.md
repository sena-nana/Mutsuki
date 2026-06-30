# Event Router

The event router owns `mutsuki.bot.event/ingest@1`.

It receives a standard `BotEvent`, evaluates subscriptions, and emits targeted tasks for business handlers. Core does not perform fan-out.
