---
name: runtime
description: Change ServiceHost Core bootstrap, ServiceRuntimeBuilder, HostServices, event-source lifecycle, service loop, shutdown, drain, or panic boundaries.
---

# Runtime

- Start Core only through published runtime-host/core APIs and a validated RuntimeLoadPlan.
- Own process and EventSource lifecycle, not Core scheduling or domain behavior.
- Make bootstrap failure unavailable/fatal; never report a service healthy without a real Core runtime.
- Stop IPC, EventSources and runners before releasing HostRuntime.
- Keep standard protocol wrappers in StdPlugins and product selection in product/template repositories.
- Components share state only through application services: the provider calls
  `register_application_service`, each consumer calls `require_application_service` and resolves
  lazily through `RunnerFactoryContext::require_service` (`register_scoped_*` or
  `factory_context()`). Never share owner state through `OnceLock`/static slots or rely on
  selection order. Reconfiguring a provider stages its table, co-reloads declared consumers and
  restores the previous table on failure; a missing provider fails boot/reload loudly.
- State several components share without one owner (one database actor) uses
  `ensure_application_service`: the first factory creates it in the Host-owned builtin
  component, later factories and reconfigured components reuse it, and it lives until shutdown.

Test startup failure, real builder assembly, application-service co-reload/rollback, event-source stop and graceful shutdown ordering.
