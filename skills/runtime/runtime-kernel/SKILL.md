---
name: runtime-kernel
description: Change domain-neutral scheduling, TaskPool, batch runners, dispatch, routing, cancellation, continuation, or trace propagation.
---

# Runtime Kernel

Use only for domain-neutral runtime mechanics. The actor is the scheduler and the runtime is the single
source of task, runner, lease, generation, ordering, cancellation, trace, and correlation facts.

- Execute through batch-first `run_batch`; isolate entry failures and return exactly one completion per entry.
- Reject undeclared protocols and runners with structured failures. Never add fallback dispatch.
- Inject time, IDs, and host services into deterministic paths; do not read ambient globals.
- Nothing that blocks on disk, sockets, or locks runs on the actor. Offloaded/native async work is admitted first and its result is applied back on the actor.
- Retain in-flight provider calls through actor result application across timeout, reload, and shutdown. Ordered lanes fence older results; concurrent lanes remain parallel.
- Keep observability semantic: disabled tracing allocates no span capacity, enabled tracing retains the configured bounded history. Time and allocation gates are separate.

Test single and multi-entry batches, partial failure, cancellation, lease/generation mismatch, ordered
reload fencing, shutdown drain, and the applicable performance case.

For provider lifecycle, async creation, or the related verification gates, read
[the lifecycle reference](references/lifecycle.md).
