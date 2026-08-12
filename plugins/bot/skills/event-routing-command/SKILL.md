---
name: event-routing-command
description: Change Bot Flow graph validation/execution, node catalogs, command matching, explicit branching, reply event generation, or business-facing Bot node Runner behavior.
---

# Bot Flow And Command Nodes

- Treat the published Bot Flow revision as the only matching, ordering and branching source.
- Plugins declare `mutsuki.bot.flow.nodes@1` node descriptors, typed ports, config schema and exact
  Handler binding; they never declare commands, subscriptions, priority, propagation or hooks.
- Consume typed `BotFlowEventEnvelope` values and emit named `BotNodeOutput` values; never call a
  platform API outside the node's declared binding.
- Execute every matching flow independently. Multiple outgoing edges are explicit fan-out;
  multiple incoming edges trigger independently; version 1 has no cycles or implicit join.
- Pin execution to an immutable graph revision and propagate generation, target, trace and
  correlation into deterministic child tasks.
- Route structured failures only through explicit error edges; otherwise terminate that branch.
- Keep drafts, immutable graph versions, CAS metadata and audits in the Bot-owned repository.
- Command prefix, path, aliases and typed arguments live only in Command Match node config;
  `matched` and `unmatched` are explicit outputs.

Test validation failures, hit/miss, linear chains, fan-out, simultaneous flows, error edges,
revision pinning, CAS conflicts, restart recovery and typed output shape.
