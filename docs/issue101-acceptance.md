# Issue #101 acceptance evidence

Date: 2026-07-27

## Acceptance matrix

| Requirement | Evidence |
| --- | --- |
| Runtime / Adapter / Plugin boundaries | `mutsuki-agent-contracts`, `mutsuki-agent-runtime`, `mutsuki-agent-adapter-*` and `mutsuki-agent-plugin-*`; `check_workspace.py` verifies the monorepo boundary |
| AgentRuntimeProfile | `mutsuki-agent-runtime::profile` tests cover runtime, Adapter, Provider, Plugin, policy and persistence/distribution validation |
| Provider credential isolation | the OpenAI-compatible async batch test proves an unavailable credential fails only its Provider/session entry while another Provider/session completes through the public protocol |
| Reference coding agent | `templates/bot/crates/mutsuki-bot/tests/agentkit_issue3_e2e.rs` executes context, Git/LSP-style tools, two approval suspend/resume loops and a final response |
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
  passed 938 tests with 3 ignored and 4 filtered across 183 suites. The reviewed commit still
  requires a final clean-revision clone run.

## Performance gates

`artifacts/performance/issue101-reference.json` contains a three-process deterministic reference run
with 31 cases. Every case passed. The analysis classification is `environmental-noise` because 9 of
31 cases exceeded the MAD ratio warning threshold; it is not `framework-suspect`. All five
correctness counters are zero, including cross-process hash mismatches and public network requests.

| New architecture case | p95 | Correctness hash |
| --- | ---: | --- |
| checkpoint codec, 100 round trips | 471,958 ns | `e24e481f5e78bb94d3027c168b7daea4a9dc92b0de95dd9439a91b6072840e3c` |
| Agent Link query, 100 operations | 851,458 ns | `615055e4328e0f53c1c019fe644bd5a18407b2b643c537414cf553ee576bfa38` |
| distributed placement, 100 tasks | 9,250 ns | `92405ca93c2cfe89b47aa2ddbd57119bc95cd887d278484d865a0874525444e0` |
| shared LSP query, 100 operations | 262,250 ns | `2b93ed4a9c9a86707d8509f7eded62b6245ae2cde1d2bfdfed6755b4cce24cd4` |

Artifact SHA-256:

- reference report: `243fb73fcd0960c3ba78c8d550ff01cadfad993c09cde8e215dade03d3c6ea90`;
- reference analysis: `4978b6ead58e747faf03f20d362cd38f10508d47e38073908c4d2b6c8e7618b3`;
- final smoke report: `b193feb5831b0fdfb749ec7f06e1b82e7709715869ab52cf4c86aeb367229bfd`;
- final smoke analysis: `7a1444b428f4d1dd0d654d418dc7a74c31782e0edda698d6338bdbc49f5db520`.

The root Core smoke benchmark also passed 59/59 gates; its report is
`target/mutsuki-benchmarks/core-smoke-time.json`.

## Release state

The working tree is ready for review and commit. The repository policy requires user confirmation
before committing. Issue #101 and its child issues must remain open until the reviewed commit is
pushed and the clean revision is revalidated. The existing `v0.1.0` tag is immutable and does not
contain this breaking migration; the candidate release train is `v0.2.0`.
