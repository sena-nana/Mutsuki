---
name: protocol
description: Change Agent protocol DTOs, protocol identifiers, JSON schemas, error codes, manifest providers or consumers, contract surfaces, or wire compatibility.
---

# Agent Protocol

- Keep all Agent wire DTOs and protocol IDs in `mutsuki-agent-contracts`.
- Mirror each callable protocol with typed SDK markers and update schemas plus manifests together.
- Reuse Core task, batch, resource, effect, trace and error semantics.
- Keep DTOs independent of Provider clients, Host services and language objects.
- Version breaking changes and update every implementing Runner in the same change.
- `AgentSkillPolicy.allowlist` is `None` (all discovered), `Some([])` (none), or `Some(ids)`.
  Catalog `discover`/`load` stay unfiltered so editors can list packages. Context build copies
  a non-empty allowlist to `AgentContextBuildRequest.skill_ids` and does not apply the default
  discovery cap to that explicit set. Unspecified `skill_ids` inject at most 16 skills.
- `AgentRuntimeProfile.begin_dialogs` is even-length user/assistant text injected as
  persona context, not concatenated into the system prompt.

Test serialization, validation and manifest surface consistency.
