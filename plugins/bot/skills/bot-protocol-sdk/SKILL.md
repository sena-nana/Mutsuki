---
name: bot-protocol-sdk
description: Change generic mutsuki.bot protocol DTOs, protocol identifiers, message segments, targets, events, operations, Rust SDK helpers, BotContext, MessageBuilder, or task options.
---

# Bot Protocol And SDK

- Keep `mutsuki.bot.*` platform-neutral and serializable; platform extensions stay namespaced and optional.
- Put wire DTOs in the protocol crate and authoring ergonomics in the SDK crate.
- Submit operations through RuntimeClient/TaskSubmitter and return `TaskHandle` semantics.
- Preserve trace, correlation, target binding, cancel policy and registry generation.
- Version breaking wire changes and update manifests, adapters and round-trip tests together.

Do not expose platform SDK clients, sockets or Host objects through the Bot API.
