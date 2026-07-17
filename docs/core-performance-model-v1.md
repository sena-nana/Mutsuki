# Core benchmark v2 and Mutsuki Performance Model v1

Core owns runtime-kernel component and stress evidence only. It does not claim ABI, real process,
Python, Tauri WebView, network or distributed performance. Those boundaries belong to their owner
repositories. Core owns only the versioned report/workload/approval contracts and the Epic
acceptance validator; that validator reads reports already produced by owners and never launches
another repository's benchmark.

## Lanes

The headline `time` lane is compiled without `allocation-tracking`; Rust therefore uses the system
allocator and atomic allocation counters cannot perturb latency. The `allocation` lane is compiled
with `allocation-tracking` and runs in a separate process. Its elapsed values are deliberately not
serialized as headline latency. Runtime-wire P0–P3 historical binaries retain their tracking
allocator for diagnostic comparisons and explicitly mark themselves as non-headline allocation
reports.

`cargo bench-reference` performs warmup, repeated samples and independent process rounds. Fixture
construction occurs before each existing case measurement. The merged report retains raw samples
and reports median, p95, p99, MAD, min and max. Whole-process CPU time, peak RSS and context switches
are emitted as `core.system.process`; they include fixture construction and are not attributed to a
single kernel operation.

Benchmark command metadata replaces output and baseline paths with stable `$OUTPUT`, `$BASELINE` and
`$BASELINE_APPROVAL` markers so reports remain portable. After all time and allocation fragments have
been retained, `scripts/run-core-reference.py --reuse-fragments` first verifies the complete expected
fragment set and then rebuilds only the merged report and anomaly analysis.

## Stable case IDs

- `core.idle-runtime`
- `core.schedule.sparse-ready`
- `core.schedule.full-ready`
- `core.task-lifecycle`
- `core.wait-wake`
- `core.deadline-cancel`
- `core.reload`
- `core.resource-plan.none`
- `core.resource-plan.shared-read`
- `core.resource-plan.write-conflict`
- `core.resource-plan.strict-order`
- `core.completion-route`
- `core.host.submit-batch`
- `core.host.task-outcome`
- `core.host.observability-page`
- `core.host.actor-command`

Each dimension set retains the prior internal case ID as `legacy_case_id`, so historical reports can
be mapped without making that implementation-oriented name the public case identity. The existing
100k Task matrix, 8.64 million tick 24-hour equivalent and one-million lifecycle case remain full-mode
coverage gates.

## Gates and baseline approval

Public CI checks correctness, case completeness, bounded history and catastrophic absolute limits.
It cannot substantiate 5–20% changes. A fixed physical reference machine compares an explicitly
approved report using these initial policies:

- median regression greater than 10% and greater than three baseline MADs;
- p99 regression greater than 20%;
- throughput decrease greater than 10%;
- allocation growth greater than 10% and at least 64 bytes per unit;
- peak RSS growth greater than 10%;
- positive retained-memory slope after warmup;
- duplicate commit, stale acceptance, unsafe retry/placement and duplicate execution are zero tolerance.

The release gate requires both `--baseline` and `--baseline-approval`. The approval records the exact
report SHA-256, canonical repository-revision snapshot hash and environment ID. GitHub Actions caches
and newly produced results are never treated as approval. Each owner retains its own report,
analysis, approval and fixed-machine history in that owner repository.

## Anomaly judgment

The reference command writes a sibling `*-analysis.json` file. Structural sample-count errors,
non-finite data or invalid dimensions are classified as benchmark implementation errors. Broad high
MAD across more than 20% of time cases is classified as environmental noise. A small isolated noisy
set is classified as case-specific noise and should trigger fixture/window inspection. Correctness or
bounded-memory violations are classified as framework suspects. No classification automatically
changes the baseline.
