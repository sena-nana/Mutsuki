---
name: plugin-loader
description: Change ServiceHost plugin.toml discovery, manifest validation, builtin registries, native or ABI loading, external plugin deployment, RuntimeLoadPlan inputs, or reload orchestration.
---

# Plugin Loader

- Map `plugin.toml` to Core `PluginManifest` without Host-private capability semantics.
- Register builtin capabilities only from real upstream crates; otherwise report unavailable.
- Validate deployment, API version, artifacts, capabilities and secret references before boot.
- Route reload through scan, validate, surface comparison, drain and generation swap.
- Never copy StdPlugins, AgentKit, BotPlugins or business implementations into the loader.
- Keep the builtin registry manifest-only; never attach an arbitrary host-call facade to a domain plugin.

Test discovery, invalid manifests, missing artifacts/capabilities and breaking reload rejection.

Provider ABI lifecycle uses `ExecuteResourceProviderRequest` (opcode 0x300c, `resource.provider.execute`) and a `ResourceProviderResponse` containing result plus invalidations. Wire schema 1.4.0 handshake rejects older revisions; rebuild provider binaries. Native providers migrate to `execute`; no receipt-only fallback. Runner-facing resource replies retain their shapes. Python mirrors DTOs and registry/artifacts, without implementing a provider endpoint.
