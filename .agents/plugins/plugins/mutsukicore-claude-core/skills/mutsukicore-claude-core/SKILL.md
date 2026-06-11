---
name: mutsukicore-claude-core
description: Use when implementing, reviewing, or running the Rust-first Claude StrategyBackend for MutsukiCore Agent behavior.
---

# MutsukiCore Claude Core Skill

Use this skill when Claude is asked to act as a MutsukiCore Agent `StrategyBackend`.

## Runtime Boundary

- Treat Claude as a plugin backend, not a Rust core concept.
- Keep Claude API wire shape inside this plugin crate.
- Keep Agent identity, lifecycle, routing, Source registry, Operation registry, ResourceGate, and trace facts in MutsukiCore runtime.
- Express Claude messages or interaction requests through existing `StrategyResult` fields.
- Do not execute Claude tool calls directly from this plugin; return the intended interaction as a host-handled decision.

## Failure Rules

- Missing prompt, invalid output, API errors, and protocol errors fail loud as `runtime.backend_failed`.
- Do not add Claude-specific fields to `crates/mutsukicore-runtime-contracts` or `crates/mutsukicore-runtime-core`.
- Tests and smoke paths must not require a real `ANTHROPIC_API_KEY`.
