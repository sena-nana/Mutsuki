# Mutsuki Performance Model v1 contracts

This directory is the single authority for the versioned performance report, deterministic
workload, owner repository snapshot and exact-byte baseline approval contracts. It contains
contract tooling only. Every
repository owns and runs the performance tests for its own runtime boundary and retains its report
and analysis under that repository's `artifacts/performance/` directory (`artifacts/perf/` in
MutsukiCore for compatibility with existing evidence).

`repository-snapshot-v1.schema.json` replaces the original central revision-lock concept with an
owner-scoped snapshot. `revision_lock_hash` is retained in report v1 for compatibility and means the
canonical SHA-256 of that report's `repository_revisions` map. The map must contain the owner
repository and only the dependencies actually used by the benchmark run. A dirty entry makes the
report provisional; it cannot be promoted to an approved baseline.

Validate one owner report:

```bash
python scripts/performance/validate_report.py path/to/report.json
```

Create an explicit approval bound to the report's exact bytes, environment and revision snapshot:

```bash
python scripts/performance/approve_baseline.py \
  --report path/to/report.json \
  --approver reviewer \
  --reason "fixed-machine reference" \
  --output path/to/report.approval.json
```

Compare a current report with an approved same-environment baseline:

```bash
python scripts/performance/compare_baseline.py \
  path/to/baseline.json path/to/current.json \
  --approval path/to/baseline.approval.json
```

The cross-owner Epic validator accepts paths to reports but never launches another repository's
benchmark. It checks schema compatibility, owner/suite identity, correctness and the five-deployment
Runner fixture hashes after each owner has produced its own report.

```bash
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
