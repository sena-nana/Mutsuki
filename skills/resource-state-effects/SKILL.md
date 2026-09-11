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

Test lifetime, sealing, lease expiry, generation mismatch, commit atomicity, restart
restoration and provider failure behavior.
