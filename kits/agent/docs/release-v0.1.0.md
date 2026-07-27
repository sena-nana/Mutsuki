# AgentKit v0.1 capability baseline

This is the first explicitly scoped AgentKit package set within the Mutsuki `v0.2.0` monorepo
baseline. All packages are released from one tag/commit and the root `Cargo.lock`.

Included capabilities:

- versioned Agent contracts, Rust SDK and profile validation;
- session/turn/context/model/tool/approval/budget runtime;
- Adapter authoring API and OpenAI-compatible reference Adapter;
- Plugin authoring/conformance and shared LSP Plugin;
- in-process, ServiceHost, TauriHost and MutsukiLink assembly;
- database-backed checkpoint/resume and distributed task/sub-agent mapping;
- deterministic smoke/reference performance model.

Excluded product responsibilities:

- Persona and character data;
- UI, diff preview and approval presentation;
- product workspace/conversation context;
- default Provider credentials or mock Provider;
- a bundled database, network daemon or distributed coordinator.

## Version policy

- Breaking contract or Agent wire changes require a new major protocol ID/version.
- Additive serde-default fields are allowed within `v0.1` only when old peers preserve semantics.
- Adapter/Plugin authoring breaks require a crate minor-version boundary and migration note.
- Feature flags may add implementations but never alter the base semantics of an existing contract.
- Provider brand/catalog changes are instance configuration, not public API breaks.

## Release procedure

Run the root locked workspace gates, Agent Host matrix, plugin/adapter conformance, deterministic
smoke and three-process reference benchmark. Validate an isolated clone using only the selected
tag/commit. Then update the root compatibility matrix, tag that exact commit and publish the
performance artifact hashes with release notes.
