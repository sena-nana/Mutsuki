# Async resource creation and the synchronous Host bridge (#182)

## Public Rust interface

`AsyncResourceRegistryGateway` is the awaitable Host registry boundary. It opens an existing
`ref_id` and creates blob, COW and capability resources using owned strings and byte vectors,
returning `BoxRuntimeFuture<ResourceRef>`. `HostContext::async_resource_registry()` and
`async_resource_registry_ref()` expose it. `HostContext::new` takes an additional optional
registry argument after the synchronous registry; custom Hosts without an async registry pass
`None`. Migrate Rust construction sites together with the SDK. This is a Rust source change,
not a new wire operation: existing provider request/reply DTOs and runtime-wire 1.4.0 remain.

The caller receives successful creation only after the Core actor registers the descriptor.
Descriptor open is an actor query, requires no provider I/O and needs no executor. A failed
provider result, conflicting owner or duplicate ref is a failure, not a successful create.
Committed invalidations are applied even if creation fails. Registration failure does not
implicitly delete provider storage: providers must retain their documented recovery/retention
policy; a timeout or lost reply must not be interpreted as proof that creation did not occur.

## Dispatch, admission and lifecycle

Both caller styles select exactly one registered provider ID. Native async `execute`, including
Future construction, runs after executor admission under panic isolation. Synchronous
`Offloaded` providers execute on the blocking pool. Explicit `Inline` providers remain on the
actor and must perform only bounded in-memory work. There is no I/O fallback when an executor
is absent or saturated. Async mailbox saturation returns `capability.exhausted` without parking
the calling async worker; executor and ordered-queue byte/count limits also apply.

The executor constructs descriptors and reports outcomes; it never owns a Core handle.
Only the actor validates ownership and applies invalidations, receipts, snapshots and descriptor
registration. It replies after application and retains the provider instance through application.
Ordered lanes stay occupied until actual completion, including after timeout/disconnection;
Concurrent invocations stay retained too. Reload to Ordered fences all older invocations of the
same provider ID. Candidate routes replace active routes only after a successful Core switch;
reload never restores descriptors. Unknown effects from a panic poison the lane until restart.
Shutdown rejects queued work and drains every executing resource invocation. Restoration is
boot-only and precedes the actor loop.

## Safe bridge for existing large-blob consumers

The synchronous Host registry/plan gateway bridges both native async and Offloaded providers
through the same admitted executor and actor completion path. It blocks the calling worker,
not the Core actor. Use it from synchronous Runner or dedicated blocking/EventSource workers;
do not call it from the actor, or use it to block an async executor worker. Await the async
registry/plan gateways from async code. `LocalResourceClient` is standalone and has no Core
registry or ordered lifecycle guarantee; it is not a substitute for a Host client.

The existing synchronous consumers keep their public interfaces:

- `ImageRenderRunner` creates its PNG output and reads image inputs through its injected registry.
- Bilibili and workshop card pipelines create/read cover resources through injected registries;
  Mihuashi reads rendered resources through the same boundary.
- QQ inbound media creates resource descriptors through a `spawn_blocking` hand-off around the
  synchronous Host bridge; outbound media reads descriptors through the injected gateway.

ServiceRuntime's resource facade binds to `host_runtime.host_context()` gateways after assembly,
so these consumers retain the Host bridge without direct access to SQLite or native async
provider maps. SQLite continues to declare Offloaded and Ordered. Size limits must accommodate
an 8 MiB payload plus routing/schema metadata; a byte limit of exactly 8 MiB is insufficient for
such a creation request. Consumer unit/fake tests do not constitute live platform account smoke.

## Verification ownership

Host tests cover all creation kinds and execution modes, admission before provider entry,
invalid descriptor results, native async synchronous bridging, blocked construction and task
progress, timeout/disconnection, queue rejection and panic poisoning. Existing reload tests
cover candidate replacement and Concurrent-to-Ordered fencing. SQLite tests cover real 8 MiB
create/read/restart, retention, staged reload and shutdown drain. Workspace and first-party Bot
acceptance exercise the shared Rust source surface. Python wire conformance must remain green;
there are no new Python DTOs. Report actual commands and limitations separately from this design.
