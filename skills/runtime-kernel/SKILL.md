---
name: runtime-kernel
description: Change MutsukiCore task scheduling, TaskPool, RunnerRegistry, batch execution, executor dispatch, ResultRouter, cancellation, continuation, or trace propagation. Use only for domain-neutral runtime mechanics.
---

# Runtime Kernel

- Keep `TaskPool` the single scheduling fact and Runner the only execution unit.
- Implement execution through batch-first `run_batch`; isolate entry failures and return exactly one completion per entry.
- Preserve lease, generation, ordering, cancellation, trace and correlation invariants through dispatch and routing.
- Inject time, IDs and host services; do not call ambient global sources from deterministic kernel paths.
- Reject unplanned protocols or runners through structured failures instead of fallback dispatch.

Test single and multi-entry batches, partial failure, cancellation, lease expiry and routing generation mismatches.
