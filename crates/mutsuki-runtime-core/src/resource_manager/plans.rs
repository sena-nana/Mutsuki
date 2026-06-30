use mutsuki_runtime_contracts::{
    CommandBatch, CommandPlan, ERR_RESOURCE_GENERATION_MISMATCH, ERR_RESOURCE_NOT_FOUND,
    ExportPlan, PatchDescriptor, PlanReceipt, ReadPlan, ResourceSemantic, RuntimeError, SagaPlan,
    ScalarValue, SnapshotDescriptor, StreamPlan, WritePlan,
};
use serde_json::{Value, json};

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

    pub fn execute_export_plan(&self, plan: &ExportPlan) -> RuntimeResult<PlanReceipt> {
        if plan.target != "inline_utf8" {
            return Err(resource_plan_error(
                "resource.export_unsupported",
                format!("resource.plan.export.{}", plan.resource.ref_id),
                [("target", ScalarValue::String(plan.target.clone()))],
            ));
        }

        let bytes = self.read_resource(&plan.resource)?;
        let output = String::from_utf8(bytes).map_err(|err| {
            resource_plan_error(
                "resource.export_decode_failed",
                format!("resource.plan.export.{}", plan.resource.ref_id),
                [
                    ("target", ScalarValue::String(plan.target.clone())),
                    ("exception_repr", ScalarValue::String(err.to_string())),
                ],
            )
        })?;
        Ok(PlanReceipt {
            plan_id: plan.plan_id.clone(),
            status: "exported".into(),
            resource_ref: Some(self.open_resource(&plan.resource.ref_id)?),
            snapshot: None,
            new_version: None,
            output: Value::String(output),
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

    pub fn execute_command_plan(&self, plan: &CommandPlan) -> RuntimeResult<PlanReceipt> {
        self.read_resource(&plan.capability)?;
        let capability = self.open_resource(&plan.capability.ref_id)?;
        if capability.semantic != ResourceSemantic::CapabilityResource {
            return Err(resource_plan_error(
                "resource.semantic_mismatch",
                format!("resource.plan.command.{}", plan.capability.ref_id),
                [],
            ));
        }
        if plan.operation != "query" {
            return Err(resource_plan_error(
                "resource.command_unsupported",
                format!("resource.plan.command.{}", plan.capability.ref_id),
                [("operation", ScalarValue::String(plan.operation.clone()))],
            ));
        }
        Ok(PlanReceipt {
            plan_id: plan.plan_id.clone(),
            status: "commanded".into(),
            resource_ref: Some(capability.clone()),
            snapshot: None,
            new_version: None,
            output: json!({
                "capability_ref": capability.ref_id.clone(),
                "resource_kind": capability.resource_kind.clone(),
                "provider_id": capability.provider_id.clone(),
                "operation": plan.operation.clone(),
                "idempotency_key": plan.idempotency_key.clone(),
                "args": plan.args.clone(),
            }),
        })
    }

    pub fn execute_command_batch(&self, batch: &CommandBatch) -> RuntimeResult<Vec<PlanReceipt>> {
        if batch.rollback_guarantee {
            return Err(resource_plan_error(
                "resource.rollback_unsupported",
                format!("resource.plan.batch.{}", batch.batch_id),
                [],
            ));
        }
        batch
            .commands
            .iter()
            .map(|command| self.execute_command_plan(command))
            .collect()
    }

    pub fn execute_saga_plan(&self, saga: &SagaPlan) -> RuntimeResult<Vec<PlanReceipt>> {
        let mut receipts = Vec::new();
        for (step_index, command) in saga.steps.iter().enumerate() {
            match self.execute_command_plan(command) {
                Ok(receipt) => receipts.push(receipt),
                Err(failure) => {
                    let compensation_attempts = saga.compensations.len() as i64;
                    let compensation_failures = saga
                        .compensations
                        .iter()
                        .rev()
                        .filter(|compensation| self.execute_command_plan(compensation).is_err())
                        .count() as i64;
                    let mut error = resource_plan_error(
                        "resource.saga_failed",
                        format!("resource.plan.saga.{}", saga.saga_id),
                        [
                            ("failed_step_index", ScalarValue::Int(step_index as i64)),
                            (
                                "compensation_attempts",
                                ScalarValue::Int(compensation_attempts),
                            ),
                            (
                                "compensation_failures",
                                ScalarValue::Int(compensation_failures),
                            ),
                        ],
                    );
                    error.0.cause = Some(Box::new(failure.error().clone()));
                    return Err(error);
                }
            }
        }
        Ok(receipts)
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
            crate::runtime_failure(
                ERR_RESOURCE_NOT_FOUND,
                "runtime.resource_manager",
                format!("resource.plan.commit.{}", plan.resource.ref_id),
            )
        })?;
        if entry.descriptor.semantic != ResourceSemantic::CowVersionedState {
            return Err(crate::runtime_failure(
                "resource.semantic_mismatch",
                "runtime.resource_manager",
                format!("resource.plan.commit.{}", plan.resource.ref_id),
            ));
        }
        if entry.descriptor.version != plan.base_version
            || entry.descriptor.generation != plan.resource.generation
        {
            return Err(crate::runtime_failure(
                ERR_RESOURCE_GENERATION_MISMATCH,
                "runtime.resource_manager",
                format!("resource.plan.commit.{}", plan.resource.ref_id),
            ));
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

fn resource_plan_error<const N: usize>(
    code: &str,
    route: String,
    evidence: [(&str, ScalarValue); N],
) -> RuntimeFailure {
    let mut error = RuntimeError::new(code, "runtime.resource_manager", route);
    for (key, value) in evidence {
        error.evidence.insert(key.into(), value);
    }
    RuntimeFailure::new(error)
}
