---
name: load-plan-reload
description: Change plugin manifests, capability resolution, RuntimeProfile, RuntimeLoadPlan, RuntimeLock, registry generations, or hot-reload compatibility.
---

# Load Plan And Reload

Treat `RuntimeLoadPlan` and `RuntimeLock` as the registry authority. Read the owning Host/plugin skills
before changing discovery or assembly.

- Freeze registration at boot; runtime additions require a new plan and registry generation.
- Reject undeclared runner, task, resource, effect, or required extension demand with structured failure.
- Resolve from the capability graph, never from an application name. Preserve universal business identity when optional contributions are absent.
- Classify changes as identical, additive, deprecated, removed, or breaking. Drain occupancy before removal.
- Prepare complete candidate provider routes and scopes before switching Core. Roll back only before the switch; after success the new generation is authoritative.
- Targeted reload preserves unaffected owners. Retain in-flight providers and apply their original-route results through the common actor completion path.

Test deterministic resolution, missing capability, generation transitions, occupied removal, targeted and
full reload, and breaking-change rejection.
