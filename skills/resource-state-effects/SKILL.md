---
name: resource-state-effects
description: Change Mutsuki ResourceRef or ValueRef descriptors, leases, ResourceManager, StateStore, EventLog, StateDelta, EffectRequest, provider commands, or commit semantics.
---

# Resource, State And Effects

- Pass descriptors across runtime boundaries; keep bytes, handles and clients behind providers.
- Default shared resources to readonly/sealed and require valid generation plus lease for mutation.
- Route state and event changes through Committer tasks; do not allow plugins to mutate stores directly.
- Keep state rollback optional. Enable it only through `StateHistoryProfile`; default capacity is zero.
- Expire retained versions by logical `retain_steps` and `capacity_per_ref`. Missing history must fail loud.
- Turn external side effects into effect tasks handled by effectful runners.
- Make stale refs, expired leases, provider loss and malformed commits fail loud with stable error codes.
- The resource registry is process state. A provider whose storage outlives the process
  reports what it still holds through `ResourceProviderGateway::restore_descriptors`, and
  the Host re-registers those descriptors once while starting the runtime — before the
  actor runs, so nothing observes a half-populated registry. The default returns nothing:
  a provider that dies with the process must not claim persistence.
- Restoration is boot-only. The registry outlives staged reloads, so a reloaded provider
  generation must not re-register descriptors that are already there; a duplicate `ref_id`
  stays a hard failure. Restored ids must therefore never be reused by later allocation.
- Synchronous provider plans run on the Core actor thread, which also schedules every
  task. A provider that blocks on a disk or a socket declares
  `ResourceProviderExecution::Offloaded` so the Host runs it on the async executor
  instead; the default `Inline` belongs to providers whose calls are in-memory work.
  Offloading releases the actor, not the calling worker, and the work is then bound by
  the executor's in-flight limits like any other async resource plan.
- Core state still changes only on the actor: an offloaded plan returns a reply and the
  actor applies it (receipt sync, snapshot sync, descriptor registration) when the
  completion event arrives. A provider must never be handed a Core handle to mutate.

Test lifetime, sealing, lease expiry, generation mismatch, commit atomicity, restart
restoration and provider failure behavior.

## Descriptor invalidation (#184)

Provider `execute` returns `ResourceProviderOutcome`: committed invalidations are independent of operation success, including partial batch/saga failure. Host applies provider/ref/resource-generation removals on the actor before replying. Absent refs are idempotent; conflicting owners/generations fail. Invalidation dominates same-outcome updates and removes writer/derived occupancy facts. Receipt status and business JSON are not lifecycle signals.

Invalidating providers declare Ordered. Their provider-id lane survives staged reload, retains the executing provider until actor application, and remains occupied after caller timeout/disconnect until actual completion. Queue count/bytes use Host limits; panic with unknown effects poisons the lane until restart. No permanent tombstone history or I/O in open. SQLite keeps create-before-insert retention and capability exemption.

Index invalidations only for the current outcome so batch deletion/update conflicts do not require a quadratic scan. No per-deletion index survives application. Verify real instance call/restore counts across reload, not only shared database contents.

## Async registry (#182)

AsyncResourceRegistryGateway uses owned inputs and returns a descriptor only after actor
registration. Descriptor open stays an actor query; all create kinds reuse explicit provider
outcomes. Timeout/disconnection does not undo creation or discard committed lifecycle facts.
Sync Host clients bridge native async and Offloaded providers; LocalResourceClient cannot
replace the Host registry. See ../../docs/architecture/async-resource-creation.md.
