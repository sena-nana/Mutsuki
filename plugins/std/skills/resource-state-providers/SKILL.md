---
name: resource-state-providers
description: Implement or change standard memory, shared-memory, database, state, blob, lease, or ResourceRef provider plugins and their persistence semantics.
---

# Resource And State Providers

- Keep real bytes, mappings and connections inside the provider; expose only descriptors across runtime boundaries.
- Enforce resource generation, version, lifetime, sealing and lease rules from Core contracts.
- Route persistent state changes through declared command/commit tasks instead of direct plugin mutation.
- Make provider loss, stale refs and invalid leases structured failures.
- The persistent `mutsuki.std.resource.sqlite` provider stores blob/COW/capability
  bytes and versions in a product-supplied SQLite file; reopening the same path must
  restore descriptors, versions and bytes, and stale writes keep failing with
  `resource.generation_mismatch`. One-shot outputs are cleaned through the
  capability `delete` command, not by silent eviction.
- Persistence is only real once the descriptors come back: implement
  `restore_descriptors` so the Host can re-register stored rows into the resource
  registry at boot, and return descriptors at the versions the rows currently hold.
  Read `length(bytes)` for the size hint rather than the blob itself.
- A store whose rows are disposable carries explicit bounds instead of growing without
  limit. `mutsuki.std.resource.sqlite` takes an optional `retention` (`max_age_seconds`,
  `max_total_bytes`), reclaims on create so no timer thread is needed, and never
  reclaims capability resources because those are handles rather than payloads. The
  deployment that owns the file sets the policy; the provider only supplies the
  mechanism and defaults to unbounded.
- Commits are compare-and-swap against the stored version, not last-writer-wins: a
  provider-held mutex only orders writers inside one process, so the write predicate
  carries the base version and a zero-row update is `resource.generation_mismatch`.
- `idempotency_key` is not a receipt id unless the provider actually stores receipts;
  `mutsuki.std.resource.sqlite` echoes it and does not deduplicate.
- Resource ids are allocated by the store, never by a provider-memory counter: a
  `ref_id` must stay monotonic and must never be reissued after a delete, a reopen,
  or while two provider generations share the same backing file during staged reload.
- A provider that blocks declares `ResourceProviderExecution::Offloaded`; leaving the
  default `Inline` on a disk- or socket-backed provider stalls the whole runtime for the
  length of every call. `mutsuki.std.resource.sqlite` is offloaded, the memory and
  shared-memory providers stay inline.
- SQLite-backed providers configure the connection the same way `mutsuki-bot-state-db`
  does: `busy_timeout`, prefer `journal_mode=WAL` with a recorded fallback for
  in-memory or shared-memory-less file systems, then `synchronous=NORMAL`. Schema
  changes go through `PRAGMA user_version` migrations, never through
  `CREATE TABLE IF NOT EXISTS` alone.

Test create/read/update, sealing, lease expiry, restart persistence and invalid descriptor behavior.
