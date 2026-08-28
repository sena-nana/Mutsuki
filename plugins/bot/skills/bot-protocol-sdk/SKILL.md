---
name: bot-protocol-sdk
description: Change generic mutsuki.bot protocol DTOs, protocol identifiers, message segments, targets, events, operations, Rust SDK helpers, BotContext, MessageBuilder, or task options.
---

# Bot Protocol And SDK

- Keep `mutsuki.bot.*` platform-neutral and serializable; platform extensions stay namespaced and optional.
- Envelope IDs (`event/ingest`, `command/handle`) are not runner protocols. Do not reintroduce
  `EventHandlerSpec` or a permission/session plugin without a concrete behavior path.
- Put wire DTOs in the protocol crate and authoring ergonomics in the SDK crate.
- Keep library-surface crates (`mutsuki-bot-*`) free of `PluginBuilder` manifests; loadable
  plugin surfaces and their node catalogs live in `mutsuki-plugin-bot-*` crates.
- Submit operations through RuntimeClient/TaskSubmitter and return `TaskHandle` semantics.
- Preserve trace, correlation, target binding, cancel policy and registry generation.
- Version breaking wire changes and update manifests, adapters and round-trip tests together.
- Keep Bot Flow node/event/document DTOs in the Bot protocol owner. Plugin node catalogs are
  carried through the domain-neutral `PluginProvides.extensions` contract; Core and Std workflow
  must not interpret Bot graph semantics.

Do not expose platform SDK clients, sockets or Host objects through the Bot API.
