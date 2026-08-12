# Bot Agent Flow migration

Bot orchestration is now a breaking replacement. The Web Console publishes a versioned Bot Flow;
the published graph is the only source of matching, branching and invocation order.

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
5. Publish the graph after validation. Old `subscriptions`, Handler priority/propagation/hooks,
   command tables and conversation policy rules are rejected or ignored; no automatic import is
   attempted.

Agent owns only execution configuration such as session scope, runtime profile, STT/TTS,
concurrency and timeout. Its submit/cancel/reset/fork/status/regenerate nodes emit typed reply
events; reliable Delivery sinks own outbound delivery. Existing session generation fencing,
idempotency and connection handshake guarantees remain unchanged.

Draft save and publish use revision CAS. Publish persists an immutable version before atomically
switching the active snapshot; in-flight tasks keep their original graph revision. Connection
management stays available as a separate Web page, while trigger-rule pages are removed.
