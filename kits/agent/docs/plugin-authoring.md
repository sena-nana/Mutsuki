# Agent Plugin authoring

An Agent Plugin contributes one or more public surfaces: tool, context provider, hook, policy,
command or service. Use `mutsuki-agent-plugin-api` descriptors and normal Mutsuki `PluginBuilder`
manifests.

- A tool descriptor points to a real target protocol runner. It never embeds a private callback.
- Context providers return bounded text, metadata or `ResourceRef` snapshots.
- Shared services use `AgentServiceRunner` with a stable service protocol ID. The owning Mutsuki
  plugin generation is their only lifecycle owner; runner disposal drains and disposes the service.
- Permissions and side-effect class are declared in descriptors; approval decisions are bound to
  session, turn, action and transcript version.
- Reload follows the Mutsuki generation contract. Factories must create fresh runner/handler
  instances for drain-and-swap.

Do not add an Agent-specific service registry, generation counter or cleanup list. Host-only
composition services may use scoped `HostServiceRegistry`, but portable Agent business services
must remain task-protocol surfaces so builtin and ABI/process deployments have the same semantics.

Use `mutsuki-agent-plugin-conformance` to validate unique surface IDs, lifecycle declarations,
service access and generation behavior. `mutsuki-agent-plugin-lsp` demonstrates a shared,
restartable service used by context and tool surfaces without teaching the Host LSP semantics.

Tests must assert behavior and typed outcomes. Do not assert log strings or provide production
fallbacks that pretend an unavailable service is working.
