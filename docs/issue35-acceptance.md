# MutsukiCore Epic #35 acceptance evidence

This document is the requirement-by-requirement audit for Epic #35 and its nine owner issues. The
Epic remains open while any row marked pending lacks authoritative evidence.

## Ownership decision

On 2026-07-17 the user explicitly rejected a separate remote `MutsukiBenchmarks` repository and
required every performance test to live in the repository that owns the measured boundary. The
accepted replacement preserves the technical requirements without a tenth repository:

- MutsukiCore owns the versioned report/workload/approval contracts and a read-only Epic validator;
- ServiceHost owns builtin, ABI and Rust-process deployment fixtures and the canonical executable
  six-fixture manifest;
- PythonRunnerKit mirrors the manifest and owns Python JSONL/binary measurements;
- TauriHost, Link, DistributedHost, StdPlugins, AgentKit and BotPlugins own their own workloads;
- every owner retains reports, anomaly analysis, approvals and reference history in its own
  `artifacts/performance/` directory (`artifacts/perf/` in Core);
- the Epic validator accepts report paths but never launches or owns another repository's test.

There is no central revision-lock file. The original versioned lock requirement is implemented as
`repository-snapshot-v1`, scoped to one owner and its actual dependencies. Report v1's
`revision_lock_hash` remains a compatibility field and is the canonical SHA-256 of that owner
report's `repository_revisions` map.

## Owner issue evidence

| Issue | Owner evidence | Local verification | Status |
| --- | --- | --- | --- |
| MutsukiCore#34 | Core v2 time/allocation lanes, stable case IDs, unified report, explicit approval gate, preserved 100k/24h/1m gates | 129 reference cases; 276 Rust tests passed, 1 ignored; 8 contract tests passed | Pass; Windows approved |
| MutsukiServiceHost#15 | builtin/ABI/Rust process deployments, canonical six-fixture manifest, lifecycle and IPC cases | 16 reference cases; 56 tests passed | Pass; Windows approved |
| MutsukiTauriHost#4 | task pump, executable bridge, 1 MiB/64 MiB ResourceRef streaming and release cases | 36 reference cases; 31 tests passed | Pass; Windows approved |
| MutsukiLink#21 | aligned local/TCP/QUIC dimensions, control/backpressure/mux/latest-only/reconnect cases | 53 reference cases; 100 tests passed | Pass; Windows approved |
| MutsukiDistributedHost#22 | real Controller/Worker/ServiceHost processes, placement, registry, content localization and fault stages | 242 reference cases; 68 tests passed | Pass; Windows approved |
| MutsukiPythonRunnerKit#4 | six fixtures across JSONL/binary, codec/pipe/process layers, 1/16/56 inflight and 1/32/256 batches | 92 reference cases; 81 tests passed; Pyright 0 errors | Pass; Windows approved |
| MutsukiStdPlugins#5 | deterministic workflow/resource/fs/http/sqlite workloads with no public network | 18 reference cases; 35 tests passed, 1 ignored | Pass; Windows approved |
| MutsukiAgentKit#4 | deterministic fake model/tool for single/tool/parallel/session/wait/cancel/failure paths | 27 reference cases; 16 tests passed | Pass; Windows approved |
| MutsukiBotPlugins#10 | fake platform burst/multi-adapter/wait/rate-limit/reconnect/dedup/idle paths | 15 reference cases; 75 tests passed | Pass; Windows approved |

Every owner report validates as `mutsuki.performance.report/v1`, records the owner and actual
dependencies, environment fingerprint and measurement boundary, and passes correctness gates.

## Epic-level acceptance

| Epic #35 requirement | Evidence | Status |
| --- | --- | --- |
| Performance tests have an authoritative home | Nine owner repositories contain their own benchmark code, workflow and artifacts; no central repository is required | Pass locally |
| Version report/workload/repository-snapshot/approval contracts | Four schemas and seven Core contract tests under `performance/` | Pass locally |
| Parse every owner report | `scripts/performance/validate_issue35_reports.py` validates 9 reports and 628 cases | Pass locally |
| Same Runner fixture across five deployments | ServiceHost manifest plus builtin, ABI, Rust process, Python JSONL and Python binary hashes are exact | Pass locally |
| Independent owner benchmarks | Core/Host/Tauri/Link/Distributed/Python and three domain workloads run independently in their owner repositories | Pass locally |
| Fixed macOS ARM64 reference history | Every owner workflow targets `[self-hosted,mutsuki-reference,macOS,ARM64]`; physical Apple M4 provisional reports are retained locally | Pending clean fixed-runner history |
| Fixed Windows x64 reference history | Physical Ryzen 7 5800X Windows x64 run retained in all nine owners; 628 clean cases and nine exact-byte approvals pass aggregate validation | Pass |
| Baseline update requires explicit approval | Core tooling validates exact report bytes, revision snapshot and environment; no workflow auto-promotes output | Pass locally |
| Localize an end-to-end regression by layer | Reports retain independent client/Host/Core/Runner/Link/Distributed boundaries | Pass locally |
| Label every performance claim | All reports contain environment fingerprints and explicit measurement boundaries | Pass locally |

## Owner-local macOS ARM64 baseline

The physical Apple M4 AC run is retained under these owner paths:

- Core: `artifacts/perf/issue35-macos-arm64-provisional/`
- ServiceHost: `artifacts/performance/issue15-macos-arm64-provisional/`
- TauriHost: `artifacts/performance/issue4-macos-arm64-provisional/`
- Link: `artifacts/performance/issue21-macos-arm64-provisional/`
- DistributedHost: `artifacts/performance/issue22-macos-arm64-provisional/`
- PythonRunnerKit: `artifacts/performance/issue4-macos-arm64-provisional/`
- StdPlugins: `artifacts/performance/issue5-macos-arm64-provisional/`
- AgentKit: `artifacts/performance/issue4-macos-arm64-provisional/`
- BotPlugins: `artifacts/performance/issue10-macos-arm64-provisional/`

Together they contain 628 cases with zero correctness failures. Current anomaly classifications are
six `environmental-noise` suites and three `case-specific-noise` suites, with 161 noisy cases. No
regression claim is made without an approved clean same-environment baseline.

Repeatable observations:

- Distributed registry median throughput is about 55k mutations/s for Fast 10k, 83 mutations/s for
  Durable and 63 mutations/s for Critical. Durable/Critical are stable real fsync/replication costs,
  not a harness correctness failure; they remain a framework/storage hotspot candidate.
- 1 GiB cold content localization is about 417 MB/s at concurrency 1, 1.62 GB/s at concurrency 4
  and 1.36 GB/s at concurrency 16. The concurrency-16 drop is repeatable saturation and a
  framework/storage hotspot candidate, not a proven regression.
- ServiceHost startup medians are about 289 ms for ABI, 1.85 ms for builtin and 1.83 ms for the Rust
  process fixture. ABI staging/dynamic loading dominates by design.
- Bot loopback WebSocket idle CPU has medians of 12.37 ms in the retained earlier run, 18.26 ms in
  the owner-local run and 17.94 ms in an immediate independent repeat for a one-second window.
  Allocations remain 715-718, output hashes are identical and all correctness counters are zero.
  The two latest runs reproduce the higher cost, but high MAD and the earlier lower run make this a
  case-specific measurement/environment sensitivity and possible idle-runtime hotspot. It is not a
  test implementation correctness error and is not yet a framework regression.

## Owner-local Windows x64 baseline

On 2026-07-19 all nine owner suites ran on a physical AMD Ryzen 7 5800X Windows x64 machine with
64 GiB RAM, the high-performance power plan and no active hypervisor. Each owner retains
`report.json`, `report-analysis.json` and `report.approval.json` under
`artifacts/performance/reference-windows-x64/` (`artifacts/perf/reference-windows-x64/` in Core).

The Core aggregate validator reports `9 reports, 628 cases, clean_required=true`. All owner
correctness gates pass, the canonical five-deployment Runner fixture hashes match, every recorded
repository revision is clean, and each approval is bound to the exact report bytes, environment ID
and revision snapshot. The reports classify broad timing variance as environmental noise; they do
not make a regression claim without same-machine history.

## Harness errors found and corrected

1. The registry reference matrix originally crossed every large scale with every fsync mode and
   produced a multi-hour redundant workload. The corrected matrix covers 10k in all modes and
   100k/1m in Fast mode.
2. Content samples retained every 1 GiB destination until process exit and exhausted local disk.
   Each sample now removes its destination outside the timed window and asserts cleanup.
3. Content and registry throughput used a generic `units/s`; reports now use `bytes/s` and
   `mutations/s`.
4. Core and Distributed reports retained absolute local paths; command/output metadata now uses
   portable markers or sibling directory names.
5. Extending the ServiceHost ABI fixture initially removed its legacy echo protocol. The fixture now
   preserves that contract while adding six benchmark protocols; the real cdylib and workspace tests
   pass.
6. Windows executable discovery omitted `.exe` for DistributedHost benchmark binaries. The harness
   now resolves platform-specific executable names before launch and supports raw-result resume.
7. PythonRunnerKit's parent wrote every inflight pipe request before reading responses, which could
   deadlock on a full Windows pipe. A concurrent bounded receiver now drains replies and enforces a
   timeout while validating every response.
8. DistributedHost content localization used the system temporary directory, so the 1 GiB x16 case
   could exhaust a smaller system volume. Temporary data now lives beside the configured report on
   the benchmark output filesystem and is still deleted outside the timed window.
9. Core cross-process merging treated observed `iterations` and `units` as case identity, so valid
   pagination-count variation broke aggregation. Those normalization counts are now excluded from
   stable case identity in the Rust and Python comparison paths.
10. Core's generic command helper mapped a successful empty `git status --porcelain` to
    `unavailable`, permanently marking clean repositories dirty. Repository status now uses a
    dedicated fail-closed check with clean/dirty/error regression tests.

## Remaining evidence boundary

Windows x64 is now proven by native compilation, execution, clean reports and approvals. This
Windows machine cannot produce the remaining fixed macOS ARM64 history, and the GitHub organization
currently exposes no online self-hosted reference runner. The provisional Apple M4 observations
above cannot be promoted or rewritten as clean fixed-runner evidence; macOS must be rerun natively.

## Local aggregate validation

Run from MutsukiCore after every owner has produced its own report:

```text
python scripts/performance/validate_issue35_reports.py \
  --fixture-manifest ../MutsukiServiceHost/fixtures/performance/runner-fixtures-v1.json \
  --report core=artifacts/perf/issue35-macos-arm64-provisional/report.json \
  --report service-host=../MutsukiServiceHost/artifacts/performance/issue15-macos-arm64-provisional/report.json \
  --report tauri-host=../MutsukiTauriHost/artifacts/performance/issue4-macos-arm64-provisional/report.json \
  --report link=../MutsukiLink/artifacts/performance/issue21-macos-arm64-provisional/report.json \
  --report distributed-host=../MutsukiDistributedHost/artifacts/performance/issue22-macos-arm64-provisional/report.json \
  --report python-runner-kit=../MutsukiPythonRunnerKit/artifacts/performance/issue4-macos-arm64-provisional/report.json \
  --report std-plugins=../MutsukiStdPlugins/artifacts/performance/issue5-macos-arm64-provisional/report.json \
  --report agent-kit=../MutsukiAgentKit/artifacts/performance/issue4-macos-arm64-provisional/report.json \
  --report bot-plugins=../MutsukiBotPlugins/artifacts/performance/issue10-macos-arm64-provisional/report.json
```

## Required closure sequence

1. Review, commit and push each owner repository after user confirmation.
2. Run the owner workflows on fixed macOS ARM64 and Windows x64 machines and retain their artifacts.
3. Create exact-byte approvals for clean reports inside each owner repository.
4. Run the Core validator with `--require-clean`, compare same-machine history and re-audit every row.
5. Close child issues and Epic #35 only after every pending row is complete.
