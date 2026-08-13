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

## P0 activation and cleanup inventory

| Current path | Current owner/cleanup | Classification | Migration requirement |
| --- | --- | --- | --- |
| `RuntimeBootstrapper` runners/handlers/providers | Core registry calls runner/management `dispose` | generation-bound | stage under scope; Core switches/drains; Host scope disposes backend after drain |
| SDK `HostServiceRegistry` | flat frozen map plus manual merge by plugin id | Host-local effect | scoped provider, parent lookup/isolation, dependency facts and owner lease |
| ABI `PluginSession`/dynamic library | per-session flag plus `Drop`; library closes with final `Arc` | backend instance effect | one scope owner, explicit async/bounded dispose, retain library on dirty failure |
| process/Python binary runners | runner/transport-specific shutdown | backend instance effect | same scope lifecycle and conformance as builtin/ABI |
| ServiceHost event sources/watch callbacks | independent supervisor and shutdown token | Host-local async effect | scope-owned registration and bounded child-first shutdown |
| ServiceHost load-plan hooks/observers | manual validate/activate vectors | staged Host-local effect | activation rollback and generation commit ordering under scope transaction |
| WebExtension RPC/event registration | local `Disposable`; current dispose closure does not remove entries | Host-local effect, leak risk | real scope-owned registration lease; disabling removes callbacks deterministically |
| Agent service implementations | each service implements its own `dispose` | backend/service effect | bridge service lifetime to one scope; retain domain-specific drain behavior |
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
