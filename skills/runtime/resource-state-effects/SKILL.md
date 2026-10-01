---
name: resource-state-effects
description: Change ResourceRef or ValueRef descriptors, leases, ResourceManager, StateStore, EventLog, provider outcomes, effects, or commit semantics.
---

# Resources, State And Effects

Keep descriptors and lifecycle facts on runtime boundaries; keep bytes, handles, clients, and storage behind
providers.

- Default shared resources to readonly/sealed and require valid generation plus lease for mutation.
- Route state and event changes through Committer tasks. Plugins never mutate stores directly.
- Keep history opt-in through `StateHistoryProfile`; stale refs, expired leases, provider loss, and malformed commits fail with stable errors.
- Provider results may include committed invalidations independently of business success. The Host actor applies removals before replying; invalidation dominates same-outcome updates.
- Ordered provider lanes survive reload and caller disconnect. Retain the executing instance until actor application; never restore descriptors during reload.
- Offloaded and native async creation share admission and completion bookkeeping. Registration remains actor-owned and timeout does not undo committed creation.

Test sealing, lease expiry, generation mismatch, commit atomicity, restart restoration, partial invalidation,
provider failure, reload, and shutdown behavior.

For provider lifecycle, async creation, or the related verification gates, read
[the lifecycle reference](references/lifecycle.md).
