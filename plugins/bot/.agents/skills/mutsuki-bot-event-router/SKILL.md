---
name: mutsuki-bot-event-router
description: Maintain the Bot Flow DAG executor. Use when changing `mutsuki-plugin-bot-event-router`, `mutsuki.bot.flow/ingress@1`, node execute, graph-owned match nodes, or revision-pinned fan-out.
---

# Mutsuki Bot Event Router

## Scope

- Own `mutsuki.bot.flow/ingress@1` and `mutsuki.bot.flow.node/execute@1`.
- Execute the single immutable Bot Flow revision from the active ConfigService snapshot.
- Provide graph-owned match nodes (`flow.match/*`) as exact Handler bindings.
- Treat `mutsuki.bot.event/ingest@1` as the Gateway event envelope `protocol_id`, not a
  standalone ingest runner.

## Rules

- Do not put Bot broadcast or fan-out into Mutsuki Core.
- Do not inspect QQBot raw payloads; route the standard Bot event envelope.
- Do not restore subscription/priority/hook routing. Matching, order and branching live only in
  the live graph.
- Pin execution to an immutable graph revision and propagate generation, target, trace and
  correlation into child tasks.

## Validation

Run `cargo fmt --check` and `cargo test` from the repository root after Rust changes.
