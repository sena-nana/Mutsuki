---
name: contracts
description: Evolve MutsukiCore public runtime DTOs, protocol identifiers, error codes, serialization shapes, and contract documentation. Use before changes that cross crate, host, plugin, runner, language, or process boundaries.
---

# Contracts

Treat `plans/contracts.md` and `mutsuki-runtime-contracts` as the shared wire authority.

- Keep contracts domain-neutral and serializable; never expose pointers, clients, sockets or language objects.
- Preserve batch-first `WorkBatch`/`CompletionBatch`, one completion per entry, and structured failures.
- Use `TaskHandle` for task identity, outcome and cancellation across public APIs.
- Update plans, Rust types, exports, schemas and cross-language mirrors together when the wire shape changes.
- Require explicit version or migration handling for breaking surfaces; do not add compatibility shims in consumers.

Test round trips, invalid inputs and downstream conformance at every affected boundary.
