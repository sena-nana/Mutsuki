use std::sync::Arc;

use mutsuki_runtime_contracts::resource::experimental::{CommandBatch, SagaPlan};
use mutsuki_runtime_contracts::*;
use mutsuki_runtime_core::{RuntimeFailure, RuntimeResult};
use mutsuki_runtime_sdk::{ResourcePlanGateway, ResourceProviderGateway};

use crate::{RuntimeBootstrapper, runner_manifest};

use super::helpers::runtime_profile;

const PROVIDER_ID: &str = "fake.persistent";

/// Stands in for a provider whose rows outlive the process: it reports the
/// descriptors it "already had" instead of creating them at runtime.
struct PersistentProvider {
    stored: Vec<ResourceRef>,
    owner_id: String,
}

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

impl ResourceProviderGateway for PersistentProvider {
    fn create_blob_resource(&self, _schema: &str, _bytes: Vec<u8>) -> RuntimeResult<ResourceRef> {
        Err(Self::unsupported("create_blob"))
    }

    fn create_cow_state_resource(
        &self,
        _kind_id: &str,
        _schema: &str,
        _bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        Err(Self::unsupported("create_cow"))
    }

    fn create_capability_resource(
        &self,
        _kind_id: &str,
        _schema: &str,
    ) -> RuntimeResult<ResourceRef> {
        Err(Self::unsupported("create_capability"))
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
