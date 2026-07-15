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

The current runner execution model is deliberately single-instance: one
logical `runner_id` can have at most one active `WorkBatch`. A batch may still
contain multiple entries and use the runner's declared batch-internal entry
parallelism. Configurations that request multiple active batches for one
runner are rejected during startup.

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
cargo metadata --locked --format-version 1
cargo fmt --check
cargo test
bash scripts/check-distributed-boundary.sh
```

Python runner kit checks are run in the split `MutsukiPythonRunnerKit`
repository.

## Performance baseline

Issue #28 provides one command for the complete benchmark matrix and one for
the disaster-regression smoke gate:

```powershell
cargo bench-full
cargo bench-smoke
```

Both commands write structured JSON under `target/mutsuki-benchmarks/`. The
full command covers 1k/10k/100k tasks, 1/16/128 runners, 0/1/50/100% ready
ratios, batch sizes 1/32/256, protocol/hint/continuation routing, bounded
long-running behavior, resource planning, completion routing, and Host-facing
actor APIs. The harness records machine and Rust information, commit, profile,
elapsed time, throughput, allocator counts/bytes, and retained/peak deltas.

The 2026-07-15 local baseline was captured on macOS aarch64 with 10 logical
CPUs and Rust 1.97.0 in the release profile. It measured:

| Case | Current measured baseline |
| --- | ---: |
| Equivalent 24-hour idle run (8.64m ticks) | 20.1 ns/tick |
| 100k tasks / 128 runners / 1% ready / batch 32 | 1.26 us/claimed entry |
| 100k tasks / 16 runners / 100% ready / batch 32 | 22.6 us/claimed entry |
| 256-entry resource-plan construction, no resources | 2.23 us/entry |
| 256-entry completion validation and routing | 8.20 us/entry |
| Host actor statistics round trip | 3.93 us/command |
| 1m task lifecycle retained growth in the second half | 0 bytes |

These values are environment-specific evidence, not universal performance
claims. The checked-in structured comparison points are in
`artifacts/perf/issue28-baseline.json`. CI runs a broad absolute smoke gate;
the scheduled/release workflow uploads every full JSON report and compares
matching cases with the previous saved report using a 3x relative regression
threshold plus explicit noise floors.

## Reading Order

- [AGENTS.md](AGENTS.md)
- [plans/roadmap.md](plans/roadmap.md)
- [plans/architecture.md](plans/architecture.md)
- [plans/engineering.md](plans/engineering.md)
- [plans/contracts.md](plans/contracts.md)

## License

See [LICENSE](LICENSE).
