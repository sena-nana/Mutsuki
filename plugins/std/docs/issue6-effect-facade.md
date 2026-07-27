# StdPlugins Issue #6 acceptance

## Goal

Reduce Effect-facade derivation cost by sharing payload ownership and skipping
queued diagnostic events when detailed observation is off.

## Changes

- Core `TaskPayload` (`Arc<Value>`) + `Task::derive_with_protocol` at
  `1d42325107a82f98dda3912097c3c0aefd4907ba`
- ServiceHost pin `f98ed2d609d6a650d686a26949dfe556fb4f6fdb`
- New `mutsuki-std-effect` helper with protocol pair tables and observation policy
- fs / http / db facades consume the helper; effect runners keep
  `max_inflight_batches = 1` and blocking/Io isolation metadata

## Performance gate (pure derive microbench)

Command:

```text
cargo run --release -p mutsuki-std-benchmarks --bin effect-derive-bench -- both 30
```

Required case: 256 entries × 64 KiB payload

| mode | allocs/entry | bytes/entry |
| --- | ---: | ---: |
| baseline (deep clone + queued event) | 25.000 | 69135.3 |
| optimized (shared Arc + Quiet) | 9.000 | 1811.3 |
| reduction | 64.0% | 97.4% |

Gate (≥50% allocation reduction): PASS

Raw output: `artifacts/performance/issue6/effect-derive-bench.txt`
