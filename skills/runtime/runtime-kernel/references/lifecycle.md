# Lifecycle details

Paths below are relative to the repository root.

## Observability verification

Keep Core's observability harness semantic-only: disabled tracing constructs no span and retains
zero capacity; enabled tracing retains 64 spans. Measure the independent
`core.observability.disabled-trace` case through an opaque receiver in the benchmarks owner.
Optimized time smoke gates p99 <= 1 ms/decision; allocation never gates time. Dev uses `--gate none`.
Require complete current/baseline lane metrics and validated reference fragments for fine regression;
never approve failed reports or count case presence as a metric comparison. Use synthetic gate tests
and the owner commands in `docs/core-performance-model-v1.md`; preserve the fixed-machine evidence boundary.

## Descriptor invalidation (#184)

Provider `execute` returns `ResourceProviderOutcome`: committed invalidations are independent of operation success, including partial batch/saga failure. Host applies provider/ref/resource-generation removals on the actor before replying. Absent refs are idempotent; conflicting owners/generations fail. Invalidation dominates same-outcome updates and removes writer/derived occupancy facts. Receipt status and business JSON are not lifecycle signals.

Invalidating providers declare Ordered. Their provider-id lane survives staged reload, retains the executing provider until actor application, and remains occupied after caller timeout/disconnect until actual completion. Queue count/bytes use Host limits; panic with unknown effects poisons the lane until restart. No permanent tombstone history or I/O in open. SQLite keeps create-before-insert retention and capability exemption.

Host shutdown rejects queued resource work and drains executing ordered operations before releasing the actor, so a timed-out worker cannot mutate storage after same-process restart restoration.

Invoke native async provider execute (including Future construction) only after executor admission and inside panic isolation. Inline provider panics also poison their route without killing the actor. Cache pending request routing/byte accounting, count serialized bytes without buffering a duplicate payload, and avoid rescanning blocked queues until a lane completes. Reload drains arbitrate both control and data mailboxes and use the common lifecycle completion method.

Retain every offloaded/native-async provider invocation through actor result application, including Concurrent providers. A reload to Ordered fences all older invocations for that provider ID until the last result is applied; Concurrent execution otherwise remains parallel. Host shutdown drains all executing resource invocations.

Offloaded and native async resource requests share invocation construction and admission bookkeeping; LocalResourceClient reuses the Host single-provider route check. Reload selects each active route once without cloning and pruning complete provider maps.

## Async creation (#182)

Both registry caller styles reuse provider admission and actor lifecycle completion. Native
async Future construction and Offloaded creation must not run on the actor. Async mailbox
saturation rejects without parking its worker. Test task progress while creation is blocked,
not only descriptor visibility. See docs/architecture/async-resource-creation.md.
