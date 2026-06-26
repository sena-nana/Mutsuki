---
name: mutsuki-plugin-dev-claude-runner
description: Use when implementing, reviewing, or running the dev-only Claude runner bridge for Mutsuki protocol smoke tests.
---

# Mutsuki Dev Claude Runner

Use this skill when Claude-related runner behavior is needed for local Mutsuki smoke tests.

## Runtime Boundary

- Treat Claude as a postponed dev runner, not as a Rust core concept or standard plugin.
- Keep Claude API wire shape inside this plugin crate.
- Keep Claude, LLM, ChatCompletion, Yume, IM, MCP, SDK clients, sockets, and product semantics out of Rust `contracts` and `core`.
- Use `mutsuki.dev.claude.run` as the protocol id for this dev bridge.
- Do not execute Claude tool calls directly from this plugin; return structured runner output for host handling.

## Failure Rules

- Missing prompt, invalid output, API errors, and protocol errors fail loud as `runtime.host_failed`.
- Do not add Claude-specific fields to `crates/mutsuki-runtime-contracts` or `crates/mutsuki-runtime-core`.
- Tests and smoke paths must not require a real `ANTHROPIC_API_KEY`.
