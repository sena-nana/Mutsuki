---
name: protocol
description: Evolve Mutsuki public DTOs, protocol identifiers, error codes, serialization, and cross-language contract mirrors.
---

# Protocol And Contracts

Use this skill for any boundary between crates, hosts, plugins, runners, languages, or processes.
Read `plans/contracts.md`, the owning contract crate, and the closest scoped `AGENTS.md` first.

- Keep contracts domain-neutral, serializable, versioned, and free of clients, sockets, pointers, or storage handles.
- Preserve batch-first `WorkBatch`/`CompletionBatch`, one completion per entry, `TaskHandle`, and structured failures.
- Change Rust types, exports, schemas, Python/TypeScript mirrors, compatibility notes, and fixtures together.
- Keep protocol IDs stable and use an explicit migration for breaking changes; consumers must not add shims.
- Resource provider execution returns an explicit result plus invalidations. Host actor ownership stays outside the wire DTO.
- Async resource creation reports success only after actor registration; it does not change existing provider opcodes or wire versions.

Test round trips, invalid input, version negotiation, and the affected downstream conformance suites. For
performance report contracts, reject missing, non-finite, malformed, or failed metrics.

For provider lifecycle, async creation, or the related verification gates, read
[the lifecycle reference](references/lifecycle.md).
