---
name: sdk-runner-host
description: Change the Mutsuki Rust SDK, SDK macros, native runner helpers, RuntimeClient, TaskSubmitter, runner adapters, or Binary Runner Link host support. Use for generic authoring and connection surfaces, not product hosting.
---

# SDK And Runner Host

- Wrap existing contracts and runtime operations; do not create a second scheduler or domain runtime.
- Return `TaskHandle` from submissions and preserve cancellation, trace, correlation and generation context.
- Keep macros small and inspectable: typed protocol metadata, descriptors and compile-time validation only.
- Keep native and binary adapters batch-first and wire-compatible with published contracts.
- Leave process supervision, configuration, secrets and lifecycle to MutsukiServiceHost or another product host.

Test SDK ergonomics against real runtime surfaces and run Runner Link conformance for codec or adapter changes.

Provider integration uses `ResourceProviderGateway::execute` (or native async execute) returning explicit result-plus-invalidation outcomes. Existing caller resource replies remain unchanged; Host actor owns lifecycle application. Invalidating providers declare Ordered. Standalone `LocalResourceClient` rejects ordered providers because it has no Core registry; use HostContext resource clients. Native async future construction must not block.
