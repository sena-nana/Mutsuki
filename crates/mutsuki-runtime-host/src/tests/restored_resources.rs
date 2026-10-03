use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mutsuki_runtime_contracts::resource::experimental::{CommandBatch, SagaPlan};
use mutsuki_runtime_contracts::*;
use mutsuki_runtime_core::{RuntimeFailure, RuntimeResult};
use mutsuki_runtime_sdk::{
    AsyncResourceProviderGateway, ResourcePlanGateway, ResourceProviderExecution,
    ResourceProviderGateway,
};

use crate::{RuntimeBootstrapper, runner_manifest};

use super::helpers::runtime_profile;

const PROVIDER_ID: &str = "fake.persistent";

/// Stands in for a provider whose rows outlive the process: it reports the
/// descriptors it "already had" instead of creating them at runtime.
struct PersistentProvider {
    stored: Vec<ResourceRef>,
    owner_id: String,
}

#[allow(clippy::unused_self, clippy::unnecessary_wraps)]
impl PersistentProvider {
    fn descriptor(ref_id: &str, provider_id: &str) -> ResourceRef {
        ResourceRef {
            ref_id: ref_id.into(),
            resource_id: ResourceId {
                kind_id: "fixture.blob".into(),
                slot_id: ref_id.into(),
                generation: 1,
                version: 3,
            },
            semantic: ResourceSemantic::FrozenValue,
            provider_id: provider_id.into(),
            resource_kind: "fixture.blob".into(),
            schema: "fixture.v1".into(),
            version: 3,
            generation: 1,
            access: ResourceAccess::ProviderRpc {
                provider_id: provider_id.into(),
                method: "restore".into(),
            },
            size_hint: Some(7),
            content_hash: None,
            lifetime: ResourceLifetime::Persistent,
            lease: None,
            seal_state: ResourceSealState::Sealed,
        }
    }

    fn unsupported(route: &str) -> RuntimeFailure {
        RuntimeFailure::new(RuntimeError::new(
            ERR_RESOURCE_UNSUPPORTED,
            "runtime.resource_provider.fake",
            route,
        ))
    }
}

impl ResourcePlanGateway for PersistentProvider {
    fn collect_read_plan(&self, _plan: &ReadPlan) -> RuntimeResult<Vec<u8>> {
        Ok(b"restored".to_vec())
    }

    fn snapshot_read_plan(
        &self,
        _plan: &ReadPlan,
        _kind_id: &str,
        _schema: &str,
    ) -> RuntimeResult<SnapshotDescriptor> {
        Err(Self::unsupported("snapshot"))
    }

    fn open_stream_plan(&self, _plan: &ReadPlan) -> RuntimeResult<StreamPlan> {
        Err(Self::unsupported("stream"))
    }

    fn execute_export_plan(&self, _plan: &ExportPlan) -> RuntimeResult<PlanReceipt> {
        Err(Self::unsupported("export"))
    }

    fn commit_write_plan(&self, _plan: &WritePlan, _bytes: Vec<u8>) -> RuntimeResult<PlanReceipt> {
        Err(Self::unsupported("write"))
    }

    fn execute_command_plan(&self, _plan: &CommandPlan) -> RuntimeResult<PlanReceipt> {
        Err(Self::unsupported("command"))
    }

    fn execute_command_batch(&self, _batch: &CommandBatch) -> RuntimeResult<Vec<PlanReceipt>> {
        Err(Self::unsupported("command_batch"))
    }

    fn execute_saga_plan(&self, _saga: &SagaPlan) -> RuntimeResult<Vec<PlanReceipt>> {
        Err(Self::unsupported("saga"))
    }
}

#[allow(clippy::unused_self, clippy::unnecessary_wraps)]
impl PersistentProvider {
    pub fn create_blob_resource(
        &self,
        _schema: &str,
        _bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        Err(Self::unsupported("create_blob"))
    }

    pub fn create_cow_state_resource(
        &self,
        _kind_id: &str,
        _schema: &str,
        _bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        Err(Self::unsupported("create_cow"))
    }

    pub fn create_capability_resource(
        &self,
        _kind_id: &str,
        _schema: &str,
    ) -> RuntimeResult<ResourceRef> {
        Err(Self::unsupported("create_capability"))
    }
}

impl ResourceProviderGateway for PersistentProvider {
    fn execute(
        &self,
        request: mutsuki_runtime_sdk::ResourceProviderRequest,
    ) -> mutsuki_runtime_sdk::ResourceProviderOutcome<mutsuki_runtime_sdk::ResourceProviderReply>
    {
        use mutsuki_runtime_sdk::{ResourceProviderReply as R, ResourceProviderRequest as Q};
        let result = match request {
            Q::CreateBlob { schema, bytes } => {
                self.create_blob_resource(&schema, bytes).map(R::Created)
            }
            Q::CreateCow {
                kind_id,
                schema,
                bytes,
            } => self
                .create_cow_state_resource(&kind_id, &schema, bytes)
                .map(R::Created),
            Q::CreateCapability { kind_id, schema } => self
                .create_capability_resource(&kind_id, &schema)
                .map(R::Created),
            Q::Collect(plan) => self.collect_read_plan(&plan).map(R::Bytes),
            Q::Snapshot {
                plan,
                kind_id,
                schema,
            } => self
                .snapshot_read_plan(&plan, &kind_id, &schema)
                .map(|value| R::Snapshot(Box::new(value))),
            Q::OpenStream(plan) => self.open_stream_plan(&plan).map(R::Stream),
            Q::Export(plan) => self
                .execute_export_plan(&plan)
                .map(|value| R::Receipt(Box::new(value))),
            Q::Commit { plan, bytes } => self
                .commit_write_plan(&plan, bytes)
                .map(|value| R::Receipt(Box::new(value))),
            Q::Command(plan) => self
                .execute_command_plan(&plan)
                .map(|value| R::Receipt(Box::new(value))),
            Q::Batch(batch) => self.execute_command_batch(&batch).map(R::Receipts),
            Q::Saga(saga) => self.execute_saga_plan(&saga).map(R::Receipts),
        };
        mutsuki_runtime_sdk::ResourceProviderOutcome::new(result)
    }

    fn restore_descriptors(&self) -> RuntimeResult<Vec<ResourceRef>> {
        Ok(self
            .stored
            .iter()
            .map(|descriptor| {
                let mut descriptor = descriptor.clone();
                descriptor.provider_id = self.owner_id.clone();
                descriptor
            })
            .collect())
    }
}

/// Persistent providers may use the native async boundary as well. Restore is
/// intentionally synchronous so boot can rebuild the Core registry before the
/// actor starts, regardless of how plans execute after startup.
struct AsyncPersistentProvider {
    stored: Vec<ResourceRef>,
    owner_id: String,
}

impl AsyncResourceProviderGateway for AsyncPersistentProvider {
    fn execute(
        &self,
        _request: mutsuki_runtime_sdk::ResourceProviderRequest,
    ) -> mutsuki_runtime_sdk::ResourceProviderFuture {
        Box::pin(async {
            mutsuki_runtime_sdk::ResourceProviderOutcome::new(Err(PersistentProvider::unsupported(
                "execute",
            )))
        })
    }

    fn restore_descriptors(&self) -> RuntimeResult<Vec<ResourceRef>> {
        Ok(self
            .stored
            .iter()
            .map(|descriptor| {
                let mut descriptor = descriptor.clone();
                descriptor.provider_id = self.owner_id.clone();
                descriptor
            })
            .collect())
    }
}

fn bootstrapper_with(provider: PersistentProvider) -> RuntimeBootstrapper {
    let mut manifest = runner_manifest("plugin-a", Vec::new());
    manifest.provides.resource_providers = vec![PROVIDER_ID.into()];
    let mut bootstrapper = RuntimeBootstrapper::new();
    bootstrapper.register_manifest(manifest);
    bootstrapper.register_resource_provider(PROVIDER_ID, Arc::new(provider));
    bootstrapper
}

fn bootstrapper_with_async(provider: AsyncPersistentProvider) -> RuntimeBootstrapper {
    let mut manifest = runner_manifest("plugin-a", Vec::new());
    manifest.provides.resource_providers = vec![PROVIDER_ID.into()];
    let mut bootstrapper = RuntimeBootstrapper::new();
    bootstrapper.register_manifest(manifest);
    bootstrapper.register_async_resource_provider(PROVIDER_ID, Arc::new(provider));
    bootstrapper
}

#[test]
fn descriptors_a_persistent_provider_still_holds_are_openable_after_boot() {
    let stored = vec![
        PersistentProvider::descriptor("restored-1", PROVIDER_ID),
        PersistentProvider::descriptor("restored-2", PROVIDER_ID),
    ];
    let runtime = bootstrapper_with(PersistentProvider {
        stored: stored.clone(),
        owner_id: PROVIDER_ID.into(),
    })
    .into_host_runtime(runtime_profile())
    .unwrap();

    let registry = runtime.host_context().resource_registry();
    for expected in &stored {
        let opened = registry
            .open_resource_descriptor(expected.ref_id.as_str())
            .expect("a restored descriptor is registered at boot");
        assert_eq!(&opened, expected);
        // The descriptor is usable, not just present.
        assert_eq!(
            registry
                .collect_read_plan(&ReadPlan {
                    plan_id: format!("read:{}", expected.ref_id),
                    resource: opened,
                    operation: "collect".into(),
                    args: serde_json::Value::Null,
                })
                .unwrap(),
            b"restored"
        );
    }
}

#[test]
fn a_provider_restoring_someone_elses_descriptor_fails_the_boot() {
    let started = bootstrapper_with(PersistentProvider {
        stored: vec![PersistentProvider::descriptor("restored-1", PROVIDER_ID)],
        owner_id: "fake.other".into(),
    })
    .into_host_runtime(runtime_profile());
    let Err(error) = started else {
        panic!("a descriptor owned by another provider must not be registered");
    };
    assert_eq!(error.error().code, ERR_REGISTRY_UNAUTHORIZED);
}

#[test]
fn a_provider_with_nothing_stored_registers_nothing() {
    let runtime = bootstrapper_with(PersistentProvider {
        stored: Vec::new(),
        owner_id: PROVIDER_ID.into(),
    })
    .into_host_runtime(runtime_profile())
    .unwrap();
    assert_eq!(
        runtime
            .host_context()
            .resource_registry()
            .open_resource_descriptor("restored-1")
            .unwrap_err()
            .error()
            .code,
        ERR_RESOURCE_NOT_FOUND
    );
}

#[test]
fn an_async_provider_restores_descriptors_before_actor_start() {
    let stored = vec![PersistentProvider::descriptor(
        "restored-async",
        PROVIDER_ID,
    )];
    let runtime = bootstrapper_with_async(AsyncPersistentProvider {
        stored: stored.clone(),
        owner_id: PROVIDER_ID.into(),
    })
    .into_host_runtime(runtime_profile())
    .unwrap();

    assert_eq!(
        runtime
            .host_context()
            .resource_registry()
            .open_resource_descriptor("restored-async")
            .unwrap(),
        stored[0]
    );
}

#[test]
fn malformed_restored_access_route_is_rejected_before_host_start() {
    let mut descriptor = PersistentProvider::descriptor("malformed-access", PROVIDER_ID);
    descriptor.access = ResourceAccess::ProviderRpc {
        provider_id: "some.other.provider".into(),
        method: "restore".into(),
    };
    let result = bootstrapper_with(PersistentProvider {
        stored: vec![descriptor],
        owner_id: PROVIDER_ID.into(),
    })
    .into_host_runtime(runtime_profile());
    let error = match result {
        Ok(_) => panic!("a restored descriptor must not route through another provider"),
        Err(error) => error,
    };
    assert_eq!(error.error().code, ERR_REGISTRY_UNAUTHORIZED);
}

#[test]
fn malformed_restored_identity_is_rejected_before_host_start() {
    let mut malformed = PersistentProvider::descriptor("malformed-identity", PROVIDER_ID);
    malformed.generation = 0;
    malformed.resource_id.generation = 0;
    let result = bootstrapper_with(PersistentProvider {
        stored: vec![malformed],
        owner_id: PROVIDER_ID.into(),
    })
    .into_host_runtime(runtime_profile());
    let error = match result {
        Ok(_) => panic!("zero generation cannot be restored into the registry"),
        Err(error) => error,
    };
    assert_eq!(error.error().code, ERR_REGISTRY_UNAUTHORIZED);

    let mut malformed = PersistentProvider::descriptor("malformed-version", PROVIDER_ID);
    malformed.version = 0;
    malformed.resource_id.version = 0;
    let result = bootstrapper_with(PersistentProvider {
        stored: vec![malformed],
        owner_id: PROVIDER_ID.into(),
    })
    .into_host_runtime(runtime_profile());
    let error = match result {
        Ok(_) => panic!("zero version cannot be restored into the registry"),
        Err(error) => error,
    };
    assert_eq!(error.error().code, ERR_REGISTRY_UNAUTHORIZED);

    let mut malformed = PersistentProvider::descriptor("mismatched-identity", PROVIDER_ID);
    malformed.resource_id.version = malformed.version + 1;
    let result = bootstrapper_with(PersistentProvider {
        stored: vec![malformed],
        owner_id: PROVIDER_ID.into(),
    })
    .into_host_runtime(runtime_profile());
    let error = match result {
        Ok(_) => panic!("resource identity version must match the descriptor"),
        Err(error) => error,
    };
    assert_eq!(error.error().code, ERR_REGISTRY_UNAUTHORIZED);
}

#[test]
fn sync_and_async_routes_for_one_provider_are_rejected() {
    let result = bootstrapper_with(PersistentProvider {
        stored: Vec::new(),
        owner_id: PROVIDER_ID.into(),
    })
    .into_host_runtime_with_config(
        runtime_profile(),
        crate::HostRuntimeConfig::default().with_async_resource_provider(
            PROVIDER_ID,
            Arc::new(AsyncPersistentProvider {
                stored: Vec::new(),
                owner_id: PROVIDER_ID.into(),
            }),
        ),
    );
    let error = match result {
        Ok(_) => panic!("one provider id cannot have sync and async routes"),
        Err(error) => error,
    };
    assert_eq!(error.error().code, ERR_REGISTRY_UNAUTHORIZED);
}

#[test]
fn inactive_provider_rows_are_not_restored() {
    let mut manifest = runner_manifest("plugin-a", Vec::new());
    manifest.provides.resource_providers = vec!["active.provider".into()];
    let mut bootstrapper = RuntimeBootstrapper::new();
    bootstrapper.register_manifest(manifest);
    bootstrapper.register_resource_provider(
        "active.provider",
        Arc::new(PersistentProvider {
            stored: Vec::new(),
            owner_id: "active.provider".into(),
        }),
    );
    bootstrapper.register_resource_provider(
        "inactive.provider",
        Arc::new(PersistentProvider {
            stored: vec![PersistentProvider::descriptor(
                "inactive-resource",
                "inactive.provider",
            )],
            owner_id: "inactive.provider".into(),
        }),
    );
    let runtime = bootstrapper
        .into_host_runtime(runtime_profile())
        .expect("inactive provider registration is ignored by the resolved plan");
    assert_eq!(
        runtime
            .host_context()
            .resource_registry()
            .open_resource_descriptor("inactive-resource")
            .unwrap_err()
            .error()
            .code,
        ERR_RESOURCE_NOT_FOUND
    );
    assert_eq!(
        runtime
            .host_context()
            .resource_registry()
            .create_blob_resource("inactive.provider", "fixture.v1", b"blocked".to_vec())
            .unwrap_err()
            .error()
            .code,
        ERR_REGISTRY_UNAUTHORIZED
    );
}

#[test]
fn explicit_host_provider_route_survives_inactive_plugin_filtering() {
    let mut manifest = runner_manifest("plugin-a", Vec::new());
    manifest.provides.resource_providers = vec!["active.provider".into()];
    let mut bootstrapper = RuntimeBootstrapper::new();
    bootstrapper.register_manifest(manifest);
    bootstrapper.register_resource_provider(
        "active.provider",
        Arc::new(PersistentProvider {
            stored: Vec::new(),
            owner_id: "active.provider".into(),
        }),
    );
    let explicit_descriptor =
        PersistentProvider::descriptor("explicit-restored", "explicit.provider");
    let runtime = bootstrapper
        .into_host_runtime_with_config(
            runtime_profile(),
            crate::HostRuntimeConfig::default().with_resource_provider(
                "explicit.provider",
                Arc::new(PersistentProvider {
                    stored: vec![explicit_descriptor.clone()],
                    owner_id: "explicit.provider".into(),
                }),
            ),
        )
        .unwrap();
    assert_eq!(
        runtime
            .host_context()
            .resource_registry()
            .open_resource_descriptor("explicit-restored")
            .unwrap(),
        explicit_descriptor
    );
    assert_eq!(
        runtime
            .host_context()
            .resource_registry()
            .create_blob_resource("explicit.provider", "fixture.v1", b"explicit".to_vec())
            .unwrap_err()
            .error()
            .code,
        ERR_RESOURCE_UNSUPPORTED
    );
}

/// Parks inside `collect_read_plan` so a test can observe whether the Core
/// actor is held for the length of a provider call.
struct BlockingProvider {
    execution: ResourceProviderExecution,
    entered: Arc<std::sync::Barrier>,
    release: Arc<std::sync::Barrier>,
}

impl ResourcePlanGateway for BlockingProvider {
    fn collect_read_plan(&self, _plan: &ReadPlan) -> RuntimeResult<Vec<u8>> {
        self.entered.wait();
        self.release.wait();
        Ok(b"unblocked".to_vec())
    }

    fn snapshot_read_plan(
        &self,
        _plan: &ReadPlan,
        _kind_id: &str,
        _schema: &str,
    ) -> RuntimeResult<SnapshotDescriptor> {
        Err(PersistentProvider::unsupported("snapshot"))
    }

    fn open_stream_plan(&self, _plan: &ReadPlan) -> RuntimeResult<StreamPlan> {
        Err(PersistentProvider::unsupported("stream"))
    }

    fn execute_export_plan(&self, _plan: &ExportPlan) -> RuntimeResult<PlanReceipt> {
        Err(PersistentProvider::unsupported("export"))
    }

    fn commit_write_plan(&self, _plan: &WritePlan, _bytes: Vec<u8>) -> RuntimeResult<PlanReceipt> {
        Err(PersistentProvider::unsupported("write"))
    }

    fn execute_command_plan(&self, _plan: &CommandPlan) -> RuntimeResult<PlanReceipt> {
        Err(PersistentProvider::unsupported("command"))
    }

    fn execute_command_batch(&self, _batch: &CommandBatch) -> RuntimeResult<Vec<PlanReceipt>> {
        Err(PersistentProvider::unsupported("command_batch"))
    }

    fn execute_saga_plan(&self, _saga: &SagaPlan) -> RuntimeResult<Vec<PlanReceipt>> {
        Err(PersistentProvider::unsupported("saga"))
    }
}

#[allow(clippy::unused_self, clippy::unnecessary_wraps)]
impl BlockingProvider {
    pub fn create_blob_resource(
        &self,
        schema: &str,
        _bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        let mut descriptor = PersistentProvider::descriptor("blocking-1", PROVIDER_ID);
        descriptor.schema = schema.into();
        Ok(descriptor)
    }

    pub fn create_cow_state_resource(
        &self,
        _kind_id: &str,
        _schema: &str,
        _bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        Err(PersistentProvider::unsupported("create_cow"))
    }

    pub fn create_capability_resource(
        &self,
        _kind_id: &str,
        _schema: &str,
    ) -> RuntimeResult<ResourceRef> {
        Err(PersistentProvider::unsupported("create_capability"))
    }
}

impl ResourceProviderGateway for BlockingProvider {
    fn execute(
        &self,
        request: mutsuki_runtime_sdk::ResourceProviderRequest,
    ) -> mutsuki_runtime_sdk::ResourceProviderOutcome<mutsuki_runtime_sdk::ResourceProviderReply>
    {
        use mutsuki_runtime_sdk::{ResourceProviderReply as R, ResourceProviderRequest as Q};
        let result = match request {
            Q::CreateBlob { schema, bytes } => {
                self.create_blob_resource(&schema, bytes).map(R::Created)
            }
            Q::CreateCow {
                kind_id,
                schema,
                bytes,
            } => self
                .create_cow_state_resource(&kind_id, &schema, bytes)
                .map(R::Created),
            Q::CreateCapability { kind_id, schema } => self
                .create_capability_resource(&kind_id, &schema)
                .map(R::Created),
            Q::Collect(plan) => self.collect_read_plan(&plan).map(R::Bytes),
            Q::Snapshot {
                plan,
                kind_id,
                schema,
            } => self
                .snapshot_read_plan(&plan, &kind_id, &schema)
                .map(|value| R::Snapshot(Box::new(value))),
            Q::OpenStream(plan) => self.open_stream_plan(&plan).map(R::Stream),
            Q::Export(plan) => self
                .execute_export_plan(&plan)
                .map(|value| R::Receipt(Box::new(value))),
            Q::Commit { plan, bytes } => self
                .commit_write_plan(&plan, bytes)
                .map(|value| R::Receipt(Box::new(value))),
            Q::Command(plan) => self
                .execute_command_plan(&plan)
                .map(|value| R::Receipt(Box::new(value))),
            Q::Batch(batch) => self.execute_command_batch(&batch).map(R::Receipts),
            Q::Saga(saga) => self.execute_saga_plan(&saga).map(R::Receipts),
        };
        mutsuki_runtime_sdk::ResourceProviderOutcome::new(result)
    }

    fn execution(&self) -> ResourceProviderExecution {
        self.execution
    }
}

fn blocking_runtime(provider: BlockingProvider) -> crate::HostRuntime {
    let mut manifest = runner_manifest("plugin-a", Vec::new());
    manifest.provides.resource_providers = vec![PROVIDER_ID.into()];
    let mut bootstrapper = RuntimeBootstrapper::new();
    bootstrapper.register_manifest(manifest);
    bootstrapper.register_resource_provider(PROVIDER_ID, Arc::new(provider));
    let config = crate::HostRuntimeConfig::default().with_async_executor(Arc::new(
        crate::TokioAsyncExecutor::new(2, 8, 8, 1024 * 1024).unwrap(),
    ));
    bootstrapper
        .into_host_runtime_with_config(runtime_profile(), config)
        .unwrap()
}

#[test]
fn an_offloaded_provider_does_not_hold_the_core_actor() {
    let entered = Arc::new(std::sync::Barrier::new(2));
    let release = Arc::new(std::sync::Barrier::new(2));
    let runtime = blocking_runtime(BlockingProvider {
        execution: ResourceProviderExecution::Offloaded,
        entered: entered.clone(),
        release: release.clone(),
    });

    let registry = runtime.host_context().resource_registry_ref();
    let blocked = std::thread::spawn({
        let registry = registry.clone();
        move || {
            registry.collect_read_plan(&ReadPlan {
                plan_id: "read:blocked".into(),
                resource: PersistentProvider::descriptor("restored-1", PROVIDER_ID),
                operation: "collect".into(),
                args: serde_json::Value::Null,
            })
        }
    });
    entered.wait();

    // The provider call is now parked. An unrelated command must still be
    // answered; if the plan were running inline the actor could not reply.
    let (probe_tx, probe_rx) = std::sync::mpsc::channel();
    std::thread::spawn({
        let registry = registry.clone();
        move || {
            let _ = probe_tx.send(registry.open_resource_descriptor("absent"));
        }
    });
    let probed = probe_rx.recv_timeout(Duration::from_secs(10));

    // Unpark the provider before asserting: a regression must fail this test
    // rather than leave the actor wedged and deadlock the shutdown that runs
    // when the runtime is dropped.
    release.wait();
    let unblocked = blocked.join().unwrap();

    let probed = probed.expect("the actor must answer while an offloaded provider is blocked");
    assert_eq!(probed.unwrap_err().error().code, ERR_RESOURCE_NOT_FOUND);
    assert_eq!(unblocked.unwrap(), b"unblocked");
}

#[test]
fn an_offloaded_create_registers_its_descriptor_on_the_actor() {
    let unused = Arc::new(std::sync::Barrier::new(1));
    let runtime = blocking_runtime(BlockingProvider {
        execution: ResourceProviderExecution::Offloaded,
        entered: unused.clone(),
        release: unused,
    });
    let registry = runtime.host_context().resource_registry();
    let created = registry
        .create_blob_resource(PROVIDER_ID, "fixture.v1", b"payload".to_vec())
        .unwrap();
    // Registering is a Core mutation, so it has to happen on the actor even
    // though the provider call itself ran on the executor.
    assert_eq!(
        registry
            .open_resource_descriptor(created.ref_id.as_str())
            .unwrap(),
        created
    );
}

struct BoundedState {
    next_slot: u64,
    order: Vec<String>,
    resources: HashMap<String, (ResourceRef, Vec<u8>)>,
    reclaimed_by_create: HashMap<String, Vec<String>>,
}

/// In-memory stand-in for a provider that reclaims older payloads on create.
struct BoundedProvider {
    max_total_bytes: u64,
    execution: ResourceProviderExecution,
    state: Mutex<BoundedState>,
}

impl BoundedProvider {
    fn new(max_total_bytes: u64, execution: ResourceProviderExecution) -> Self {
        Self {
            max_total_bytes,
            execution,
            state: Mutex::new(BoundedState {
                next_slot: 1,
                order: Vec::new(),
                resources: HashMap::new(),
                reclaimed_by_create: HashMap::new(),
            }),
        }
    }

    fn insert(
        &self,
        kind_id: &str,
        semantic: ResourceSemantic,
        schema: &str,
        bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        let incoming_len = if semantic == ResourceSemantic::CapabilityResource {
            0
        } else {
            bytes.len() as u64
        };
        if semantic != ResourceSemantic::CapabilityResource && incoming_len > self.max_total_bytes {
            return Err(PersistentProvider::unsupported("payload_exceeds_bound"));
        }
        let mut state = self.state.lock().expect("bounded provider mutex");
        let mut reclaimed = Vec::new();
        if semantic != ResourceSemantic::CapabilityResource {
            let mut total: u64 = state
                .resources
                .values()
                .map(|(_, payload)| payload.len() as u64)
                .sum();
            let target = self.max_total_bytes.saturating_sub(incoming_len);
            let mut index = 0;
            while total > target && index < state.order.len() {
                let ref_id = state.order[index].clone();
                let Some((descriptor, payload)) = state.resources.get(&ref_id) else {
                    index += 1;
                    continue;
                };
                if descriptor.semantic == ResourceSemantic::CapabilityResource {
                    index += 1;
                    continue;
                }
                total = total.saturating_sub(payload.len() as u64);
                state.resources.remove(&ref_id);
                state.order.remove(index);
                reclaimed.push(ref_id);
            }
        }
        let slot = state.next_slot;
        state.next_slot += 1;
        let ref_id = format!("bounded-{slot}");
        let mut descriptor = PersistentProvider::descriptor(&ref_id, PROVIDER_ID);
        descriptor.resource_kind = kind_id.into();
        descriptor.resource_id.kind_id = kind_id.into();
        descriptor.semantic = semantic;
        descriptor.schema = schema.into();
        descriptor.size_hint = Some(bytes.len() as u64);
        state
            .resources
            .insert(ref_id.clone(), (descriptor.clone(), bytes));
        state.order.push(ref_id.clone());
        if !reclaimed.is_empty() {
            state.reclaimed_by_create.insert(ref_id, reclaimed);
        }
        Ok(descriptor)
    }
}

impl BoundedProvider {
    fn create_blob_resource(&self, schema: &str, bytes: Vec<u8>) -> RuntimeResult<ResourceRef> {
        self.insert("fixture.blob", ResourceSemantic::FrozenValue, schema, bytes)
    }
    fn create_cow_state_resource(
        &self,
        kind_id: &str,
        schema: &str,
        bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        self.insert(kind_id, ResourceSemantic::CowVersionedState, schema, bytes)
    }
    fn create_capability_resource(
        &self,
        kind_id: &str,
        schema: &str,
    ) -> RuntimeResult<ResourceRef> {
        self.insert(
            kind_id,
            ResourceSemantic::CapabilityResource,
            schema,
            Vec::new(),
        )
    }
}

impl ResourcePlanGateway for BoundedProvider {
    fn collect_read_plan(&self, plan: &ReadPlan) -> RuntimeResult<Vec<u8>> {
        self.state
            .lock()
            .expect("bounded provider mutex")
            .resources
            .get(plan.resource.ref_id.as_str())
            .map(|(_, bytes)| bytes.clone())
            .ok_or_else(|| {
                RuntimeFailure::new(RuntimeError::new(
                    ERR_RESOURCE_NOT_FOUND,
                    "runtime.resource_provider.fake",
                    format!("resource.bounded.{}", plan.resource.ref_id),
                ))
            })
    }

    fn snapshot_read_plan(
        &self,
        _plan: &ReadPlan,
        _kind_id: &str,
        _schema: &str,
    ) -> RuntimeResult<SnapshotDescriptor> {
        Err(PersistentProvider::unsupported("snapshot"))
    }

    fn open_stream_plan(&self, _plan: &ReadPlan) -> RuntimeResult<StreamPlan> {
        Err(PersistentProvider::unsupported("stream"))
    }

    fn execute_export_plan(&self, _plan: &ExportPlan) -> RuntimeResult<PlanReceipt> {
        Err(PersistentProvider::unsupported("export"))
    }

    fn commit_write_plan(&self, _plan: &WritePlan, _bytes: Vec<u8>) -> RuntimeResult<PlanReceipt> {
        Err(PersistentProvider::unsupported("write"))
    }

    fn execute_command_plan(&self, plan: &CommandPlan) -> RuntimeResult<PlanReceipt> {
        if plan.operation != "delete" {
            return Err(PersistentProvider::unsupported("command"));
        }
        let target_ref_id = plan
            .args
            .get("ref_id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| PersistentProvider::unsupported("missing ref_id"))?
            .to_string();
        let mut state = self.state.lock().expect("bounded provider mutex");
        if state.resources.remove(&target_ref_id).is_none() {
            return Err(RuntimeFailure::new(RuntimeError::new(
                ERR_RESOURCE_NOT_FOUND,
                "runtime.resource_provider.fake",
                format!("resource.bounded.delete.{target_ref_id}"),
            )));
        }
        state.order.retain(|ref_id| ref_id != &target_ref_id);
        Ok(PlanReceipt {
            plan_id: plan.plan_id.clone(),
            status: "deleted".into(),
            resource_ref: Some(plan.capability.clone()),
            snapshot: None,
            descriptor_updates: Vec::new(),
            descriptor_removals: vec![target_ref_id.clone()],
            new_version: None,
            output: serde_json::json!({ "deleted_ref_id": target_ref_id }),
        })
    }

    fn execute_command_batch(&self, _batch: &CommandBatch) -> RuntimeResult<Vec<PlanReceipt>> {
        Err(PersistentProvider::unsupported("command_batch"))
    }

    fn execute_saga_plan(&self, _saga: &SagaPlan) -> RuntimeResult<Vec<PlanReceipt>> {
        Err(PersistentProvider::unsupported("saga"))
    }
}

impl ResourceProviderGateway for BoundedProvider {
    fn execute(
        &self,
        request: mutsuki_runtime_sdk::ResourceProviderRequest,
    ) -> mutsuki_runtime_sdk::ResourceProviderOutcome<mutsuki_runtime_sdk::ResourceProviderReply>
    {
        use mutsuki_runtime_sdk::{ResourceProviderReply as R, ResourceProviderRequest as Q};
        let result = match request {
            Q::CreateBlob { schema, bytes } => {
                self.create_blob_resource(&schema, bytes).map(R::Created)
            }
            Q::CreateCow {
                kind_id,
                schema,
                bytes,
            } => self
                .create_cow_state_resource(&kind_id, &schema, bytes)
                .map(R::Created),
            Q::CreateCapability { kind_id, schema } => self
                .create_capability_resource(&kind_id, &schema)
                .map(R::Created),
            Q::Collect(plan) => self.collect_read_plan(&plan).map(R::Bytes),
            Q::Snapshot {
                plan,
                kind_id,
                schema,
            } => self
                .snapshot_read_plan(&plan, &kind_id, &schema)
                .map(|value| R::Snapshot(Box::new(value))),
            Q::OpenStream(plan) => self.open_stream_plan(&plan).map(R::Stream),
            Q::Export(plan) => self
                .execute_export_plan(&plan)
                .map(|value| R::Receipt(Box::new(value))),
            Q::Commit { plan, bytes } => self
                .commit_write_plan(&plan, bytes)
                .map(|value| R::Receipt(Box::new(value))),
            Q::Command(plan) => self
                .execute_command_plan(&plan)
                .map(|value| R::Receipt(Box::new(value))),
            Q::Batch(batch) => self.execute_command_batch(&batch).map(R::Receipts),
            Q::Saga(saga) => self.execute_saga_plan(&saga).map(R::Receipts),
        };
        let mut outcome = mutsuki_runtime_sdk::ResourceProviderOutcome::new(result);
        if let Ok(R::Created(descriptor)) = &outcome.result {
            let reclaimed = self
                .state
                .lock()
                .expect("bounded provider mutex")
                .reclaimed_by_create
                .remove(descriptor.ref_id.as_str())
                .unwrap_or_default();
            outcome.invalidations = reclaimed
                .into_iter()
                .map(|ref_id| ResourceDescriptorInvalidation {
                    provider_id: PROVIDER_ID.into(),
                    ref_id: ref_id.into(),
                    generation: 1,
                })
                .collect();
        }
        outcome
    }

    fn execution(&self) -> ResourceProviderExecution {
        self.execution
    }

    fn take_reclaimed_ref_ids(&self, created_ref_id: &str) -> RuntimeResult<Vec<String>> {
        Ok(self
            .state
            .lock()
            .expect("bounded provider mutex")
            .reclaimed_by_create
            .remove(created_ref_id)
            .unwrap_or_default())
    }
}

fn bounded_runtime(provider: BoundedProvider) -> crate::HostRuntime {
    let mut manifest = runner_manifest("plugin-a", Vec::new());
    manifest.provides.resource_providers = vec![PROVIDER_ID.into()];
    let mut bootstrapper = RuntimeBootstrapper::new();
    bootstrapper.register_manifest(manifest);
    bootstrapper.register_resource_provider(PROVIDER_ID, Arc::new(provider));
    let config = crate::HostRuntimeConfig::default().with_async_executor(Arc::new(
        crate::TokioAsyncExecutor::new(2, 8, 8, 1024 * 1024).unwrap(),
    ));
    bootstrapper
        .into_host_runtime_with_config(runtime_profile(), config)
        .unwrap()
}

#[test]
fn host_create_unregisters_reclaimed_descriptors_without_restart() {
    let runtime = bounded_runtime(BoundedProvider::new(
        16,
        ResourceProviderExecution::Offloaded,
    ));
    let registry = runtime.host_context().resource_registry();
    let oldest = registry
        .create_blob_resource(PROVIDER_ID, "fixture.v1", vec![b'a'; 10])
        .unwrap();
    let newest = registry
        .create_blob_resource(PROVIDER_ID, "fixture.v1", vec![b'b'; 10])
        .unwrap();

    assert_eq!(
        registry
            .open_resource_descriptor(oldest.ref_id.as_str())
            .unwrap_err()
            .error()
            .code,
        ERR_RESOURCE_NOT_FOUND
    );
    assert_eq!(
        registry
            .open_resource_descriptor(newest.ref_id.as_str())
            .unwrap(),
        newest
    );
}

#[test]
fn host_delete_command_unregisters_the_deleted_descriptor() {
    let runtime = bounded_runtime(BoundedProvider::new(
        64,
        ResourceProviderExecution::Offloaded,
    ));
    let registry = runtime.host_context().resource_registry();
    let capability = registry
        .create_capability_resource(PROVIDER_ID, "fixture.capability", "fixture.v1")
        .unwrap();
    let blob = registry
        .create_blob_resource(PROVIDER_ID, "fixture.v1", b"doomed".to_vec())
        .unwrap();
    registry
        .execute_command_plan(&CommandPlan {
            plan_id: "command:delete".into(),
            capability,
            operation: "delete".into(),
            args: serde_json::json!({ "ref_id": blob.ref_id }),
            idempotency_key: None,
        })
        .unwrap();
    assert_eq!(
        registry
            .open_resource_descriptor(blob.ref_id.as_str())
            .unwrap_err()
            .error()
            .code,
        ERR_RESOURCE_NOT_FOUND
    );
}
