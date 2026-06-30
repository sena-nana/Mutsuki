# Mutsuki

> A domain-neutral single-task runtime kernel implemented as a Rust framework.

**Current boundary: Rust-first single-task runtime kernel**

The root workspace is the Rust framework surface. It provides serializable
runtime contracts, the reusable `CoreRuntime` kernel, and native/JSONL runner
host helpers.

The current Python runner kit lives in
[`python/mutsuki-runtime-python`](python/mutsuki-runtime-python). It mirrors the
Rust contracts and provides `PythonRunnerBackend`, `StdioJsonlBridge`, and a
descriptor-based `PythonResourceManager`.

The runtime shape is:

```text
RuntimeProfile + PluginManifest
  -> RuntimeLoadPlan / RuntimeLock
  -> CoreRuntime
  -> TaskPool + TaskLease + RunnerRegistry + Executor dispatch + ResultRouter
  -> StateStore + ResourceManager / ResourceCell + EventLog + TraceLog
```

## Crates

- `crates/mutsuki-runtime-contracts` - pure serializable contracts:
  Task, TaskLease, Runner, StateDelta, EffectRequest, ValueRef, ResourceRef,
  ResourceCellRef, ResourceLease, PluginManifest, RuntimeLoadPlan,
  ContractSurface, trace, events, and errors.
- `crates/mutsuki-runtime-core` - runtime mechanics:
  CoreRuntime, TaskPool, TaskLease, RunnerRegistry, Executor dispatch,
  ResultRouter, StateStore, ResourceManager, reload surface checks, event log,
  and trace log.
- `crates/mutsuki-runtime-host` - native Rust host helper:
  runtime bootstrapper, deterministic load-plan resolver, and stdio JSONL runner client.
- `python/mutsuki-runtime-python` - optional Python runner kit:
  pure contract mirrors, Python runner backend, stdio JSONL runner server, and
  descriptor-based resource manager.

## Standard Plugin Naming

The first standard plugin batch follows GitHub issue #8:

- distributable plugin packages use `mutsuki-plugin-<domain>-<name>`;
- protocol packages use `mutsuki-protocol-<domain>`;
- standard runtime plugin ids reserve the `mutsuki.std.<domain>.<name>` prefix;
- protocol ids use `mutsuki.<domain>.<action>` and do not include `plugin`.

## Verification

```powershell
cargo fmt --check
cargo test
```

Python runner kit checks live under `python/mutsuki-runtime-python`:

```powershell
uv run ruff check src tests
uv run pyright src tests
uv run pytest
```

## Reading Order

- [AGENTS.md](AGENTS.md)
- [plans/roadmap.md](plans/roadmap.md)
- [plans/architecture.md](plans/architecture.md)
- [plans/engineering.md](plans/engineering.md)
- [plans/contracts.md](plans/contracts.md)

## License

See [LICENSE](LICENSE).
