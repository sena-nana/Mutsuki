---
name: core-conformance
description: Validate standard plugin manifests, batch runners, task routing, ResourceRef behavior, RuntimeLoadPlan surfaces, host assembly, and compatibility with pinned MutsukiCore revisions.
---

# Core Conformance

- Exercise public Core/SDK contracts rather than plugin internals.
- Validate batch-first execution with single, multi-entry and partial failure cases.
- Confirm manifests and RunnerDescriptors stay inside the resolved LoadPlan and registry generation.
- Use test doubles only at external boundaries; never bypass Core routing or replace production capability.
- Verify cross-repository dependencies from fixed remote Git revisions in an independent checkout.

Report the exact Core revision and every executed conformance command.
