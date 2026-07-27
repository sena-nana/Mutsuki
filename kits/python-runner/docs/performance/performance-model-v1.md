# Python Runner performance model v1

`benchmarks/performance_model.py` emits `mutsuki.performance.report/v1` plus a sibling anomaly
analysis. It measures three owner boundaries: codec-only, in-memory backend dispatch and a real
Python process over stdin/stdout. ServiceHost end-to-end latency is intentionally left to the
ServiceHost deployment matrix.

The versioned fixture manifest is `benchmarks/runner-fixtures-v1.json`. Its noop, echo, calibrated
CPU, wait, resource and fault outputs match the Rust builtin/ABI/process fixtures byte-for-byte after
canonical JSON encoding. `benchmarks/fixture_process.py` exposes stable `python-jsonl` and
`python-binary` process entries for external orchestration.

Reference mode covers batch 1/32/256, payload 256 B/4 KiB/64 KiB, the 1 MiB codec stress case,
inflight 1/16/56, calibrated 50 us/1 ms/10 ms execution, cold start, Hello/Ack, warm reuse, cancel,
dispose, crash, stdout/stderr pressure, idle CPU/RSS and repeated-process RSS. Combinations that
exceed the negotiated 4 MiB payload or 8 MiB frame limit are retained as structured policy-rejection
cases instead of being silently dropped.

Codec sampling records median, p95, p99, MAD, raw samples, frame bytes, tracemalloc peak and GC
state. Process cases record the same latency distribution plus applicable CPU/RSS and correctness
counters. Large resource content remains a descriptor: the 1 MiB resource case never places those
bytes in a control frame.

Use explicit repository inputs for complete local revision provenance:

```text
uv run python benchmarks/performance_model.py \
  --mode reference \
  --repository MutsukiCore=/absolute/path/to/MutsukiCore \
  --repository MutsukiServiceHost=/absolute/path/to/MutsukiServiceHost \
  --output /absolute/path/to/python-reference.json
```

Historical same-machine controls under `run_p0.py` and `run_p1.py` remain available. This repository
owns Python JSONL/binary reports and retains them under `artifacts/performance/`; ServiceHost owns the
Rust builtin/ABI/process half of the five-deployment matrix. MutsukiCore's acceptance validator
compares the two owner reports' fixture hashes. This runner never treats a new CI artifact as an
approved baseline.
