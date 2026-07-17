# BotPlugins Performance Model v1

This suite implements MutsukiBotPlugins #10 and is the Bot business-layer owner workload for the
MutsukiCore #35 performance model. `benchmarks/workloads-v1.json` fixes the workload schema,
fixture version, seed, network policy, and case dimensions.

## Fixture and measurement boundary

- `mutsuki-bot-testkit` owns the versioned fake platform event/card fixtures. QQ gateway JSON is
  parsed through the public QQ mapper; fixture output is deterministic and does not become a
  production fallback.
- Event cases execute the real BotEventRouter and BotCommandRunner batch paths. Multi-adapter cases
  use 4 and 16 accounts and report minimum/maximum completion fairness.
- Rate-limit handling uses the real QQ OpenAPI transport with a scripted 429 and fixed 1 ms
  `Retry-After`. No public request is permitted.
- Reconnect starts a real ServiceRuntime and the reusable loopback QQ HTTP/WebSocket fake, verifies
  Identify followed by Resume, dispatches two messages, and performs clean shutdown.
- `bot.connection-idle` establishes the same real runtime/WebSocket connection, completes Resume,
  then measures process CPU and allocations only during a 250 ms smoke or 1 s reference idle
  window. Runtime construction and reconnect work remain outside `idle_long_connection_cpu_ns`.
- The long-run case accepts 10,000 events in smoke and 100,000 in reference mode, then proves the
  dedup store remains bounded to 2,048 entries and still suppresses a replay after eviction and
  re-reservation.

Simulated platform delay and Bot orchestration are reported separately. Core scheduling and generic
ServiceHost deployment are not attributed to BotPlugins; only the reconnect and connection-idle
cases intentionally include a real ServiceRuntime deployment.

## Running and repository revision snapshot

```text
python scripts/run-performance-model.py \
  --mode reference \
  --process-runs 3 \
  --repository MutsukiCore=../MutsukiCore \
  --repository MutsukiServiceHost=../MutsukiServiceHost \
  --repository MutsukiStdPlugins=../MutsukiStdPlugins \
  --repository MutsukiAgentKit=../MutsukiAgentKit \
  --output artifacts/performance/issue10-reference.json
```

The command retains every child-process raw report, emits `mutsuki.performance.report/v1`, records
the dirty state and revision of every named repository, and writes a sibling anomaly-analysis file.
Metrics include latency p50/p95/p99/MAD, event throughput, event-to-handler/result, queue depth,
dropped/deferred/retried counts, adapter fairness, duplicate executions, CPU/RSS, allocations,
bounded retention, and idle long-connection CPU.

## Correctness and anomaly attribution

Stable output hashes, duplicate execution, wrong route, unexpected error, and public-network
counters are hard correctness gates. A non-zero counter first requires fixture/harness inspection;
it is not automatically a framework defect. When correctness is clean but the small smoke sample has
a high MAD-to-median ratio, the analysis classifies it as case-specific or environmental noise.
Framework attribution requires a reproducible reference-mode regression on the same environment and
repository revision snapshot after excluding fixture, measurement-boundary, and machine-state
errors.

Reference artifact approval and history comparison across the fixed macOS ARM64 and Windows x64
environments are owned by this repository under `artifacts/performance/`. Exact-byte approval uses
the shared MutsukiCore performance contract and never promotes a newly generated result
automatically.
