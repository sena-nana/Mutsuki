# Issue #101 performance gate

Issue #101 retains the deterministic workload and correctness hashes from Issue #100, then adds
architecture cases for runtime batch orchestration, context/session snapshots, shared LSP, Agent
wire/reconnect, distributed placement and checkpoint codec.

The distributed placement case uses a retry-safe task without Host-local `ResourceRef` inputs, so
it measures a genuinely remote-eligible request. Resource-bound tasks are covered separately by
the functional origin-fallback test.

Smoke runs use one process and fixed seed with public network disabled. Reference evidence uses
three fresh processes. Each case records latency distribution, allocation/retained-memory data,
correctness counters and a deterministic hash. A timing-only change cannot make a correctness
failure pass.

Stage artifacts are stored under `artifacts/performance/issue101-stage-*-smoke.json`; the final
reference artifact is `artifacts/performance/issue101-reference.json`. The corresponding
`*-analysis.json` files classify regressions separately from environmental noise.

The release gate requires:

- every correctness counter is zero;
- retained-resource/session growth stays within the declared case budget and LSP workspaces return
  to zero after close;
- every new case has the same correctness hash across reference processes;
- Issue #100 workloads stay within their declared regression budgets;
- any exception names the case, environment and owner instead of weakening the threshold.
