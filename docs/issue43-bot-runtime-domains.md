# Bot/Agent RuntimeDomain reference

`mutsuki-bot-runtime-reference` is an executable Issue #43 reference profile. It is a separate
workspace crate and is not linked into the production `mutsuki-bot` entry point.

The two-domain topology owns two independent consistency and lifecycle domains:

- `bot-domain`: QQ gateway control and final Bot delivery facts;
- `agent-domain`: Agent session, context, model and tool work.

Both domains share the frozen Host service registry, but do not share Core TaskPool, leases,
ResourceManager or StateStore. Calls use typed `CrossDomainTaskRequest` handles.

The reference imports the protocol IDs and payload types from MutsukiBotPlugins and
MutsukiAgentKit instead of copying those contracts. Its tests prove owner-contract execution,
structured invalid-input failure, identical worker budgets and that aborting `agent-domain` does
not stop `bot-domain`.

## Performance gate

```powershell
cargo run --release --locked -p mutsuki-bot-runtime-reference `
  --bin bot-runtime-domain-bench -- `
  --samples 300 --min-background-ms 20 `
  --output artifacts/perf/issue43-bot-runtime-domains.json
```

The single-domain baseline and bot/agent topology use the same two workers, protocols, runners,
payloads and outputs. Two Agent owner-contract workloads occupy the shared baseline while the
gate measures QQ gateway-status latency. The reference reuses warmed long-lived runtimes,
alternates paired sample order and requires at least 50% nearest-rank p99 improvement from at
least 300 samples. Calibration targets twice the declared 20 ms minimum and accepts an iteration
count only when the fastest of three warmed attempts reaches that target, so both Agent tasks
remain active while the sampler establishes pressure on a preemptive host.
