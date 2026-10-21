---
name: sdk-runner-host
description: Change the Mutsuki Rust SDK, SDK macros, native runner helpers, RuntimeClient, task submitters, runner adapters, or generic Runner Link host support.
---

# SDK And Runner Host

Wrap published contracts and runtime operations; do not create a second scheduler or domain runtime.

- Return `TaskHandle` from submissions and preserve cancellation, trace, correlation, lease, and generation context.
- Keep macros small and inspectable: typed metadata, descriptors, and compile-time validation only.
- Keep native and binary adapters batch-first and wire-compatible. Resource clients expose descriptors and gateways, not provider storage.
- Native async future construction must not block. Synchronous Host clients may bridge async providers only on caller workers.
- Never spin or sleep-poll `task_outcome`. Blocking callers use `TaskSubmitter::wait_outcome` /
  `RuntimeClient::wait_outcome` (event-driven on Host actor clients, bounded backoff elsewhere);
  Runners waiting on a child prefer a `TaskAwait` continuation, which frees the worker slot.
- Grouped batch lanes isolate handler panics: the panicking lane fails its remaining entries, while
  entries already completed on any lane keep their results.
- Leave process supervision, configuration, secrets, and product lifecycle to the owning Host.
- The SDK is the only plugin-facing path to kernel types: it re-exports `Runner`, `RunnerContext`, `AsyncBatchHandler`, `AsyncCompletionFuture` and the runtime result/statistics types. Plugins, Kits, Hosts and products must not depend on `mutsuki-runtime-core` (`package-boundaries.toml` enforces it); scheduler-policy types come from `mutsuki-runtime-host`.

Test SDK ergonomics against real runtime surfaces and run Link/ABI conformance for codec or adapter changes.

For provider lifecycle, async creation, or the related verification gates, read
[the lifecycle reference](references/lifecycle.md).
