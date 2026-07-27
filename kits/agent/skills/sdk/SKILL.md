---
name: sdk
description: Change Agent Rust SDK clients, typed builders, protocol markers, task helpers, bundle ergonomics, or proc macros such as agent tool metadata generation.
---

# Agent SDK And Macros

- Wrap Mutsuki protocols and RuntimeClient; do not build a separate Agent runtime.
- Prefer typed DTOs and builders over free-form JSON.
- Return and consume `TaskHandle` semantics while preserving trace, correlation and cancellation.
- Keep macro output small: metadata, descriptors and compile-time validation only.
- Check `mutsuki-runtime-sdk` before duplicating a runtime primitive.

Test public SDK flows and macro expansion behavior.
