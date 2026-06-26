---
name: mutsuki-plugin-dev-codex-runner
description: Use when implementing, reviewing, or running the dev-only Codex runner bridge for Mutsuki protocol smoke tests.
---

# Mutsuki Dev Codex Runner

Use this skill when Codex is asked to run as a dev-only Mutsuki runner for local protocol smoke tests.

## Required Reading

Before changing behavior, read these repository contracts in order:

1. `plans/roadmap.md`
2. `plans/architecture.md`
3. `plans/engineering.md`
4. `plans/contracts.md`
5. Relevant implementation and tests.

## Runtime Boundary

- Treat Codex as a postponed dev runner, not as a Rust core concept or standard plugin.
- Keep Codex, LLM, ChatCompletion, Yume, IM, MCP, SDK clients, sockets, and product semantics out of Rust `contracts` and `core`.
- Expose test behavior through the current runner JSONL methods: `runner.step`, `runner.cancel`, and `runner.dispose`.
- Use `mutsuki.dev.codex.run` as the protocol id for this dev bridge.
- Return structured runner results; do not call host resources or external SDKs from the runtime core.

## Failure Rules

- Unknown or invalid Codex output must fail loud as `runtime.host_failed`.
- Never swallow malformed JSON or silently return a successful default.
- Deterministic time, IDs, and lease tokens must come from runtime or host injection.
- Do not add Codex-specific fields to Rust contracts for v1.
