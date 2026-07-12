---
name: sdk-runner-host
description: Change MutsukiCore Rust SDK, SDK macros, native runner helpers, RuntimeClient, TaskSubmitter, runner adapters, or JSONL Runner Link host support. Use for generic authoring and connection surfaces, not product hosting.
---

# SDK And Runner Host

- Wrap existing contracts and runtime operations; do not create a second scheduler or domain runtime.
- Return `TaskHandle` from submissions and preserve cancellation, trace, correlation and generation context.
- Keep macros small and inspectable: typed protocol metadata, descriptors and compile-time validation only.
- Keep native and JSONL adapters batch-first and wire-compatible with published contracts.
- Leave process supervision, configuration, secrets and lifecycle to ServiceHost or another product host.

Test SDK ergonomics against real runtime surfaces and run Runner Link conformance for codec or adapter changes.
