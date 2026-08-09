# Configurable Bot Agent migration

The production Bot Agent path is now selected as `mutsuki.plugin.bot.agent`; the former
`QqAiBotPluginBundle` remains an explicit injected integration API for tests and specialized
products, not the template default.

Required migration:

1. Register one shared `AgentConnectionRegistry` in the Agent connection and Bot configured
   catalogs.
2. Select `mutsuki.agent.connections`, `mutsuki.bot.command`, the platform adapter, and
   `mutsuki.plugin.bot.agent` explicitly.
3. Add the required `BotAgentConfig.connection_id`. Endpoint and authentication fields move to
   the connector-owned opaque connection config; only Host secret key references are persisted.
4. Persist conversation admission through `ConversationPolicyRuleUpsert/Delete`. The default
   Product policy has `agent_enabled=false`; enabling the plugin is not itself an admission rule.

Message propagation is deterministic: higher-priority handlers run first; Stop or Consume ends
the pipeline. Continue, an unknown command, or a localized handler failure reaches the final Agent
fallback, where policy and connection health are checked. No implicit Provider or alternate
connection is selected on failure.

Config Web mounts connection and policy pages only when their actual Host services are registered.
Connection candidates are handshaken before generation replacement; rule writes use expected
revision, durable audit entries, and SQLite recovery.
