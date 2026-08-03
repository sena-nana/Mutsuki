# Runtime wire performance report for issue #30

This report records the release decision for the runtime wire redesign. The
four checked-in reports were produced from the same clean release build at
commit `f56489ba9fa41bee9742fcb8474a46fcb41f19b8` on macOS/aarch64 with
`rustc 1.97.0` and ten logical CPUs.

## Result

| Phase | Scope | Gates |
| --- | --- | ---: |
| P0 | typed contract and allocation reduction | 16 / 16 passed |
| P1 | multiplexing, cancellation, and concurrency | 5 / 5 passed |
| P2 | MessagePack framing, stdio, and native ABI | 11 / 11 passed |
| P3 | bounded rejection of hostile input | 3 / 3 passed |

The reports are the authoritative machine-readable evidence:

- `artifacts/perf/issue30-final-p0.json`
- `artifacts/perf/issue30-final-p1.json`
- `artifacts/perf/issue30-final-p2.json`
- `artifacts/perf/issue30-final-p3.json`

## Representative measurements

All codec values below are nanoseconds per entry. Frame sizes include the
complete request frame.

| Workload | Binary MessagePack |
| --- | ---: |
| encode, 1 entry | 2,355.08 |
| decode, 1 entry | 6,449.17 |
| encode, 16 entries | 681.61 |
| decode, 16 entries | 2,798.18 |
| encode, 256 entries | 544.53 |
| decode, 256 entries | 2,255.11 |
| encode, 4,096 entries | 549.89 |
| decode, 4,096 entries | 2,340.60 |

At 4,096 entries, the binary frame was 3,223,770 bytes. The measured stdio
round trip was 19,825.09 ns for binary. The native ABI encode path measured
21.81 ns and 56 bytes for binary.

P1 sustained 23,156 requests/s with one request in flight, 33,202 requests/s
with 16 in flight, and 32,160 requests/s with 56 in flight. Cancellation had a
45,959 ns p95 response latency. The benchmark keeps roughly 5,000 operations in
each concurrency case so scheduler noise cannot decide the scaling gate.

P3 rejected 100,000 instances of each hostile input class. Malformed
MessagePack was rejected in 416.34 ns/frame, truncated frames in 8.91 ns/frame,
and oversized prefixes in 3.00 ns/frame. Every result is below the 50,000
ns/frame bound. The length and truncation paths allocate no heap memory.

## Release decision

- Binary MessagePack is the only internal runtime wire codec for stdio and native ABI.
- Latency-sensitive batching should normally stay at or below 256 entries.
  Larger throughput batches are supported when they remain inside the checked
  frame and payload limits.
- Large resources belong behind `ResourceRef` or a streaming capability; they
  must not be copied into a single runtime wire frame.
- Frame length, payload depth, item count, and pending-request limits remain
  mandatory before allocation. Unknown flags, opcodes, message types, duplicate
  responses, and late responses fail the connection structurally.
- Native libraries are loaded off the async executor and selected by the exact
  manifest ABI. ABI v1/v2 share the same binary wire codec and invalid ABI
  declarations do not fall back.

The Rust golden-vector suite covers all 18 opcodes in both directions. The
companion Python kit consumes the same checked-in vectors and must reproduce
the bytes exactly before a release can be activated. Fuzzing completed 10,000
runs without a crash; integration coverage also exercises cancellation,
duplicate and late responses, disconnect cleanup, and concurrent in-flight
requests.
