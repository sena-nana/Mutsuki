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

## Crate Responsibilities

- `mutsuki-bot-protocol`: pure Bot data contracts.
- `mutsuki-bot-sdk`: author helpers over Bot protocol tasks.
- `mutsuki-plugin-bot-event-router`: event subscription and dispatch.
- `mutsuki-plugin-bot-command`: generic command parsing.
- `mutsuki-plugin-bot-adapter-qqbot`: QQBot platform translation and OpenAPI side effects.

## Deferred Plugins

Session and permission plugins are intentionally not part of the MVP workspace until a concrete behavior path needs them.
