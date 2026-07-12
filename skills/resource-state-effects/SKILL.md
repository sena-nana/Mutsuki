---
name: resource-state-effects
description: Change MutsukiCore ResourceRef or ValueRef descriptors, leases, ResourceManager, StateStore, EventLog, StateDelta, EffectRequest, provider commands, or commit semantics.
---

# Resource, State And Effects

- Pass descriptors across runtime boundaries; keep bytes, handles and clients behind providers.
- Default shared resources to readonly/sealed and require valid generation plus lease for mutation.
- Route state and event changes through Committer tasks; do not allow plugins to mutate stores directly.
- Turn external side effects into effect tasks handled by effectful runners.
- Make stale refs, expired leases, provider loss and malformed commits fail loud with stable error codes.

Test lifetime, sealing, lease expiry, generation mismatch, commit atomicity and provider failure behavior.
