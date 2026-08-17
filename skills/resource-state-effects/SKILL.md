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

Test lifetime, sealing, lease expiry, generation mismatch, commit atomicity and provider failure behavior.
