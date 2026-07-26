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

## Bot/Agent business-purpose comparison

The BotTemplate reference uses the owner contracts from MutsukiBotPlugins and MutsukiAgentKit.
Both topologies query the QQ gateway status while Agent session-append and context-build contract
work are saturated. They keep the same protocols, runners, payloads, outputs and two-worker budget;
the candidate assigns one RuntimeDomain/worker to Bot and one to Agent.

The committed 300-sample result was:

| topology | p50 | p95 | p99 | max |
| --- | ---: | ---: | ---: | ---: |
| one shared domain, two workers | 60.181 ms | 82.189 ms | 118.344 ms | 147.529 ms |
| Bot/Agent domains, one worker each | 0.142 ms | 0.214 ms | 0.530 ms | 3.115 ms |

The p99 improvement was 99.55%. Two clean repeats improved p99 by 99.41% and 98.99%. The
remote-only independent checkout improved p99 by 99.69% (68.899 ms to 0.215 ms). All runs passed
the 50% gate.

Command:

```powershell
cargo run --release --locked -p mutsuki-bot-runtime-reference `
  --bin bot-runtime-domain-bench -- `
  --samples 300 --min-background-ms 20 `
  --output artifacts/perf/issue43-bot-runtime-domains.json
```

## LiliaGithub business-purpose comparison

The workspace reference keeps one `github-domain` consistency domain while splitting physical
local-blocking and workspace-CPU paths. Both topologies use the production operation protocols,
Git command/parser, runners, payloads, outputs and two-worker budget. Each sample must finish and
validate a real `git status --porcelain=v1 -z --branch` result while analysis of captured
production file/history snapshots occupies the CPU work.

The p99 metric is submit-to-runner-start latency, which is the ExecutionDomain-controlled
dispatch boundary. Terminal business output remains mandatory; external Git/disk duration is not
misreported as scheduler latency. The committed 300-sample result was:

| topology | p50 | p95 | p99 | max |
| --- | ---: | ---: | ---: | ---: |
| one shared domain, two workers | 336.370 ms | 514.413 ms | 554.196 ms | 644.044 ms |
| local/analysis paths, one worker each | 0.089 ms | 0.979 ms | 69.174 ms | 426.741 ms |

The p99 improvement was 87.52%. The preceding clean run improved p99 by 99.93%, and a second clean
run by 99.79%. A shallow remote-only independent checkout calibrated from its own two-commit
history and improved p99 by 99.75% (332.744 ms to 0.837 ms). All stable-gate runs passed.

Command:

```powershell
cargo run --release --locked --manifest-path src-tauri/Cargo.toml `
  -p lilia_github_workspace_runtime_reference `
  --bin workspace-execution-domain-bench -- `
  --samples 300 --min-background-ms 20 --workspace . `
  --output artifacts/perf/issue43-liliagithub-execution-domains.json
```

## LiliaCode business-purpose comparison

The product reference keeps the protocols, runners, payloads, outputs and total worker budget
identical. Both topologies prepare the production LiliaGithub task-handoff contract while two
production Agent stdin payload builds and one production `git worktree list --porcelain`
inspection are active.

The gate uses 100 paired samples, alternates topology order, reuses one warmed long-lived runtime
per topology and calculates p99 with nearest-rank. The required p99 improvement remains 50%.

Committed fixed-machine result:

| topology | p50 | p95 | p99 | max |
| --- | ---: | ---: | ---: | ---: |
| one shared domain, three workers | 33.650 ms | 44.013 ms | 57.096 ms | 61.097 ms |
| product/agent/workspace domains, one worker each | 0.193 ms | 0.928 ms | 3.444 ms | 29.021 ms |

The product-reference p99 improvement was 93.97%. Two clean repeats improved p99 by 98.05% and
65.91%. A final remote-only independent checkout improved p99 by 91.97% (105.042 ms to 8.431 ms).
All four stable-gate runs passed.

Command:

```powershell
cargo run --release --locked --features runtime-domain-reference `
  --manifest-path apps/desktop/src-tauri/Cargo.toml `
  --bin lilia-runtime-domain-bench -- `
  --samples 100 --min-background-ms 20 --workspace . `
  --output artifacts/perf/issue43-liliacode-runtime-domains.json
```

## Released revision matrix

| repository | Issue #43 revision | responsibility |
| --- | --- | --- |
| MutsukiCore | `4a9789ac7568777d3b2668891563fe8109a6dd67` | domain contract, QoS scheduler, isolated workers, RuntimeGroup and Core benchmark |
| MutsukiServiceHost | `d0a2141932a4d1a6df16157a8895c437b62eda39` | config-driven ServiceRuntime domain assembly |
| MutsukiTauriHost | `55fe422626a20d0091f56483e787be69c6582cc9` | desktop domain snapshots and observation |
| MutsukiStdPlugins | `382b6984129fc09b10cc8e5f3af3649fd353f9ea` | Core contract alignment |
| MutsukiAgentKit | `6a8b6d90f02fa06c68067dd29d795a382ec4b7a0` | Agent protocol/plugin contract alignment |
| MutsukiBotPlugins | `c9152c8e8e05f313343cc836985d118732785374` | Bot/Host integration alignment |
| MutsukiBotTemplate | `e5cb240f88d13994d42f1a213a4327b3140d18a3`, `008c103f6129489210b08388496c1739a64a8d74` | external Bot profile assembly, fixed pins and runnable Bot/Agent reference |
| LiliaGithub | `b8d189c42cf437ff3031da38c6e94e8af9bc7b96`, `811c1cd3249576a3f2a045490d53d6d4595a32f9` | operation routing plus runnable workspace performance gate |
| LiliaCode | `3571cccf03521a6bca817c140ddd64cebbf6a9b8`, `59f90d9c6ea8757d674bc3ce4d8362e5574fcbca` | runnable three-domain reference, real-work benchmark and stable gate |

Every revision above was pushed before downstream pins were accepted. Later unrelated main-branch
commits do not change the pinned Issue #43 ownership boundary.

## Validation

- Every Rust owner/integration repository passed `cargo fmt --check`, `cargo check`,
  `cargo test` and locked metadata validation after its change.
- LiliaGithub passed its full Rust all-target suite, including 278 workspace tests and runtime
  startup with all operation classes routed; its reference added 5 passing tests.
- BotTemplate passed locked check/metadata and the full serial workspace suite, including 13
  product assembly tests; the one real QQ smoke remains intentionally ignored without a secret.
- LiliaCode passed 407 library/application tests, the benchmark percentile test, three
  RuntimeGroup reference tests, 781 desktop frontend tests, Tauri verification and task-handoff
  E2E verification.
- The Core, BotTemplate, LiliaGithub and LiliaCode gates all enforce at least 50% p99 improvement
  without relaxing the baseline or threshold.

## Independent checkout

Fresh remote clones of MutsukiBotTemplate, LiliaGithub and LiliaCode were created under the system
temporary directory with no sibling repositories. In those clones:

- locked metadata resolved every Git dependency from its fixed remote revision;
- BotTemplate and LiliaGithub passed workspace/all-target `cargo check`;
- BotTemplate passed its reference tests and 300-sample gate: 68.899 ms to 0.215 ms p99,
  improvement 99.69%;
- LiliaGithub passed its reference tests and 300-sample gate from a shallow history: 332.744 ms
  to 0.837 ms p99, improvement 99.75%;
- LiliaCode passed all-target feature-enabled `cargo check`, all three RuntimeGroup reference
  tests, and the release 100-sample performance gate;
- remote-only LiliaCode result: single-domain p99 105.042 ms, three-domain p99 8.431 ms,
  improvement 91.97%.

This confirms that, for the same business purpose and the same worker budget, the isolated
multi-path topology provides the expected performance improvement and remains independently
reproducible from pushed revisions.
