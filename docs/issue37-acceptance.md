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

The separate system-allocator time lane completed 68 cases and 91 gates with `PASS`. The required
100k-task / 128-runner / 1%-ready / batch-32 sparse case improved from 3951.2 ns/unit to
3132.6 ns/unit (20.72% lower median latency) and from 253087.67 to 319223.65 units/s (26.13% higher
median throughput) relative to the approved same-machine report; it therefore has no regression.

Reproduce the development comparison:

```powershell
cargo run --release -p mutsuki-runtime-benchmarks --features allocation-tracking -- `
  full --lane allocation --warmup 0 --samples 1 --gate none `
  --output target/mutsuki-benchmarks/issue37-allocation.json
```

The retained final report must be generated from a clean pushed code revision before Issue #37 is
closed; the approved baseline itself is not overwritten automatically.

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

This section is intentionally completed only after each repository has passed and its commit is
visible on the remote.

## Core local validation

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | Pass; MSVC emitted its informational import-library linker message |
| `cargo test --workspace` | Pass: benchmark 10, contracts 21, Core 122, Host 99 plus 1 ignored helper, SDK 28, wire 13, all doc tests |
| `cargo metadata --locked --format-version 1` | Pass |
| `cargo bench-full` | Pass: 68 time cases, 91 gates |
| full release allocation lane | Pass: 67 cases; required 256-entry reductions exceed 30% |
