# QQBot Adapter

The QQBot adapter is a platform adapter, not a business bot plugin.

It owns:

- Gateway frame handling and reconnect/heartbeat actions.
- Raw QQBot event to standard `BotEvent` mapping.
- Standard Bot message/media/recall tasks to QQBot OpenAPI requests.
- QQBot-specific raw calls.
- Redaction for errors and returned evidence.

It does not own:

- Business commands.
- Business sessions.
- Permission policy.
- Host startup.
- Core scheduling.
