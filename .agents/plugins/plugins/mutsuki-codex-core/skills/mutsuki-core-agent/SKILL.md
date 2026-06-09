---
name: mutsuki-core-agent
description: Use when implementing, reviewing, or running MutsukiCore Agent behavior where Codex acts as the Agent StrategyBackend rather than as a normal tool operation.
---

# MutsukiCore Agent Skill

Use this skill when Codex is asked to act as the core decision engine for a Mutsuki Agent.

## Required Reading

Before changing behavior, read these repository contracts in order:

1. `plans/roadmap.md`
2. `plans/architecture.md`
3. `plans/engineering.md`
4. `plans/contracts.md`
5. Relevant implementation and tests.

## Runtime Boundary

- Treat Codex as a `StrategyBackend` for a Mutsuki `AgentSpec`, not as a Rust core concept.
- Keep Agent identity, lifecycle, routing, Source registry, Operation registry, ResourceGate, and trace facts in Mutsuki runtime.
- Keep Codex, LLM, ChatCompletion, Yume, IM, MCP, SDK clients, sockets, and product semantics out of Rust `contracts` and `core`.
- Express Codex decisions only through existing `StrategyResult` fields: `status`, `decision`, `emitted`, and `error`.
- If Codex wants to use a tool, return a structured decision that names the intended Mutsuki Operation. Do not call host resources or external SDKs directly from the runtime core.

## Failure Rules

- Unknown or invalid Codex output must fail loud as `runtime.backend_failed`.
- Never swallow malformed JSON or silently return a successful default.
- Stale Operation keys, unregistered Sources, resource lease mismatches, and quota exhaustion must preserve existing structured error codes.
- Deterministic time, IDs, and lease tokens must come from runtime or host injection.

## Bridge Shape

The bridge should be a Python sidecar/backend layer using the existing stdio JSONL methods:

- `on_awake`
- `on_input`
- `next_step`
- `on_stop`
- `list_operations`
- `list_sources`
- `invoke`
- `resource.*`

Do not add Codex-specific fields to Rust contracts for v1.
