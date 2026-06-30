use mutsuki_runtime_contracts::{
    CommandPlan, ExportPlan, PatchDescriptor, ReadPlan, ResourceSemantic, StreamPlan, WritePlan,
};
use serde_json::Value;

use crate::RuntimeResult;

use super::ResourceManager;

impl ResourceManager {
    pub fn build_read_plan(&self, ref_id: &str, operation: &str) -> RuntimeResult<ReadPlan> {
        let resource = self.open_resource(ref_id)?;
        Ok(ReadPlan {
            plan_id: format!("read-plan:{ref_id}:{operation}"),
            resource,
            operation: operation.into(),
            args: Value::Null,
        })
    }

    pub fn open_stream_plan(&self, plan: &ReadPlan) -> RuntimeResult<StreamPlan> {
        if plan.resource.semantic != ResourceSemantic::StreamResource {
            return Err(crate::runtime_failure(
                "resource.semantic_mismatch",
                "runtime.resource_manager",
                format!("resource.stream_plan.{}", plan.resource.ref_id),
            ));
        }
        Ok(StreamPlan {
            plan_id: format!("stream-plan:{}", plan.resource.ref_id),
            resource: plan.resource.clone(),
            operation: "open_stream".into(),
            args: Value::Null,
        })
    }

    pub fn build_export_plan(&self, ref_id: &str, target: &str) -> RuntimeResult<ExportPlan> {
        let resource = self.open_resource(ref_id)?;
        Ok(ExportPlan {
            plan_id: format!("export-plan:{ref_id}:{target}"),
            resource,
            target: target.into(),
            args: Value::Null,
        })
    }

    pub fn build_command_plan(
        &self,
        ref_id: &str,
        operation: &str,
        args: Value,
        idempotency_key: Option<String>,
    ) -> RuntimeResult<CommandPlan> {
        let capability = self.open_resource(ref_id)?;
        Ok(CommandPlan {
            plan_id: format!("command-plan:{ref_id}:{operation}"),
            capability,
            operation: operation.into(),
            args,
            idempotency_key,
        })
    }

    pub fn build_write_plan(
        &self,
        ref_id: &str,
        conflict_policy: &str,
        operations: Value,
    ) -> RuntimeResult<WritePlan> {
        let resource = self.open_resource(ref_id)?;
        let patch = PatchDescriptor {
            patch_id: format!("patch:{ref_id}:{}", resource.version),
            target_ref: resource.clone(),
            base_version: resource.version,
            conflict_policy: conflict_policy.into(),
            operations,
        };
        Ok(WritePlan {
            plan_id: format!("write-plan:{ref_id}:{}", patch.base_version),
            resource,
            base_version: patch.base_version,
            conflict_policy: patch.conflict_policy.clone(),
            patch,
            returning: None,
        })
    }
}
