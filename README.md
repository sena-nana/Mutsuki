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

The new Python backend kit lives in
[`python/mutsuki-runtime-python`](python/mutsuki-runtime-python). It mirrors the
Rust contracts and provides an in-process Python backend host for strategy,
operation, and resource lease experiments. It is not a standalone runtime and
does not depend on the old reference package.

## Crates

- `crates/mutsuki-runtime-contracts` - pure serializable contracts:
  Agent, Envelope, ScopeRule, Operation / Source snapshots, trace, errors, and
  resource descriptors, plus runtime events.
- `crates/mutsuki-runtime-core` - runtime mechanics:
  lifecycle, inbox ticks, routing, operation registry, source registry,
  trace bookkeeping, event stream, election policy, trace closure checks, and
  resource lease governance.
- `crates/mutsuki-runtime-host` - native Rust host helper:
  in-memory operation/source backend for direct framework use, smoke tests, and
  a generic stdio JSONL backend adapter.
- `python/mutsuki-runtime-python` - optional Python backend kit:
  pure contract mirrors, in-process backend host, descriptor-only resource
  backend, stdio JSONL server, and Python test fixtures.

## Verification

```powershell
cargo test
```

Optional Python reference checks live under `python/reference-mutsukibot` and
should be run from that folder when intentionally working on that layer.

Python backend kit checks live under `python/mutsuki-runtime-python`:

```powershell
uv run ruff check src tests
uv run pyright src tests
uv run pytest
```

## Reading Order

- [AGENTS.md](AGENTS.md) - project constitution and hard rules
- [plans/roadmap.md](plans/roadmap.md) - current Rust-first target and gates
- [plans/architecture.md](plans/architecture.md) - runtime direction and domain boundaries
- [plans/engineering.md](plans/engineering.md) - workspace layout and implementation rules
- [plans/contracts.md](plans/contracts.md) - internal contract surface
- [plans/rust-python-runtime-boundary.md](plans/rust-python-runtime-boundary.md) - Python reference boundary and optional host rules
- [plans/python-backend-mvp.md](plans/python-backend-mvp.md) - current Python backend kit MVP

## License

See [LICENSE](LICENSE).
