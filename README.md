# Mutsuki

> A domain-neutral TaskPool + Plugin Runner runtime kernel implemented as a Rust framework.

**Current boundary: Rust-first TaskPool runtime kernel**

The root workspace is the Rust framework surface. It provides serializable
runtime contracts, the reusable `CoreRuntime` kernel, and native/JSONL runner
host helpers.

The current Python runner kit lives in
[`python/mutsuki-runtime-python`](python/mutsuki-runtime-python). It mirrors the
Rust contracts and provides `PythonRunnerHost`, `StdioJsonlRunnerServer`, and a
descriptor-based `PythonResourceManager`.

The runtime shape is:

```text
RuntimeProfile + PluginManifest
  -> RuntimeLoadPlan / RuntimeLock
  -> CoreRuntime
  -> TaskPool + RunnerRegistry + RunnerLoop + ResultRouter
  -> StateStore + ResourceManager + EventLog + TraceLog
```

## Crates

- `crates/mutsuki-runtime-contracts` - pure serializable contracts:
  Task, Runner, StateDelta, EffectRequest, ValueRef, ResourceRef, PluginManifest,
  RuntimeLoadPlan, ContractSurface, trace, events, and errors.
- `crates/mutsuki-runtime-core` - runtime mechanics:
  CoreRuntime, TaskPool, RunnerRegistry, RunnerLoop, ResultRouter, StateStore,
  ResourceManager, reload surface checks, event log, and trace log.
- `crates/mutsuki-runtime-host` - native Rust host helper:
  native runner host, deterministic load-plan resolver, and stdio JSONL runner client.
- `python/mutsuki-runtime-python` - optional Python runner kit:
  pure contract mirrors, Python runner host, stdio JSONL runner server, and
  descriptor-based resource manager.

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
