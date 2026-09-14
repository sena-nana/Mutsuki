---
name: contracts
description: Evolve Mutsuki public runtime DTOs, protocol identifiers, error codes, serialization shapes, and contract documentation. Use before changes that cross crate, host, plugin, runner, language, or process boundaries.
---

# Contracts

Treat `plans/contracts.md` and `mutsuki-runtime-contracts` as the shared wire authority.

- Keep contracts domain-neutral and serializable; never expose pointers, clients, sockets or language objects.
- Preserve batch-first `WorkBatch`/`CompletionBatch`, one completion per entry, and structured failures.
- Use `TaskHandle` for task identity, outcome and cancellation across public APIs.
- Update plans, Rust types, exports, schemas and cross-language mirrors together when the wire shape changes.
- Require explicit version or migration handling for breaking surfaces; do not add compatibility shims in consumers.

Test round trips, invalid inputs and downstream conformance at every affected boundary.

Performance report validation is also a contract boundary: metrics and gate values must be finite
numbers with the declared scalar/distribution shape, and failed reports cannot be approved
baselines. Run `python3 -m unittest discover -s performance/tests` when changing these rules.

Provider ABI lifecycle uses `ExecuteResourceProviderRequest` (opcode 0x300c, `resource.provider.execute`) and a `ResourceProviderResponse` containing result plus invalidations. Wire schema 1.4.0 handshake rejects older revisions; rebuild provider binaries. Native providers migrate to `execute`; no receipt-only fallback. Runner-facing resource replies retain their shapes. Python mirrors DTOs and registry/artifacts, without implementing a provider endpoint.

AsyncResourceRegistryGateway and the optional HostContext constructor argument are Rust source
changes for #182. Migrate construction sites atomically; existing provider DTOs/opcodes and wire
1.4.0 stay unchanged. A successful creation means actor registration completed, not merely that
provider storage was written. See ../../docs/architecture/async-resource-creation.md.
