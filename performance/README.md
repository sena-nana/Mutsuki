# Mutsuki Performance Model v1 contracts

This directory is the single authority for the versioned performance report, deterministic
workload, owner repository snapshot and exact-byte baseline approval contracts. It contains
contract tooling only. Each monorepo area owns and runs the performance tests for its runtime
boundary and retains historical reports under its local `artifacts/performance/` directory.
Ephemeral smoke reports belong under the root `target/mutsuki-benchmarks/`.

`repository-snapshot-v1.schema.json` replaces the original central revision-lock concept with an
owner-scoped snapshot. `revision_lock_hash` is retained in report v1 for compatibility and means the
canonical SHA-256 of that report's `repository_revisions` map. The map must contain the owner
repository and only external dependencies actually used by the benchmark run. Every in-tree owner
records the single `Mutsuki` revision. A dirty entry makes the report provisional; it cannot be
promoted to an approved baseline.

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

The cross-owner Epic validator accepts paths to reports but never launches benchmarks. It checks
schema compatibility, owner/suite identity, correctness and the five-deployment Runner fixture
hashes after each area has produced its own report.

```bash
python scripts/performance/validate_issue35_reports.py \
  --fixture-manifest hosts/service/fixtures/performance/runner-fixtures-v1.json \
  --report core=target/mutsuki-benchmarks/core-smoke-time.json \
  --report service-host=target/mutsuki-benchmarks/service-host-smoke.json \
  --report tauri-host=target/mutsuki-benchmarks/tauri-smoke.json \
  --report link=target/mutsuki-benchmarks/link-smoke.json \
  --report distributed-host=target/mutsuki-benchmarks/distributed-smoke.json \
  --report python-runner-kit=target/mutsuki-benchmarks/python-runner-smoke.json \
  --report std-plugins=target/mutsuki-benchmarks/std-smoke.json \
  --report agent-kit=target/mutsuki-benchmarks/agent-smoke.json \
  --report bot-plugins=target/mutsuki-benchmarks/bot-smoke.json \
  --require-clean
```

Use `--require-clean` only for committed reference evidence. Smoke reports created while developing
a change remain valid schema/correctness evidence but are intentionally marked dirty and cannot be
approved as a release baseline.
