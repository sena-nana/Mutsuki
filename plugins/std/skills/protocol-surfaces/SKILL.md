---
name: protocol-surfaces
description: Define or change Mutsuki standard config, database, filesystem, HTTP, observe, resource, or workflow protocol DTOs, identifiers, schemas, manifests, and contract surfaces.
---

# Protocol Surfaces

- Keep DTOs serializable, backend-neutral and free of OS or SDK handles.
- Use `mutsuki.<domain>.<action>` protocol IDs and keep schema, manifest provider/consumer declarations and exports aligned.
- Reuse Core contracts for task, resource, effect, trace and error semantics.
- Version breaking wire changes and update every standard plugin consumer in the same change.

Test serialization, validation and manifest surface consistency.
