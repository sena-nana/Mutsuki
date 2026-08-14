---
name: load-plan-reload
description: Change Mutsuki plugin manifests, capability resolution, RuntimeProfile, RuntimeLoadPlan, RuntimeLock, registry generations, contract surfaces, or hot-reload compatibility checks.
---

# Load Plan And Reload

- Treat `RuntimeLoadPlan`/`RuntimeLock` as registry authority; reject undeclared runner, task, resource and effect demand.
- Freeze registration at boot. Runtime additions require a new plan and registry generation.
- Compare reload surfaces as Identical, Additive, Deprecated, Removed or Breaking.
- Drain occupancy before removal; require migration, drain or restart for breaking changes.
- Keep discovery and product selection in hosts or templates; Core only validates deterministic plans.
- Resolve from the active capability graph, never an application name. Profiles project supported
  optional extensions without changing universal business identity; unsupported required extensions
  fail resolution.
- Route scope trees, scoped services, backend ownership and cleanup through
  `../plugin-capability-composition/SKILL.md`; stage the candidate scope before generation switch,
  roll it back only pre-switch and never make Core own Host effect leases.

Test deterministic resolution, missing capability, generation transitions, occupied removal and breaking reload rejection.
