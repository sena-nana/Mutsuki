# Bot Agent Flow migration

Bot orchestration is now a breaking replacement. Bot Flow is an ordinary versioned configuration
provider; its active document is the only source of matching, branching and invocation order.

Required migration:

1. Register one shared `AgentConnectionRegistry` in the Agent connection and Bot configured
   catalogs.
2. Select the platform adapter, `mutsuki.bot.flow`, `mutsuki.bot.command`,
   `mutsuki.plugin.bot.agent`, and reliable delivery explicitly.
3. Add the required `BotAgentConfig.connection_id`. Endpoint and authentication fields remain in
   connector-owned opaque connection config; only Host secret key references are persisted.
4. Recreate admission and routing in the Flow editor. Mention, wake word, account, role,
   permission and rate-limit conditions belong to Match nodes. Command prefix, path, aliases and
   typed arguments belong to Command Match node configuration.
5. Validate and apply the graph with its configuration revision. Old `subscriptions`, Handler priority/propagation/hooks,
   command tables and conversation policy rules are rejected or ignored; no automatic import is
   attempted.

Agent owns only execution configuration such as session scope, runtime profile, STT/TTS,
concurrency and timeout. Its submit/cancel/reset/fork/status/regenerate nodes emit typed reply
events; reliable Delivery sinks own outbound delivery. Existing session generation fencing,
idempotency and connection handshake guarantees remain unchanged.

The editor keeps drafts in the browser and performs one ConfigService revision-CAS apply. The
repository plugin durably commits the redacted document while the Flow provider atomically swaps
its active snapshot; failures roll both sides back, and in-flight tasks keep their original graph
revision. The Flow editor is an explicitly selected WebExtension and is independent of the Router.
