# Agent Plugin authoring

An Agent Plugin contributes one or more public surfaces: tool, context provider, hook, policy,
command or service. Use `mutsuki-agent-plugin-api` descriptors and normal Mutsuki `PluginBuilder`
manifests.

- A tool descriptor points to a real target protocol runner. It never embeds a private callback.
- Context providers return bounded text, metadata or `ResourceRef` snapshots.
- Shared services register through `HostServiceRegistry` with a stable service ID.
- Permissions and side-effect class are declared in descriptors; approval decisions are bound to
  session, turn, action and transcript version.
- Reload follows the Mutsuki generation contract. Factories must create fresh runner/handler
  instances for drain-and-swap.

Use `mutsuki-agent-plugin-conformance` to validate unique surface IDs, lifecycle declarations,
service access and generation behavior. `mutsuki-agent-plugin-lsp` demonstrates a shared,
restartable service used by context and tool surfaces without teaching the Host LSP semantics.

Tests must assert behavior and typed outcomes. Do not assert log strings or provide production
fallbacks that pretend an unavailable service is working.
