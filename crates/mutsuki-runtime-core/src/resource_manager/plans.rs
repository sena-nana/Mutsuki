use mutsuki_runtime_contracts::{
    ERR_RESOURCE_GENERATION_MISMATCH, ERR_RESOURCE_NOT_FOUND, PatchDescriptor, PlanReceipt,
    ReadPlan, ResourceSemantic, RuntimeError, SnapshotDescriptor, StreamPlan, WritePlan,
};
use serde_json::Value;

use crate::{RuntimeFailure, RuntimeResult};

use super::{ResourceManager, simple_hash};

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

    pub fn collect_read_plan(&self, plan: &ReadPlan) -> RuntimeResult<Vec<u8>> {
        self.read_resource(&plan.resource)
    }

    pub fn snapshot_read_plan(
        &mut self,
        plan: &ReadPlan,
        kind_id: &str,
        schema: &str,
    ) -> RuntimeResult<SnapshotDescriptor> {
        let bytes = self.collect_read_plan(plan)?;
        let snapshot_ref = self.create_snapshot_resource(kind_id, schema, &plan.resource, bytes)?;
        let source = self.open_resource(&plan.resource.ref_id)?;
        Ok(SnapshotDescriptor {
            snapshot_ref,
            source_version: source.version,
            snapshot_version: 1,
            source_ref: source,
            is_stale: false,
            is_latest: true,
        })
    }

    pub fn open_stream_plan(&self, plan: &ReadPlan) -> RuntimeResult<StreamPlan> {
        if plan.resource.semantic != ResourceSemantic::StreamResource {
            return Err(RuntimeFailure::new(RuntimeError::new(
                "resource.semantic_mismatch",
                "runtime.resource_manager",
                format!("resource.stream_plan.{}", plan.resource.ref_id),
            )));
        }
        Ok(StreamPlan {
            plan_id: format!("stream-plan:{}", plan.resource.ref_id),
            resource: plan.resource.clone(),
            operation: "open_stream".into(),
            args: Value::Null,
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

    pub fn commit_write_plan(
        &mut self,
        plan: &WritePlan,
        bytes: Vec<u8>,
    ) -> RuntimeResult<PlanReceipt> {
        let entry = self.hub.get_mut(&plan.resource.ref_id).ok_or_else(|| {
            RuntimeFailure::new(RuntimeError::new(
                ERR_RESOURCE_NOT_FOUND,
                "runtime.resource_manager",
                format!("resource.plan.commit.{}", plan.resource.ref_id),
            ))
        })?;
        if entry.descriptor.semantic != ResourceSemantic::CowVersionedState {
            return Err(RuntimeFailure::new(RuntimeError::new(
                "resource.semantic_mismatch",
                "runtime.resource_manager",
                format!("resource.plan.commit.{}", plan.resource.ref_id),
            )));
        }
        if entry.descriptor.version != plan.base_version
            || entry.descriptor.generation != plan.resource.generation
        {
            return Err(RuntimeFailure::new(RuntimeError::new(
                ERR_RESOURCE_GENERATION_MISMATCH,
                "runtime.resource_manager",
                format!("resource.plan.commit.{}", plan.resource.ref_id),
            )));
        }
        entry.descriptor.generation += 1;
        entry.descriptor.version += 1;
        entry.descriptor.resource_id.generation = entry.descriptor.generation;
        entry.descriptor.resource_id.version = entry.descriptor.version;
        entry.descriptor.size_hint = Some(bytes.len() as u64);
        entry.descriptor.content_hash = Some(simple_hash(&bytes));
        self.backend.write(&mut entry.descriptor, &bytes)?;
        entry.bytes = bytes;
        Ok(PlanReceipt {
            plan_id: plan.plan_id.clone(),
            status: "committed".into(),
            resource_ref: Some(entry.descriptor.clone()),
            snapshot: None,
            new_version: Some(entry.descriptor.version),
            output: Value::Null,
        })
    }
}
