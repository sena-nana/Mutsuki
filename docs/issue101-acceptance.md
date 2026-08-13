# Issue #101 acceptance evidence

Date: 2026-07-27

## Acceptance matrix

| Requirement | Evidence |
| --- | --- |
| Runtime / Adapter / Plugin boundaries | `mutsuki-agent-contracts`, `mutsuki-agent-runtime`, `mutsuki-agent-adapter-*` and `mutsuki-agent-plugin-*`; `check_workspace.py` verifies the monorepo boundary |
| AgentRuntimeProfile | `mutsuki-agent-runtime::profile` tests cover runtime, Adapter, Provider, Plugin, policy and persistence/distribution validation |
| Provider credential isolation | the OpenAI-compatible async batch test proves an unavailable credential fails only its Provider/session entry while another Provider/session completes through the public protocol |
| Reference coding agent | `products/bot/crates/mutsuki-bot/tests/agentkit_issue3_e2e.rs` executes context, Git/LSP-style tools, two approval suspend/resume loops and a final response |
| In-process Core | the public `RuntimeBootstrapper` test completes model → tool → model |
| ServiceHost | `agentkit_issue3_e2e` covers cancellation, stream, budget, errors, session resume and approval |
| TauriHost | runtime-client runner/async-handler factories complete Agent loop before and after generation reload |
| Plugin lifecycle | `mutsuki-agent-plugin-conformance/tests/abi_conformance.rs` proves builtin and real ABI v2 paths share manifest and runner behavior |
| LSP service | `mutsuki-agent-plugin-lsp` shares one workspace service, uses strict document versions and completes a real `rust-analyzer` E2E |
| MutsukiLink | the local IPC test returns a different server PID through `AgentClient`/`AgentLinkServer` |
| Remote and durable | A real Agent subagent traverses Coordinator → wire transport → WorkerEndpoint → remote HostAdapter, including outcome/cancel; tasks with unmaterialized `ResourceRef` or unresolved data-locality inputs fall back to the origin Host; coordinator epoch fencing and database-backed checkpoint reopen tests pass |
| Persona/product boundary | no Persona types or LiliaCode private dependency; the product compilation boundary is documented |
| Production Provider policy | the default gateway test proves no implicit Provider; the production profile rejects test-only instances |
| Release/compatibility | the immutable `v0.1.0` history is preserved; this breaking AgentKit migration targets Mutsuki `v0.2.0` with AgentKit package set `v0.1.0` |

## Functional gates

- Agent owner packages: 62 passed across 43 suites.
- TauriHost: 53 passed.
- ServiceHost reference Agent E2E: 1 passed.
- Root workspace: 938 passed, 3 ignored, 4 filtered across 183 suites.
- Workspace boundary: 121 Rust packages, one root workspace, no internal Git pins.
- Distributed boundary: passed.
- Independent exact working-tree materialization without sibling repositories: locked metadata,
  formatting, workspace/distributed boundaries and template export passed; the full workspace
  passed 938 tests with 3 ignored and 4 filtered across 183 suites.

## Performance gates

`artifacts/performance/issue101-reference.json` contains a three-process deterministic reference run
with 31 cases. Every case passed. The analysis classification is `environmental-noise` because 9 of
31 cases exceeded the MAD ratio warning threshold; it is not `framework-suspect`. All five
correctness counters are zero, including cross-process hash mismatches and public network requests.
Both smoke and reference reports lock revision
`6eb4044589896ebe82500c22d0dabcdcdd652dd6` with `dirty=false`.

| New architecture case | p95 | Correctness hash |
| --- | ---: | --- |
| checkpoint codec, 100 round trips | 431,833 ns | `e24e481f5e78bb94d3027c168b7daea4a9dc92b0de95dd9439a91b6072840e3c` |
| Agent Link query, 100 operations | 653,375 ns | `615055e4328e0f53c1c019fe644bd5a18407b2b643c537414cf553ee576bfa38` |
| distributed placement, 100 tasks | 7,667 ns | `92405ca93c2cfe89b47aa2ddbd57119bc95cd887d278484d865a0874525444e0` |
| shared LSP query, 100 operations | 373,917 ns | `2b93ed4a9c9a86707d8509f7eded62b6245ae2cde1d2bfdfed6755b4cce24cd4` |

Artifact SHA-256:

- reference report: `154382c22f0ee9c90d6c358e7d4e0aec06470d3fe87cd75c65c3b5e35db4770c`;
- reference analysis: `10e3d2394a289c391de1d8a473b527af5965bb21929ff66dc082316fb99301d5`;
- final smoke report: `46b43a7aa68c61e71faca3a9048ac09aa536c359972c636e2d41f76a8aaf4ba9`;
- final smoke analysis: `d97d4efa4bdf762942f33ea6ad1c17d9e586ee0acf5af24823da0b5f36d23d59`.

The root Core smoke benchmark also passed 59/59 gates; its report is
`target/mutsuki-benchmarks/core-smoke-time.json`.

## Release state

The implementation is committed at `6eb4044589896ebe82500c22d0dabcdcdd652dd6`.
Issue #101 and its child issues remain open until the evidence commit is pushed and the final clean
revision is revalidated. The existing `v0.1.0` tag is immutable and does not contain this breaking
migration; the candidate release train is `v0.2.0`.
