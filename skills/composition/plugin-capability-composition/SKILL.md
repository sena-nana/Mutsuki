---
name: plugin-capability-composition
description: Change PluginScope, scoped Host services, reversible effects, backend ownership, contribution projection, or staged reload composition.
---

# Plugin Capability Composition

Keep identity, location, and generation distinct; keep capability and application distinct; keep plugin and
deployment distinct.

- Host owns scope trees, service dependency graphs, Host-local effects, and backend instances. Core owns deterministic runtime facts, generation switches, drains, and stale fencing.
- Register every long-lived callback, watcher, transport, process, library, and backend under one scope owner. Cleanup is bounded, reverse-order, child-first, idempotent, and retryable as `FailedDirty`.
- Declare required/optional services and static/rebindable bindings before activation; missing required services fail activation.
- Prepare a candidate scope before reload and retire old scopes only after the new generation is authoritative. Targeted reload must preserve unaffected owners.
- Project application contributions through universal, optional, and required extensions; never branch compatibility on product names.
- Builtin, ABI, process, Python, and future WASM backends keep the same contract and lifecycle semantics.

Test activation rollback, cleanup retry, dependency loss/rebind, targeted reload isolation, contribution
projection, repeated lifecycle bounds, and cross-deployment behavior.
