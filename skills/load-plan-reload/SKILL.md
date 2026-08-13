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
- Route scope trees, scoped services, backend ownership and cleanup through
  `../plugin-capability-composition/SKILL.md`; Core must not own Host effect leases.

Test deterministic resolution, missing capability, generation transitions, occupied removal and breaking reload rejection.
