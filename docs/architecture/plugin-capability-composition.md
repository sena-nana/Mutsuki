# Plugin capability composition

Issue #172 refines the plugin boundaries established by Issues #11 and #13. The current
architecture is authoritative: Host composition owns scopes, dependencies and reversible effects;
Core owns deterministic runtime facts and generation commits. The earlier Core-owned lazy
`PluginManager` proposal is superseded.

## Invariants

```text
identity != location != generation
capability != application
plugin != deployment
plugin != domain entity
```

A plugin is an ownership and lifecycle unit. It may provide universal capabilities, domain
contributions and application contributions under one scope and generation. Applications select a
Profile and project only supported contributions; they do not own plugin identity or lifecycle.
Compatibility is decided from capabilities, versions, permissions, platform and deployment support,
never from an application-name branch.

Plugin boundaries must preserve transactions. Operations that commit one revision, idempotency key
or ACID unit stay in one plugin or share an explicit UnitOfWork. Capability contracts remain
domain-coarse (for example `GitService` or `LspService`) instead of turning every method into RPC.

Builtin, ABI, process, WASM and Python implementations expose the same business surface and pass the
same conformance behavior. Builtin keeps its typed local fast path; conformance does not require
serialization or IPC where no boundary exists.

## Ownership

```text
Host PluginScopeManager
  owns scope tree and service dependency graph
  owns reversible Host effects and backend instances
  stages RuntimeLoadPlan declarations
             |
             v
Core prepared reload
  validates and commits registry generation
  owns occupancy, leases and stale-completion fencing
```

Scope identity and plugin generation are associated but are different facts. A staged scope cannot
publish into the active generation. Parent shutdown proceeds child-first. Activation failure rolls
back completed effects in reverse registration order. Async cleanup is explicit and bounded; `Drop`
is only a fail-safe. Cleanup that cannot be proven safe leaves a retained `FailedDirty` scope and
must not unload its native library or process backend.

Runtime registry declarations are generation-bound effects. A scope may own their staged
declaration, but activation and retirement occur only through `RuntimeLoadPlan` and Core generation
switch/drain. Service availability must never mutate Core registries directly.

## Activation and cleanup inventory

| Path | Scope owner after Issue #172 | Classification | Remaining constraint |
| --- | --- | --- | --- |
| `RuntimeBootstrapper` runners/handlers/providers | staged plugin scope plus Core prepared reload | generation-bound | Core remains the only active-registry authority; dispose only after drain |
| SDK `HostServiceRegistry` | plugin scope provision and dependency graph | Host-local effect | typed APIs plus opaque `HostServiceValue`; no public `Any`/downcast or cross-ABI concrete object |
| ABI `PluginSession`/dynamic library | one backend-instance scope effect | backend instance effect | explicit bounded dispose; retain the session/library after dirty failure |
| process/Python binary runners | one process-management scope effect | backend instance effect | protocol shutdown is best effort; child termination is authoritative and retryable |
| ServiceHost event sources | application-scope effect per source | Host-local async effect | targeted reload retires only affected source effects; dirty owners block replacement |
| ServiceHost load-plan hooks | application-scope effect per hook | staged Host-local effect | activate before publication, retire after switch, report cleanup failure |
| Config providers/watch callbacks | owned provider/subscription leases | Host-local effect | candidate rollback restores the previous owner; stale leases cannot remove a newer owner |
| WebExtension RPC/event registration | Web `DisposableScope` owned by one extension setup | Host-local effect | reverse-order retryable cleanup; setup failure rolls back every registration |
| Agent portable services | generation-bound `AgentServiceRunner` | runner/backend lifetime | service drain/dispose is invoked by Runner disposal; no Agent-specific lifecycle |
| Agent/Bot configured Host services | loaded-plugin or load-plan effect | backend/service effect | domain-specific drain remains in the service; registration lifetime is scope-owned |
| stream/subscription/timer occupancy handles | Core `SurfaceOccupancyHandle` | generation-bound fact | keep in Core; scope waits for zero occupancy, never removes by Drop |

The inventory intentionally excludes business Task side effects. Scope cleanup manages long-lived
Host registrations and instances, not compensation or replay of external business operations.

## Lifecycle and reload

Scopes use explicit states: Created, Resolving, Activating, Active, Suspending, Suspended, Draining,
Disposing, Disposed and FailedDirty. Lifecycle policy may constrain an owner to RequiredBuiltin,
ApplicationLifetime, DrainRequired or RestartRequired; plugin ownership does not imply that every
plugin is hot-unloadable.

Reload prepares a complete staged scope, resolves dependencies, activates staged Host effects,
validates the business surface and prepares Core reload. Before the generation switch, any failure
rolls back the staged scope and leaves the old scope/generation authoritative. After the switch, the
new generation remains authoritative; an old-scope cleanup failure is retained and reported as
FailedDirty rather than rolling business Tasks back or pretending unload succeeded.

## Contribution projection

An extension contribution declares its namespace/version and whether it is required. A Host Profile
selects the namespaces it understands. Unknown optional contributions are omitted from the active
projection, while the universal business surface remains active. Missing required contributions are
resolution errors. One package can therefore be consumed as universal plus contribution A in one
application, universal-only in another, and universal plus contribution B in a third without
changing its package identity or manifest business identity.

## Migration example

An existing plugin that embeds product checks should publish one universal business surface and
typed contribution descriptors instead:

```rust
PluginBuilder::new("example.repository")
    .extension(PluginExtensionDescriptor {
        extension_id: "agent.tools".into(),
        version: 1,
        projection: ExtensionProjection::Optional,
        payload: agent_tools,
    })
    .extension(PluginExtensionDescriptor {
        extension_id: "workbench.panels".into(),
        version: 1,
        projection: ExtensionProjection::Optional,
        payload: workbench_panels,
    });
```

Profile A lists `agent.tools`, Profile B lists no extension namespace, and Profile C lists
`workbench.panels`. All three resolve the same plugin identity, deployment-independent business
surface and transaction boundary. Only the active contribution projection differs. A required
contribution uses `ExtensionProjection::Required` and makes an unsupported Profile fail during
resolution rather than exposing a placeholder feature.

Long-lived resources migrate into the same plugin build as scoped effects:

```rust
PluginBuilder::new("example.repository")
    .host_service("repository.service", repository, "repository.read")
    .host_effect(HostEffectKind::BackendInstance, Box::new(repository_backend));
```

Consumers declare the service in their manifest and use `HostContext::plugin_scope(plugin_id)` plus
`require_service`. They do not retain a global service reference across a generation change.

The step-by-step author migration is in
[`../migration/plugin-scope-effects.md`](../migration/plugin-scope-effects.md).
