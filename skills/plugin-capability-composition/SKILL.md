---
name: plugin-capability-composition
description: Change PluginScope, scoped Host services, reversible effects, backend ownership, plugin contribution projection, or staged reload composition.
---

# Plugin Capability Composition

- Preserve `identity != location != generation`, `capability != application`, and
  `plugin != deployment`. Never branch compatibility on an application name.
- Treat a plugin as one ownership and lifecycle unit without splitting a domain transaction merely
  to match UI or deployment boundaries.
- Host owns the scope tree, service dependency graph, Host-local effects and backend instances.
  Core owns deterministic runtime facts, occupancy, generation switch/drain and stale fencing.
- Register every long-lived callback, watcher, transport, process, library and backend through one
  scope owner. Cleanup is async, bounded, reverse-order, child-first and idempotent.
- Keep a failed cleanup as `FailedDirty`; retain the backing instance and diagnostics for retry.
- Declare required/optional services and static/rebindable bindings before activation. Resolve only
  through the plugin scope; missing required services fail activation.
- Prepare a candidate scope before Core reload. Roll it back on pre-switch failure. After a
  successful switch, keep the new generation authoritative and dispose only retired scopes.
- For targeted reload, preserve unaffected scope owners and immediately clean unused candidate
  backends.
- Project application/domain contributions through `RuntimeProfile.supported_extensions` using
  universal, optional and required semantics. The universal business surface must not change when
  an application contribution is omitted.
- Builtin keeps typed in-process service/runner paths. ABI, process, WASM and Python must preserve
  the same business contract and lifecycle semantics without forcing local serialization.

Test activation rollback, reverse cleanup, timeout/dirty retry, dependency loss/rebind, targeted
reload isolation, service projection, 10k repeated lifecycle bounds and cross-deployment behavior.
