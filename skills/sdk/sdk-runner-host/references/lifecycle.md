# Lifecycle details

Paths below are relative to the repository root.

Provider integration uses `ResourceProviderGateway::execute` (or native async execute) returning explicit result-plus-invalidation outcomes. Existing caller resource replies remain unchanged; Host actor owns lifecycle application. Invalidating providers declare Ordered. Standalone `LocalResourceClient` rejects ordered providers because it has no Core registry; use HostContext resource clients. Native async future construction must not block.

AsyncResourceRegistryGateway exposes owned-input open/blob/COW/capability methods returning
BoxRuntimeFuture<ResourceRef>; HostContext exposes borrowed/Arc accessors. ActorCommandClient
uses nonblocking mailbox admission and oneshot replies. Synchronous Host clients bridge async
providers only from caller workers; async consumers await the async gateways. Document
HostContext::new source migration. See docs/architecture/async-resource-creation.md.

Host ABI fixture tests read Cargo compiler-artifact output instead of assuming ./target/debug;
independent clone validation may use a shared CARGO_TARGET_DIR build cache.
