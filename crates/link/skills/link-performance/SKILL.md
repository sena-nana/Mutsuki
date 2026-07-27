---
name: link-performance
description: Benchmark Mutsuki Link latency, throughput, reserved control priority, reconnect, and allocation behavior. Use for Link hot-path changes, performance claims, release gates, or updates to performance reports and baselines.
---

# Link Performance

- Use `crates/link/scripts/run-performance-model.py` as the owner entrypoint.
- Keep workload, seed, transport, warmup, samples, process runs and environment fingerprint in reports.
- Measure control latency while data channels are saturated; throughput alone is not acceptance.
- Keep synthetic loopback claims labelled and do not promote a local run to a fixed-machine baseline.
- Compare against an approved report only when environment and revision snapshot match.
- Preserve raw samples and anomaly analysis with the generated report.

Run correctness tests before the smoke model and report every failed gate or noisy metric.
