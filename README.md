# Mutsuki

> A domain-neutral batch-first runtime kernel implemented as a Rust framework.

**Current boundary: Rust-first batch-first runtime kernel**

The root workspace is the Rust framework surface. It provides serializable
runtime contracts, the reusable `CoreRuntime` kernel, and native/JSONL runner
host helpers. Language kits, including the Python runner kit, live in separate
repositories and mirror the contracts exposed here.

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
  runtime bootstrapper, deterministic load-plan resolver, stdio JSONL runner client,
  and policy-free process runner transport.
## Standard Plugin Naming

The first standard plugin batch follows GitHub issue #8:

- distributable plugin packages use `mutsuki-plugin-<domain>-<name>`;
- protocol packages use `mutsuki-protocol-<domain>`;
- standard runtime plugin ids reserve the `mutsuki.std.<domain>.<name>` prefix;
- protocol ids use `mutsuki.<domain>.<action>` and do not include `plugin`.

The implementations live in `MutsukiStdPlugins`; Core owns none of these domain protocols or
providers.

## Verification

```powershell
cargo fmt --check
cargo test
```

Python runner kit checks are run in the split `MutsukiPythonRunnerKit`
repository.

## Reading Order

- [AGENTS.md](AGENTS.md)
- [plans/roadmap.md](plans/roadmap.md)
- [plans/architecture.md](plans/architecture.md)
- [plans/engineering.md](plans/engineering.md)
- [plans/contracts.md](plans/contracts.md)

## License

See [LICENSE](LICENSE).
