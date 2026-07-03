use std::marker::PhantomData;

use mutsuki_runtime_contracts::{
    CommandBatch, CommandPlan, ExportPlan, ReadPlan, ResourceRef, ResourceSemantic, SagaPlan,
    StreamPlan, TransactionPlan, WritePlan,
};
use serde_json::Value;

pub trait ResourceKind {
    const KIND_ID: &'static str;
    const SEMANTIC: ResourceSemantic;
}

#[derive(Clone, Debug)]
pub struct TypedResourceHandle<T> {
    resource: ResourceRef,
    _marker: PhantomData<T>,
}

impl<T> TypedResourceHandle<T> {
    pub fn new(resource: ResourceRef) -> Self {
        Self {
            resource,
            _marker: PhantomData,
        }
    }

    pub fn resource(&self) -> &ResourceRef {
        &self.resource
    }

    pub fn into_resource(self) -> ResourceRef {
        self.resource
    }
}

impl<T: ResourceKind> TypedResourceHandle<T> {
    pub fn descriptor_matches_kind(&self) -> bool {
        self.resource.resource_id.kind_id == T::KIND_ID && self.resource.semantic == T::SEMANTIC
    }
}

#[derive(Clone, Debug, Default)]
pub struct ResourceClient;

impl ResourceClient {
    pub fn handle<T>(&self, resource: ResourceRef) -> TypedResourceHandle<T> {
        TypedResourceHandle::new(resource)
    }

    pub fn read_plan<T>(
        &self,
        handle: &TypedResourceHandle<T>,
        operation: impl Into<String>,
    ) -> ReadPlan {
        let operation = operation.into();
        ReadPlan {
            plan_id: format!("read-plan:{}:{operation}", handle.resource.ref_id),
            resource: handle.resource.clone(),
            operation,
            args: Value::Null,
        }
    }

    pub fn write_plan<T>(
        &self,
        handle: &TypedResourceHandle<T>,
        conflict_policy: impl Into<String>,
        operations: Value,
    ) -> WritePlan {
        let conflict_policy = conflict_policy.into();
        let patch = mutsuki_runtime_contracts::PatchDescriptor {
            patch_id: format!(
                "patch:{}:{}",
                handle.resource.ref_id, handle.resource.version
            ),
            target_ref: handle.resource.clone(),
            base_version: handle.resource.version,
            conflict_policy: conflict_policy.clone(),
            operations,
        };
        WritePlan {
            plan_id: format!(
                "write-plan:{}:{}",
                handle.resource.ref_id, handle.resource.version
            ),
            resource: handle.resource.clone(),
            base_version: patch.base_version,
            conflict_policy,
            patch,
            returning: None,
        }
    }

    pub fn stream_plan<T>(&self, handle: &TypedResourceHandle<T>) -> StreamPlan {
        StreamPlan {
            plan_id: format!("stream-plan:{}", handle.resource.ref_id),
            resource: handle.resource.clone(),
            operation: "open_stream".into(),
            args: Value::Null,
        }
    }

    pub fn export_plan<T>(
        &self,
        handle: &TypedResourceHandle<T>,
        target: impl Into<String>,
    ) -> ExportPlan {
        let target = target.into();
        ExportPlan {
            plan_id: format!("export-plan:{}:{target}", handle.resource.ref_id),
            resource: handle.resource.clone(),
            target,
            args: Value::Null,
        }
    }

    pub fn command_plan<T>(
        &self,
        capability: &TypedResourceHandle<T>,
        operation: impl Into<String>,
        args: Value,
        idempotency_key: Option<String>,
    ) -> CommandPlan {
        let operation = operation.into();
        CommandPlan {
            plan_id: format!("command-plan:{}:{operation}", capability.resource.ref_id),
            capability: capability.resource.clone(),
            operation,
            args,
            idempotency_key,
        }
    }

    #[deprecated(
        note = "experimental descriptor helper; CoreRuntime does not execute transaction semantics"
    )]
    pub fn transaction_plan(
        &self,
        plan_id: impl Into<String>,
        operations: Vec<WritePlan>,
        strict: bool,
    ) -> TransactionPlan {
        TransactionPlan {
            plan_id: plan_id.into(),
            operations,
            strict,
        }
    }

    #[deprecated(
        note = "experimental descriptor helper; CoreRuntime does not execute batch semantics"
    )]
    pub fn command_batch(
        &self,
        batch_id: impl Into<String>,
        commands: Vec<CommandPlan>,
        rollback_guarantee: bool,
    ) -> CommandBatch {
        CommandBatch {
            batch_id: batch_id.into(),
            commands,
            rollback_guarantee,
        }
    }

    #[deprecated(
        note = "experimental descriptor helper; CoreRuntime does not execute saga semantics"
    )]
    pub fn saga_plan(
        &self,
        saga_id: impl Into<String>,
        steps: Vec<CommandPlan>,
        compensations: Vec<CommandPlan>,
    ) -> SagaPlan {
        SagaPlan {
            saga_id: saga_id.into(),
            steps,
            compensations,
        }
    }
}

#[cfg(test)]
mod tests {
    use mutsuki_runtime_contracts::{
        ResourceAccess, ResourceId, ResourceLifetime, ResourceSealState, ResourceSemantic,
    };
    use serde_json::json;

    use super::*;

    struct TestState;

    impl ResourceKind for TestState {
        const KIND_ID: &'static str = "text_buffer";
        const SEMANTIC: ResourceSemantic = ResourceSemantic::CowVersionedState;
    }

    #[test]
    fn resource_client_builds_stable_resource_plan_shapes() {
        let client = ResourceClient;
        let state = resource_ref("state", "text_buffer", ResourceSemantic::CowVersionedState);
        let state_handle = client.handle::<TestState>(state.clone());

        let read = client.read_plan(&state_handle, "collect");
        let write = client.write_plan(&state_handle, "fail", json!({"replace": "all"}));
        let stream = client.stream_plan(&state_handle);
        let export = client.export_plan(&state_handle, "json");

        let capability = resource_ref("db", "db_pool", ResourceSemantic::CapabilityResource);
        let capability_handle = client.handle::<()>(capability.clone());
        let command = client.command_plan(
            &capability_handle,
            "query",
            json!({"sql": "select 1"}),
            Some("query:1".into()),
        );

        assert_eq!(read.resource.ref_id, state.ref_id);
        assert_eq!(write.patch.base_version, state.version);
        assert_eq!(stream.operation, "open_stream");
        assert_eq!(export.target, "json");
        assert_eq!(command.capability.ref_id, capability.ref_id);
        assert_eq!(command.idempotency_key.as_deref(), Some("query:1"));
        assert!(state_handle.descriptor_matches_kind());
    }

    fn resource_ref(slot_id: &str, kind_id: &str, semantic: ResourceSemantic) -> ResourceRef {
        let ref_id = format!("resource:{slot_id}");
        ResourceRef {
            resource_id: ResourceId {
                kind_id: kind_id.into(),
                slot_id: ref_id.clone(),
                generation: 1,
                version: 1,
            },
            ref_id,
            semantic,
            provider_id: "mutsuki.sdk.test".into(),
            resource_kind: kind_id.into(),
            schema: format!("{kind_id}.v1"),
            version: 1,
            generation: 1,
            access: ResourceAccess::Inline,
            size_hint: None,
            content_hash: None,
            lifetime: ResourceLifetime::BorrowedUntilTaskEnd,
            lease: None,
            seal_state: ResourceSealState::Sealed,
        }
    }
}
