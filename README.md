# MutsukiBot

> A domain-neutral Agent runtime kernel implemented as a Rust framework.

**Current boundary: Rust-first runtime kernel**

The root workspace is now the Rust framework surface. It provides serializable
runtime contracts, the reusable `AgentRuntime` kernel, and a native host helper
that can run an Agent loop without Python.

Python code from the earlier framework has been moved to
[`python/reference-mutsukibot`](python/reference-mutsukibot). Treat it as a
reference and migration layer for plugin-host ideas, transport examples, and
Python checks. It is no longer the root runtime implementation, but the name
does not imply the code is deprecated or disposable.

## Crates

- `crates/mutsuki-runtime-contracts` - pure serializable contracts:
  Agent, Envelope, ScopeRule, Operation / Source snapshots, trace, errors, and
  resource descriptors.
- `crates/mutsuki-runtime-core` - runtime mechanics:
  lifecycle, inbox ticks, routing, operation registry, source registry,
  trace bookkeeping, and resource lease governance.
- `crates/mutsuki-runtime-host` - native Rust host helper:
  in-memory operation/source backend for direct framework use and smoke tests.

## Verification

```powershell
cargo test
```

Optional Python reference checks live under `python/reference-mutsukibot` and
should be run from that folder when intentionally working on that layer.

## Reading Order

- [AGENTS.md](AGENTS.md) - project constitution and hard rules
- [plans/roadmap.md](plans/roadmap.md) - current Rust-first target and gates
- [plans/architecture.md](plans/architecture.md) - runtime direction and domain boundaries
- [plans/engineering.md](plans/engineering.md) - workspace layout and implementation rules
- [plans/contracts.md](plans/contracts.md) - internal contract surface
- [plans/rust-python-runtime-boundary.md](plans/rust-python-runtime-boundary.md) - Python reference boundary and optional host rules

## License

See [LICENSE](LICENSE).
