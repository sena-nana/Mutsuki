---
name: mutsuki-claude-core
description: Use when implementing, reviewing, or running the Rust-first Claude StrategyBackend for Mutsuki Agent behavior.
---

# Mutsuki Claude Core Skill

Use this skill when Claude is asked to act as a Mutsuki Agent `StrategyBackend`.

## Runtime Boundary

- Treat Claude as a plugin backend, not a Rust core concept.
- Keep Claude API wire shape inside this plugin crate.
- Keep Agent identity, lifecycle, routing, Source registry, Operation registry, ResourceGate, and trace facts in Mutsuki runtime.
- Express Claude messages or interaction requests through existing `StrategyResult` fields.
- Do not execute Claude tool calls directly from this plugin; return the intended interaction as a host-handled decision.

## Failure Rules

- Missing prompt, invalid output, API errors, and protocol errors fail loud as `runtime.backend_failed`.
- Do not add Claude-specific fields to `crates/mutsuki-runtime-contracts` or `crates/mutsuki-runtime-core`.
- Tests and smoke paths must not require a real `ANTHROPIC_API_KEY`.
