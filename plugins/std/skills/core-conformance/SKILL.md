---
name: core-conformance
description: Validate standard plugin manifests, batch runners, task routing, ResourceRef behavior, RuntimeLoadPlan surfaces, host assembly, and compatibility within one Mutsuki release revision.
---

# Core Conformance

- Exercise public Core/SDK contracts rather than plugin internals.
- Validate batch-first execution with single, multi-entry and partial failure cases.
- Confirm manifests and RunnerDescriptors stay inside the resolved LoadPlan and registry generation.
- Use test doubles only at external boundaries; never bypass Core routing or replace production capability.
- Verify root Workspace path dependencies and the exported release tag in independent checkouts.

Report the exact Core revision and every executed conformance command.
