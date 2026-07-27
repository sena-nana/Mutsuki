# TauriHost performance model v1

The local entry point is:

```sh
python3 crates/mutsuki-tauri-benchmarks/scripts/run-reference.py \
  --mode reference \
  --output artifacts/performance/issue4-macos-arm64-provisional/report.json
```

It emits `mutsuki.performance.report/v1` with median, p95, p99, MAD, process CPU and RSS evidence.
The task-pump lane retains the 1/100/1000 Waiting-task cases and fails if idle tasks trigger actor,
single-task-status or batch-state polling. Completion notification, burst cancellation, embedded
startup and graceful shutdown are measured separately.

The bridge lane executes JSON request decode, the real embedded Host call, result/event encode and
bounded frontend queue bookkeeping. This is an executable Tauri adapter fixture, not an OS WebView;
the report records that boundary and makes no full WebView latency claim. Payload dimensions are
256 B, 4 KiB and 64 KiB with 1/16/56 concurrent requests and batch sizes 1/32.

The resource lane creates 1 MiB and 64 MiB resources, times open/chunk-read/concurrent-read/preview
and release, and asserts that only `ResourceRef`, preview handles, or bounded 64 KiB chunks cross
JSON. The custom `mutsuki-resource` protocol serves preview bytes outside invoke/event JSON and
preview tokens are revocable.

Public CI runs one correctness smoke. This repository owns and retains its report/analysis history
under `artifacts/performance/`. Baseline promotion belongs to a fixed machine and requires a clean
repository-revision snapshot, matching environment fingerprint and exact-byte approval through the
MutsukiCore performance contract tooling.
