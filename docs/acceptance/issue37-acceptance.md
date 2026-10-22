# MutsukiCore Issue #37 acceptance evidence

This record maps Issue #37 to the implementation and verification that replaced deep-copy,
JSON-roundtrip and string-prefix classification on runtime hot paths. The external Row payload wire
shape is unchanged.

## Runtime changes

| Requirement | Evidence |
| --- | --- |
| Direct batch lookup | SDK adapters and `NativeRunner` resolve `BatchEntry.payload_index` directly and reject task-id mismatches; no linear task-id search remains. |
| Typed local payload | Core claims share `Arc<Task>` into `LocalTaskPayload`; builtin borrowed handlers receive `&RunnerContext` and `&Task`. Serialization emits the existing Row representation, and deserialization remains wire-backed Row. |
| Bounded task ownership | TaskPool retains the authoritative `Arc<Task>` only for the record lifetime; dispatch holds temporary Arc clones. Compatibility claim APIs explicitly clone owned `Task` values. |
| Frozen descriptor iteration | Registration normalizes accepted protocol IDs and rebuilds one sorted `Arc<[RunnerDescriptor]>`; tick and completion paths borrow that snapshot. |
| Single ready traversal | `RunnerLoad.queued_count` reads protocol/selector/step/generation counters maintained with the ready queues; only an actual claim merges candidate queues. |
| Lazy observation and payload sizing | Event builders are skipped when disabled or rejected by `DropNew`; payload size uses a counting writer rather than a temporary JSON byte vector. |
| Resource-plan construction | Requirement-to-entry indices are built during entry construction and conflict membership uses a hash set, removing reverse scans and linear membership checks. |
| Completion routing | Completion entries are consumed into indexed slots; descriptor and entry-completion deep clones were removed. |
| Explicit protocol semantics | `ProtocolClass` is persisted in resolved manifests/load plans. Purity, execution class, effect capability/permission/surface generation and occupancy use it. Prefixes are limited to canonical validation, legacy import and diagnostics. |

## Performance evidence

The allocation lane was built in release mode with the tracking allocator. The comparison baseline
is the approved `artifacts/perf/reference-windows-x64/report.json` captured on the same Windows x64
owner machine. Allocation counts and bytes are deterministic per-operation counters; RSS is retained
as process-scoped context and is not used to claim the per-case reduction.

| 256-entry case | Approved allocations/unit | Issue #37 allocations/unit | Reduction | Approved bytes/unit | Issue #37 bytes/unit | Reduction |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `core.resource-plan.none` | 91.453125 | 48.398438 | 47.08% | 8679.234375 | 3196.574219 | 63.17% |
| `core.completion-route` | 321.527344 | 7.082031 | 97.80% | 20878.097656 | 2376.273438 | 88.62% |

Both required 256-entry operations exceed the 30% allocation reduction target. The full allocation
matrix contains 67 cases, including `core.local-builtin-dispatch` at batch sizes 1, 16 and 256, and
passes every correctness counter.

The separate system-allocator time lane completed 68 cases and 91 gates with `PASS`. In the retained
clean-revision report, the required 100k-task / 128-runner / 1%-ready / batch-32 sparse case improved
from 3951.2 ns/unit to 3682.8 ns/unit (6.79% lower median latency) and from 253087.67 to
271532.53 units/s (7.29% higher median throughput) relative to the approved same-machine report;
it therefore has no regression.

Reproduce the development comparison:

```powershell
$env:MUTSUKI_BENCH_POWER_MODE = "high-performance"
$env:MUTSUKI_BENCH_VIRTUALIZATION = "bare-metal-hypervisor-absent"
cargo run --release -p mutsuki-runtime-benchmarks --features allocation-tracking -- `
  full --lane allocation --warmup 0 --samples 1 --gate none `
  --output target/mutsuki-benchmarks/issue37-allocation.json
```

The final reports are retained as `artifacts/perf/issue37-final-allocation.json` and
`artifacts/perf/issue37-final-time.json`. They record pushed Core revisions
`8cab16da39abb880d044c0cd089a6f60fa68d26e` and
`bac014b354f08b8220b7df0dfccc14e555eb8597`, respectively, with `dirty=false`. Both reports record
the same environment ID as the approved Windows x64 baseline. The approved baseline was not
overwritten.

## Required repository acceptance order

1. Push MutsukiCore after workspace formatting, checks, tests, metadata and performance gates pass.
2. Update every owner repository that compiles Core manifests or builtin runners; run its AGENTS.md
   validation and push the owner change.
3. Update every Host/application consumer pin only after the owner revision is remotely resolvable;
   validate locked metadata/build/tests and push.
4. Update MutsukiBotTemplate last, then validate in an independent checkout with no sibling path
   dependencies.
5. Add final remote revisions and command results below, push this acceptance update, and only then
   close Issue #37.

## Final cross-repository revisions

All rows below were verified against `refs/heads/main` after their repository-specific acceptance
suite passed.

| Repository | Remote revision | Acceptance result |
| --- | --- | --- |
| MutsukiCore (runtime implementation) | `d4ec2e16db4a473c5fcf3f36a88e2ade09b3f695` | Full workspace formatting, clippy, tests, locked metadata, 68-case time lane and 67-case allocation lane passed |
| MutsukiServiceHost | `9fb03856c95027f849753c3b012e87a52f1598e7` | Full formatting, all-target check, tests and locked metadata passed |
| MutsukiLink | `71d40fd0d1e32fb0ba47bfd04ac94e1da8983679` | Existing stable-ID revision passed full formatting, all-target check, tests and locked metadata; no source change was required |
| MutsukiStdPlugins | `d680004dc81843de1993e765482e5508becda3b9` | Core/ServiceHost pins updated; full local and independent-checkout suites passed |
| MutsukiAgentKit | `5e98ec0ea2ab68ddd91cb7eb50fd1d11e2a343c3` | Core pin updated; full formatting, all-target check, tests and locked metadata passed |
| MutsukiBotPlugins | `e0948702ade35a48da2203d6b5e72ddc9c7bf8ec` | Pins updated; effect protocols classified explicitly; full owner, fake-server, ServiceRuntime and batch suites passed |
| MutsukiDistributedHost | `a308b5758cdc0e3fe9261f55a67cfcf2146e9d5a` | Core/ServiceHost/Link pins and stable Link protocol IDs updated; 68 tests, locked metadata and 75-case real performance smoke passed |
| MutsukiCliHost | `681e4fd61172dcb0d7f33cf9f4e39d05f161085d` | ServiceHost pin updated; formatting, all-target check, tests and locked metadata passed |
| MutsukiTauriHost | `7efe925f3240fb16c9ef2a7e5991512429ba1f97` | Core pin and fixture updated; full local suite and independent all-target check passed |
| MutsukiPythonRunnerKit | `877a09aa780b967f9d9194d90c9163845470c77d` | `ProtocolClass` wire mirror updated; Ruff, Pyright and 82 tests passed locally and independently |
| MutsukiBotTemplate | `31c7fa83bd1a28dfb31f6bb61290573a6eaa306f` | Final pins pushed; release-set, locked metadata, ABI assembly, AgentKit, config, distribution and QQ fake-boundary suites passed locally and independently |

The independent template acceptance cloned `31c7fa83bd1a28dfb31f6bb61290573a6eaa306f`
from the remote into an empty directory, materialized all nine release repositories at their exact
manifest revisions, produced a release report with every repository `ok=true`, and passed
`cargo test --workspace --all-targets --locked`. The credentialed real QQBot smoke remained ignored
by design; its fake-server and lifecycle coverage passed.

## Core local validation

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | Pass; MSVC emitted its informational import-library linker message |
| `cargo test --workspace` | Pass: benchmark 10, contracts 21, Core 122, Host 99 plus 1 ignored helper, SDK 28, wire 13, all doc tests |
| `cargo metadata --locked --format-version 1` | Pass |
| `cargo bench-full --output artifacts/perf/issue37-final-time.json` | Pass: 68 time cases, 91 gates; report revision clean |
| full release allocation lane to `artifacts/perf/issue37-final-allocation.json` | Pass: 67 cases; report revision clean; required 256-entry reductions exceed 30% |
