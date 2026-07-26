# Issue #43 acceptance

Issue: <https://github.com/sena-nana/MutsukiCore/issues/43>

## Performance question

The deterministic comparison keeps the business work identical: one interactive request must
complete while a 20 ms background blocking operation is already running. The baseline maps every
non-control execution class to one one-thread ExecutionDomain. The candidate maps Cpu/Orchestration
to one interactive thread and Io/Blocking/Script to one background thread.

Command:

```powershell
cargo run -p mutsuki-runtime-benchmarks --release --bin execution_domain_qos -- `
  --samples 30 --background-ms 20 `
  --output artifacts/perf/issue43-execution-domain-qos.json
```

Initial fixed-machine result on 2026-07-26:

| topology | p50 | p95 | p99 | max |
| --- | ---: | ---: | ---: | ---: |
| single shared path | 20.447 ms | 22.045 ms | 22.550 ms | 22.550 ms |
| interactive/background paths | 0.128 ms | 0.377 ms | 0.909 ms | 0.909 ms |

The multi-path p99 was 95.97% lower. The executable gate requires at least 50% and passed.

## Core behavior coverage

- Lane-specific ready indexes/counts and round-robin QoS selection.
- Config-driven ExecutionDomain registry with complete/unique ExecutionClass mapping.
- Domain and lane queue/inflight budgets plus snapshots.
- Separate bounded control/data actor mailboxes with control burst quotas.
- RuntimeGroup lifecycle, aggregate snapshots, shared services, typed domain handles, explicit
  gateway requests, timeout/cancel, idempotency conflict, independent reload and abort.
- Existing single-runtime, inline/replay and legacy compute/blocking defaults remain available.

The final cross-repository revisions and ServiceRuntime/Bot/Lilia product acceptance are appended
only after owner repositories have been pushed and independent-checkout validation has passed.
