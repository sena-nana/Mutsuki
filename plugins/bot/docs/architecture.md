# Architecture

MutsukiBotPlugins sits above Mutsuki Core. It contributes ordinary plugins and protocol crates that a Host may load through a `RuntimeLoadPlan`.

```text
QQBot Gateway
  -> mutsuki-plugin-bot-adapter-qqbot
  -> mutsuki.bot.event/ingest@1
  -> mutsuki-plugin-bot-event-router
  -> business bot plugin
  -> mutsuki.bot.message/send@1
  -> mutsuki-plugin-bot-adapter-qqbot
  -> QQBot OpenAPI
```

Core still sees only tasks, runner descriptors, results, events, resource refs, and effect requests. It does not know Bot, QQBot, commands, sessions, or permissions.

`QqBotPluginBundle` lives in `mutsuki-bot-service-host-integration`, the explicit bridge between
the platform adapter and ServiceHost. It registers the adapter
manifest and recreatable native batch runners with `ServiceRuntimeBuilder`, and
registers `QqGatewayEventSource` as a Host-managed long-lived source. The source
receives its client secret through `HostEventSourceConfig::secret` and can enter
Core only through the injected `TaskSubmitter`; it has no Core internals,
business command parser, or direct runner invocation.
The bundle also registers a domain-neutral ServiceHost health component that
publishes the Gateway connection, identification, heartbeat, ACK, event,
reconnect and last-error snapshot through the standard health control surface.

## Crate Responsibilities

- `mutsuki-bot-protocol`: pure Bot data contracts.
- `mutsuki-bot-sdk`: author helpers over Bot protocol tasks.
- `mutsuki-plugin-bot-event-router`: event subscription and dispatch.
- `mutsuki-plugin-bot-command`: generic command parsing.
- `mutsuki-plugin-bot-adapter-qqbot`: QQBot platform translation and OpenAPI side effects.
- `mutsuki-bot-service-host-integration`: EventSource, health and ServiceRuntime assembly only.
- `examples/bot-echo`: platform-neutral example business plugin over `mutsuki.bot.*` only.

## Deferred Plugins

Session and permission plugins are intentionally not part of the MVP workspace until a concrete behavior path needs them.
