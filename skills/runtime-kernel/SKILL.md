---
name: runtime-kernel
description: Change Mutsuki task scheduling, TaskPool, RunnerRegistry, batch execution, executor dispatch, ResultRouter, cancellation, continuation, or trace propagation. Use only for domain-neutral runtime mechanics.
---

# Runtime Kernel

- Keep `TaskPool` the single scheduling fact and Runner the only execution unit.
- Implement execution through batch-first `run_batch`; isolate entry failures and return exactly one completion per entry.
- Preserve lease, generation, ordering, cancellation, trace and correlation invariants through dispatch and routing.
- Inject time, IDs and host services; do not call ambient global sources from deterministic kernel paths.
- Reject unplanned protocols or runners through structured failures instead of fallback dispatch.
- The actor thread is the scheduler. Nothing that blocks on a disk, a socket or a lock may
  run inside it: hand such work to the async executor and apply the result to Core when the
  completion event arrives. Resource providers declare this with
  `ResourceProviderExecution`; anything else added to the actor loop answers to the same rule.

Test single and multi-entry batches, partial failure, cancellation, lease expiry and routing generation mismatches.
