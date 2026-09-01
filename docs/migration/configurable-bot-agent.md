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
5. Save the graph with its configuration revision. Saving validates and applies once; the document
   is immediately live. Old `subscriptions`, Handler priority/propagation/hooks,
   command tables and conversation policy rules are rejected or ignored; no automatic import is
   attempted.
6. Recreate Bilibili push delivery as a graph. Bilibili polling no longer sends messages: the
   runner detects fresh items against the durable cursor and submits a
   `mutsuki.bot.event.bilibili` trigger event per item. Wire
   `mutsuki.bot.bilibili.notification` → `mutsuki.bot.bilibili.card` → a platform send node
   (reference graph `bilibili_push_flow()`, one chain for live/dynamic/video kinds); while the
   active graph has no matching Source, trigger events are silently dropped and nothing is
   pushed. Subscription targets and pause state stay in the Bilibili owner config; the subscription `outbound_binding` is carried as
   event context, and the graph's send node binding decides delivery.

Agent owns execution configuration such as session scope, runtime profile, STT/TTS,
concurrency and timeout. Its submit/cancel/reset/fork/status/regenerate nodes emit typed reply
events; reliable Delivery sinks own outbound delivery. Existing session generation fencing,
idempotency and connection handshake guarantees remain unchanged.

An explicitly assembled local Agent engine can additionally edit the graph through an
approval-gated toolset: `bot.flow.read`, `bot.flow.validate` and `bot.flow.apply` target the
same `mutsuki.bot.flow` ConfigService revision-CAS apply as the editor, so the document keeps
one source of truth and one validation path. The tools exist only where the Agent loop runtime
is co-located with the Bot plugins; wire-connected external agents have no such channel, and no
Agent tool exposes plugin reload, deployment or other control-plane commands.

Saving the editor graph performs one ConfigService revision-CAS apply and immediately activates
the snapshot. The repository plugin durably commits the redacted document while the Flow provider
atomically swaps its active snapshot; failures roll both sides back, and in-flight tasks keep
their original graph revision. Unsaved editor changes stay in the current session only. The Flow
editor is an explicitly selected WebExtension and is independent of the Router.
