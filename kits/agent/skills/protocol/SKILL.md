---
name: protocol
description: Change Agent protocol DTOs, protocol identifiers, JSON schemas, error codes, manifest providers or consumers, contract surfaces, or wire compatibility.
---

# Agent Protocol

- Keep all Agent wire DTOs and protocol IDs in `mutsuki-agent-protocol`.
- Mirror each callable protocol with typed SDK markers and update schemas plus manifests together.
- Reuse Core task, batch, resource, effect, trace and error semantics.
- Keep DTOs independent of Provider clients, Host services and language objects.
- Version breaking changes and update every implementing Runner in the same change.

Test serialization, validation and manifest surface consistency.
