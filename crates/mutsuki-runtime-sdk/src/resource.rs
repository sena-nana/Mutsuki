use std::fmt;
use std::marker::PhantomData;

use mutsuki_runtime_contracts::resource::experimental::{CommandBatch, SagaPlan, TransactionPlan};
use mutsuki_runtime_contracts::{
    CommandPlan, ExportPlan, ReadPlan, ResourceRef, ResourceSemantic, RuntimeError, StreamPlan,
    WritePlan,
};
use mutsuki_runtime_core::{RuntimeFailure, RuntimeResult};
use serde_json::Value;

pub trait ResourceKind {
    const KIND_ID: &'static str;
    const SEMANTIC: ResourceSemantic;
}

#[derive(Clone)]
pub struct TypedResourceHandle<T> {
    resource: ResourceRef,
    _marker: PhantomData<fn() -> T>,
}

impl<T> fmt::Debug for TypedResourceHandle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TypedResourceHandle")
            .field("resource", &self.resource)
            .finish()
    }
}

impl<T> TypedResourceHandle<T> {
    pub fn resource(&self) -> &ResourceRef {
        &self.resource
    }

    pub fn into_resource(self) -> ResourceRef {
        self.resource
    }
}

impl<T: ResourceKind> TypedResourceHandle<T> {
    pub fn try_from_resource(resource: ResourceRef) -> RuntimeResult<Self> {
        require_kind::<T>(&resource)?;
        Ok(Self {
            resource,
            _marker: PhantomData,
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct ResourceClient;

impl ResourceClient {
    pub fn handle<T: ResourceKind>(
        &self,
        resource: ResourceRef,
    ) -> RuntimeResult<TypedResourceHandle<T>> {
        TypedResourceHandle::try_from_resource(resource)
    }

    pub fn read_plan<T: ResourceKind>(
        &self,
        handle: &TypedResourceHandle<T>,
        operation: impl Into<String>,
    ) -> RuntimeResult<ReadPlan> {
        require_plan_semantic(
            &handle.resource,
            READ_PLAN_SEMANTICS,
            format!("resource.read_plan.{}", handle.resource.ref_id),
        )?;
        let operation = operation.into();
        Ok(ReadPlan {
            plan_id: format!("read-plan:{}:{operation}", handle.resource.ref_id),
            resource: handle.resource.clone(),
            operation,
            args: Value::Null,
        })
    }

    pub fn write_plan<T: ResourceKind>(
        &self,
        handle: &TypedResourceHandle<T>,
        conflict_policy: impl Into<String>,
        operations: Value,
    ) -> RuntimeResult<WritePlan> {
        require_plan_semantic(
            &handle.resource,
            WRITE_PLAN_SEMANTICS,
            format!("resource.write_plan.{}", handle.resource.ref_id),
        )?;
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
        Ok(WritePlan {
            plan_id: format!(
                "write-plan:{}:{}",
                handle.resource.ref_id, handle.resource.version
            ),
            resource: handle.resource.clone(),
            base_version: patch.base_version,
            conflict_policy,
            patch,
            returning: None,
        })
    }

    pub fn stream_plan<T: ResourceKind>(
        &self,
        handle: &TypedResourceHandle<T>,
    ) -> RuntimeResult<StreamPlan> {
        require_plan_semantic(
            &handle.resource,
            STREAM_PLAN_SEMANTICS,
            format!("resource.stream_plan.{}", handle.resource.ref_id),
        )?;
        Ok(StreamPlan {
            plan_id: format!("stream-plan:{}", handle.resource.ref_id),
            resource: handle.resource.clone(),
            operation: "open_stream".into(),
            args: Value::Null,
        })
    }

    pub fn export_plan<T: ResourceKind>(
        &self,
        handle: &TypedResourceHandle<T>,
        target: impl Into<String>,
    ) -> RuntimeResult<ExportPlan> {
        require_plan_semantic(
            &handle.resource,
            EXPORT_PLAN_SEMANTICS,
            format!("resource.export_plan.{}", handle.resource.ref_id),
        )?;
        let target = target.into();
        Ok(ExportPlan {
            plan_id: format!("export-plan:{}:{target}", handle.resource.ref_id),
            resource: handle.resource.clone(),
            target,
            args: Value::Null,
        })
    }

    pub fn command_plan<T: ResourceKind>(
        &self,
        capability: &TypedResourceHandle<T>,
        operation: impl Into<String>,
        args: Value,
        idempotency_key: Option<String>,
    ) -> RuntimeResult<CommandPlan> {
        require_plan_semantic(
            &capability.resource,
            COMMAND_PLAN_SEMANTICS,
            format!("resource.command_plan.{}", capability.resource.ref_id),
        )?;
        let operation = operation.into();
        Ok(CommandPlan {
            plan_id: format!("command-plan:{}:{operation}", capability.resource.ref_id),
            capability: capability.resource.clone(),
            operation,
            args,
            idempotency_key,
        })
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

const SDK_SOURCE: &str = "runtime.sdk";

const READ_PLAN_SEMANTICS: &[ResourceSemantic] = &[
    ResourceSemantic::FrozenValue,
    ResourceSemantic::VersionedSnapshot,
    ResourceSemantic::ReadOnlyFact,
    ResourceSemantic::CowVersionedState,
    ResourceSemantic::StreamResource,
];

const WRITE_PLAN_SEMANTICS: &[ResourceSemantic] = &[ResourceSemantic::CowVersionedState];

const STREAM_PLAN_SEMANTICS: &[ResourceSemantic] = &[ResourceSemantic::StreamResource];

const EXPORT_PLAN_SEMANTICS: &[ResourceSemantic] = &[
    ResourceSemantic::FrozenValue,
    ResourceSemantic::VersionedSnapshot,
    ResourceSemantic::ReadOnlyFact,
    ResourceSemantic::CowVersionedState,
];

const COMMAND_PLAN_SEMANTICS: &[ResourceSemantic] = &[ResourceSemantic::CapabilityResource];

fn require_kind<T: ResourceKind>(resource: &ResourceRef) -> RuntimeResult<()> {
    if resource.resource_id.kind_id != T::KIND_ID {
        return Err(sdk_resource_error(
            "resource.kind_mismatch",
            format!("resource.handle.{}", resource.ref_id),
        ));
    }
    if resource.semantic != T::SEMANTIC {
        return Err(sdk_resource_error(
            "resource.semantic_mismatch",
            format!("resource.handle.{}", resource.ref_id),
        ));
    }
    Ok(())
}

fn require_plan_semantic(
    resource: &ResourceRef,
    allowed: &[ResourceSemantic],
    route: String,
) -> RuntimeResult<()> {
    if allowed.contains(&resource.semantic) {
        Ok(())
    } else {
        Err(sdk_resource_error("resource.semantic_mismatch", route))
    }
}

fn sdk_resource_error(code: &'static str, route: impl Into<String>) -> RuntimeFailure {
    RuntimeFailure::new(RuntimeError::new(code, SDK_SOURCE, route))
}

#[cfg(test)]
mod tests {
    use mutsuki_runtime_contracts::{
        ResourceAccess, ResourceId, ResourceLifetime, ResourceSealState, ResourceSemantic,
    };
    use serde_json::json;

    use super::*;

    struct TestState;
    struct TestFrozen;
    struct TestSnapshot;
    struct TestFact;
    struct TestStream;
    struct TestCapability;
    struct WrongKind;
    struct WrongSemantic;

    impl ResourceKind for TestState {
        const KIND_ID: &'static str = "text_buffer";
        const SEMANTIC: ResourceSemantic = ResourceSemantic::CowVersionedState;
    }

    impl ResourceKind for TestFrozen {
        const KIND_ID: &'static str = "blob";
        const SEMANTIC: ResourceSemantic = ResourceSemantic::FrozenValue;
    }

    impl ResourceKind for TestSnapshot {
        const KIND_ID: &'static str = "ast_snapshot";
        const SEMANTIC: ResourceSemantic = ResourceSemantic::VersionedSnapshot;
    }

    impl ResourceKind for TestFact {
        const KIND_ID: &'static str = "project_facts";
        const SEMANTIC: ResourceSemantic = ResourceSemantic::ReadOnlyFact;
    }

    impl ResourceKind for TestStream {
        const KIND_ID: &'static str = "model_output_stream";
        const SEMANTIC: ResourceSemantic = ResourceSemantic::StreamResource;
    }

    impl ResourceKind for TestCapability {
        const KIND_ID: &'static str = "db_pool";
        const SEMANTIC: ResourceSemantic = ResourceSemantic::CapabilityResource;
    }

    impl ResourceKind for WrongKind {
        const KIND_ID: &'static str = "other_kind";
        const SEMANTIC: ResourceSemantic = ResourceSemantic::CowVersionedState;
    }

    impl ResourceKind for WrongSemantic {
        const KIND_ID: &'static str = "text_buffer";
        const SEMANTIC: ResourceSemantic = ResourceSemantic::FrozenValue;
    }

    #[test]
    fn resource_client_builds_stable_resource_plan_shapes() {
        let client = ResourceClient;
        let state = resource_ref("state", "text_buffer", ResourceSemantic::CowVersionedState);
        let state_handle = client.handle::<TestState>(state.clone()).unwrap();

        let read = client.read_plan(&state_handle, "collect").unwrap();
        let write = client
            .write_plan(&state_handle, "fail", json!({"replace": "all"}))
            .unwrap();
        let export = client.export_plan(&state_handle, "json").unwrap();

        let stream = resource_ref(
            "stream",
            "model_output_stream",
            ResourceSemantic::StreamResource,
        );
        let stream_handle = client.handle::<TestStream>(stream.clone()).unwrap();
        let stream_plan = client.stream_plan(&stream_handle).unwrap();

        let capability = resource_ref("db", "db_pool", ResourceSemantic::CapabilityResource);
        let capability_handle = client.handle::<TestCapability>(capability.clone()).unwrap();
        let command = client
            .command_plan(
                &capability_handle,
                "query",
                json!({"sql": "select 1"}),
                Some("query:1".into()),
            )
            .unwrap();

        assert_eq!(read.resource.ref_id, state.ref_id);
        assert_eq!(write.patch.base_version, state.version);
        assert_eq!(stream_plan.operation, "open_stream");
        assert_eq!(export.target, "json");
        assert_eq!(command.capability.ref_id, capability.ref_id);
        assert_eq!(command.idempotency_key.as_deref(), Some("query:1"));
    }

    #[test]
    fn typed_resource_handle_rejects_kind_and_semantic_mismatch() {
        let client = ResourceClient;
        let state = resource_ref("state", "text_buffer", ResourceSemantic::CowVersionedState);

        let kind_error = client.handle::<WrongKind>(state.clone()).unwrap_err();
        assert_eq!(kind_error.error().code, "resource.kind_mismatch");
        assert_eq!(kind_error.error().source, "runtime.sdk");

        let semantic_error = client.handle::<WrongSemantic>(state.clone()).unwrap_err();
        assert_eq!(semantic_error.error().code, "resource.semantic_mismatch");
        assert_eq!(semantic_error.error().source, "runtime.sdk");

        let try_from_error =
            TypedResourceHandle::<TestFrozen>::try_from_resource(state).unwrap_err();
        assert_eq!(try_from_error.error().code, "resource.kind_mismatch");
    }

    #[test]
    fn resource_client_gates_plan_semantics() {
        let client = ResourceClient;
        let frozen = client
            .handle::<TestFrozen>(resource_ref(
                "frozen",
                "blob",
                ResourceSemantic::FrozenValue,
            ))
            .unwrap();
        let snapshot = client
            .handle::<TestSnapshot>(resource_ref(
                "snapshot",
                "ast_snapshot",
                ResourceSemantic::VersionedSnapshot,
            ))
            .unwrap();
        let fact = client
            .handle::<TestFact>(resource_ref(
                "facts",
                "project_facts",
                ResourceSemantic::ReadOnlyFact,
            ))
            .unwrap();
        let stream = client
            .handle::<TestStream>(resource_ref(
                "stream",
                "model_output_stream",
                ResourceSemantic::StreamResource,
            ))
            .unwrap();
        let capability = client
            .handle::<TestCapability>(resource_ref(
                "db",
                "db_pool",
                ResourceSemantic::CapabilityResource,
            ))
            .unwrap();
        let state = client
            .handle::<TestState>(resource_ref(
                "state",
                "text_buffer",
                ResourceSemantic::CowVersionedState,
            ))
            .unwrap();

        assert_semantic_mismatch(client.write_plan(&frozen, "fail", json!({"replace": "all"})));
        assert_semantic_mismatch(client.write_plan(&snapshot, "fail", json!({"replace": "all"})));
        assert_semantic_mismatch(client.write_plan(&fact, "fail", json!({"replace": "all"})));
        assert_semantic_mismatch(client.write_plan(&stream, "fail", json!({"replace": "all"})));
        assert_semantic_mismatch(client.write_plan(&capability, "fail", json!({"replace": "all"})));

        assert_semantic_mismatch(client.stream_plan(&frozen));
        assert_semantic_mismatch(client.stream_plan(&snapshot));
        assert_semantic_mismatch(client.stream_plan(&fact));
        assert_semantic_mismatch(client.stream_plan(&state));
        assert_semantic_mismatch(client.stream_plan(&capability));

        assert_semantic_mismatch(client.command_plan(&frozen, "query", json!({}), None));
        assert_semantic_mismatch(client.command_plan(&snapshot, "query", json!({}), None));
        assert_semantic_mismatch(client.command_plan(&fact, "query", json!({}), None));
        assert_semantic_mismatch(client.command_plan(&state, "query", json!({}), None));
        assert_semantic_mismatch(client.command_plan(&stream, "query", json!({}), None));

        assert_semantic_mismatch(client.export_plan(&capability, "json"));
        assert_semantic_mismatch(client.export_plan(&stream, "json"));
    }

    fn assert_semantic_mismatch<T>(result: RuntimeResult<T>) {
        match result {
            Ok(_) => panic!("expected resource.semantic_mismatch"),
            Err(error) => assert_eq!(error.error().code, "resource.semantic_mismatch"),
        }
    }

    fn resource_ref(slot_id: &str, kind_id: &str, semantic: ResourceSemantic) -> ResourceRef {
        let ref_id = mutsuki_runtime_contracts::RefId::from(format!("resource:{slot_id}"));
        ResourceRef {
            resource_id: ResourceId {
                kind_id: kind_id.into(),
                slot_id: ref_id.to_string(),
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
