# Epic 3 cross-stage acceptance

This matrix is the final product-level acceptance for the distributed Epic. Source-repository issue
tests remain authoritative for implementation details; the template verifies immutable pins,
assembly boundaries and end-to-end deployment behavior without copying their implementations.

| Invariant | Owner evidence | Template evidence |
| --- | --- | --- |
| Plugin ABI, execute, Runner and ordinary Host APIs do not change. | Core phases 0-2; ServiceHost baseline | Existing ABI/config/QQ smoke suites compile unchanged against updated pins. |
| Old and ineligible plugins remain local. | Core portability default is LocalOnly; DistributedHost placement filters | `local-hard-realtime` and explicit local fallback policy; no cluster types enter plugin catalogs. |
| Sidecar/network failure does not stop local Core/ServiceHost. | DistributedHost external adapter boundary | Template never spawns/supervises sidecar; authenticated health monitoring marks disconnect/recovery, explicit Fast fallback is degraded, and Durable/Critical fallback is rejected. |
| Core hot paths do not run network, consensus or blocking telemetry. | Core #17-20 | Template depends on contracts only; distributed implementation is not linked into ServiceRuntime. |
| Large data does not pass through Leader. | DistributedHost remote execution and HA tests | Every deployment requires separate data endpoint, direct Worker transfer and `leader_proxy = false`. |
| Leader failover preserves valid grants and fences old terms. | DistributedHost HA issue tests | Failure drill and three-voter topology are documented and validated. |
| Worker recovery happens only when safe. | DistributedHost recovery issue tests | Mobility, recovery and effect policies are explicit; unsafe/nonrecoverable examples reject fallback. |
| Remote benefit includes scheduling, transfer, prewarm, interference and risk. | DistributedHost scheduler profitability benchmark/tests | Template pins that scheduler revision and exposes bounded resource budgets. |
| Local realtime work dominates remote background work. | DistributedHost admission/reservation/budget tests | HardRealtime LocalOnly and Background remote examples are distinct; CPU/network/checkpoint budgets are mandatory. |
| Trust/verification remains optional and plugin-neutral. | DistributedHost trust issue tests | Trust is deployment/task policy only; policy examples contain no NodeId or plugin-visible trust level. |

## Required commands

Run from a checkout that has no sibling repositories:

```powershell
cargo metadata --locked
cargo fmt --check
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

The GitHub Ubuntu and macOS jobs must also pass. Product-specific smoke remains:

```powershell
cargo test -p mutsuki-bot --test distribution_config --locked
cargo test -p mutsuki-bot --test qqbot_config_e2e --locked
cargo test -p mutsuki-bot --test unix_product_smoke --locked
```

Final acceptance additionally checks that every source issue linked by Epic #3 is closed, all Cargo
Git dependencies use pushed fixed revisions, `HEAD == origin/main`, and the working tree is clean.
