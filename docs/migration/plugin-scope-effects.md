# Migrating plugin lifetime to scope-owned effects

Issue #172 replaces plugin-owned cleanup lists with one Host-owned lifecycle. This guide applies to
builtin plugins and to Host adapters for ABI, process and Python deployments. WASM must use the same
bridge when an executable WASM backend is introduced; the current repository has no active WASM
execution adapter, so it must not advertise a lifecycle that does not exist.

## 1. Classify the object

- Runner, handler, protocol and resource-provider declarations are generation-bound. Put them in
  `PluginBuilder`; never remove an active Core entry from `Drop`.
- A watcher, callback, route, event source, connection, process or native session is a reversible
  Host effect. Implement `HostEffect` and register it with the plugin.
- A business-side effect belongs to a task/effect runner, not to plugin cleanup. Do not replay or
  compensate business tasks during reload.

## 2. Give a long-lived object one owner

```rust
use mutsuki_runtime_core::RuntimeResult;
use mutsuki_runtime_sdk::{HostEffect, HostEffectFuture, HostEffectKind, PluginBuilder};

struct RepositoryBackend {
    // process, subscription or native session handles
}

impl HostEffect for RepositoryBackend {
    fn dispose(&mut self) -> HostEffectFuture<'_> {
        Box::pin(async move {
            // Stop intake, drain with the backend's bounded policy, then release.
            // Return Err when safe cleanup cannot be proven; the scope will stay FailedDirty.
            Ok(())
        })
    }
}

fn plugin(backend: RepositoryBackend) -> RuntimeResult<mutsuki_runtime_sdk::LoadedPlugin> {
    Ok(PluginBuilder::new("example.repository")
        .host_effect(HostEffectKind::BackendInstance, Box::new(backend))
        .build())
}
```

Do not also store the same backend in a plugin `Vec<cleanup_fn>`. `Drop` may perform a synchronous
last-resort stop, but it cannot be the primary owner of async drain or native-library safety.

Objects created after boot use `HostRuntime::attach_plugin_effect`. Product-lifetime registrations
that intentionally survive a plugin-only reload use `attach_application_effect`; they must still be
retired explicitly when their owning product/configuration is removed.

## 3. Declare services and dependencies before activation

Publish Host-only typed services through `PluginBuilder::host_service` or
`rebindable_host_service`. Consumers declare `SurfaceRequirement::service` in their manifest and
resolve from `HostContext::plugin_scope(plugin_id)`. Required dependencies fail activation;
optional dependencies may be absent. Only services explicitly declared rebindable may suspend and
reactivate a dependent scope after availability changes.

Concrete `Arc<T>` services are an in-process Host composition facility. ABI, process, Python and
future WASM plugins use stable Task/Resource/capability contracts instead of receiving a concrete
Host object.

Agent domain services that must work across deployments use `AgentServiceRunner`. Register that
runner in the ordinary `PluginBuilder`; its service ID is the task protocol and its disposal first
drains, then disposes, the implementation. Do not register the same service in an Agent-only
registry or keep a parallel cleanup list.

## 4. Let reload own ordering

Prepare the complete candidate first. Candidate effects activate while their scope is staged. A
pre-switch failure rolls them back in reverse order and leaves the old generation active. After a
successful Core switch, only the retired scope drains and disposes. A cleanup failure keeps the new
generation authoritative and retains the old owner as `FailedDirty`; do not unload or kill through
a second manual path.

Config providers created for a candidate scope use `register_provider_staged`, while immediately
active owners use `register_provider_owned`; config watch listeners keep the returned
`ConfigWatchSubscription`. ServiceHost load-plan hooks and event sources are attached as application
effects. Web extensions receive registries wrapped by `DisposableScope`, so every setup-time
registration is rolled back even when setup throws.

## 5. Verify the migration

At minimum, exercise:

1. activation failure after each registration step and reverse rollback;
2. repeated and failed dispose, including a successful retry without repeating completed cleanup;
3. targeted reload preserving unaffected plugin and RuntimeDomain scopes;
4. required service loss/recovery, optional absence, inheritance and isolation;
5. process/native dirty cleanup retaining the backing instance;
6. the same business behavior through every active deployment backend.

Do not claim WASM conformance until a real WASM execution adapter exists. Do not add a placeholder
adapter merely to satisfy the deployment matrix.
