use std::sync::Arc;

use mutsuki_runtime_contracts::resource::experimental::{CommandBatch, SagaPlan};
use mutsuki_runtime_contracts::{
    CommandPlan, CompletionBatch, ExportPlan, PlanReceipt, ReadPlan, ResourceRef, RunnerDescriptor,
    SnapshotDescriptor, StreamPlan, WorkBatch, WritePlan,
};
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeResult};
use mutsuki_runtime_sdk::{ResourcePlanGateway, ResourceProviderGateway};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

pub trait JsonRequestTransport: Send + Sync {
    fn request(&self, method: &str, params: Value) -> RuntimeResult<Value>;

    fn request_as<T: DeserializeOwned>(&self, method: &str, params: Value) -> RuntimeResult<T>
    where
        Self: Sized,
    {
        let value = self.request(method, params)?;
        serde_json::from_value(value).map_err(|error| {
            mutsuki_runtime_core::RuntimeFailure::new(mutsuki_runtime_contracts::RuntimeError::new(
                mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
                "abi.transport",
                format!("abi.decode:{error}"),
            ))
        })
    }
}

pub struct TransportJsonlRunner<T> {
    descriptor: RunnerDescriptor,
    transport: Arc<T>,
}

impl<T> TransportJsonlRunner<T> {
    pub fn new(descriptor: RunnerDescriptor, transport: Arc<T>) -> Self {
        Self {
            descriptor,
            transport,
        }
    }
}

impl<T: JsonRequestTransport> Runner for TransportJsonlRunner<T> {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        let lease_ids = batch
            .task_leases
            .iter()
            .map(|lease| lease.lease_id.clone())
            .collect::<Vec<_>>();
        if lease_ids != ctx.task_lease_ids {
            return Err(mutsuki_runtime_core::RuntimeFailure::new(
                mutsuki_runtime_contracts::RuntimeError::new(
                    mutsuki_runtime_contracts::ERR_TASK_CLAIM_CONFLICT,
                    "abi.runner",
                    format!("runner.run_batch.{}", batch.batch_id),
                ),
            ));
        }
        self.transport.request_as(
            "runner.run_batch",
            json!({ "runner_id": self.descriptor.runner_id, "ctx": ctx, "batch": batch }),
        )
    }

    fn cancel(&mut self, invocation_id: &str) -> RuntimeResult<()> {
        self.transport.request(
            "runner.cancel",
            json!({ "runner_id": self.descriptor.runner_id, "invocation_id": invocation_id }),
        )?;
        Ok(())
    }

    fn dispose(&mut self) -> RuntimeResult<()> {
        self.transport.request(
            "runner.dispose",
            json!({ "runner_id": self.descriptor.runner_id }),
        )?;
        Ok(())
    }
}

pub struct TransportResourceProvider<T> {
    provider_id: String,
    transport: Arc<T>,
}

impl<T> TransportResourceProvider<T> {
    pub fn new(provider_id: impl Into<String>, transport: Arc<T>) -> Self {
        Self {
            provider_id: provider_id.into(),
            transport,
        }
    }

    fn params(&self, mut value: Value) -> Value {
        value["provider_id"] = Value::String(self.provider_id.clone());
        value
    }
}

impl<T: JsonRequestTransport> ResourcePlanGateway for TransportResourceProvider<T> {
    fn collect_read_plan(&self, plan: &ReadPlan) -> RuntimeResult<Vec<u8>> {
        self.transport.request_as(
            "resource.read.collect",
            self.params(json!({ "plan": plan })),
        )
    }

    fn snapshot_read_plan(
        &self,
        plan: &ReadPlan,
        kind_id: &str,
        schema: &str,
    ) -> RuntimeResult<SnapshotDescriptor> {
        self.transport.request_as(
            "resource.read.snapshot",
            self.params(json!({ "plan": plan, "kind_id": kind_id, "schema": schema })),
        )
    }

    fn open_stream_plan(&self, plan: &ReadPlan) -> RuntimeResult<StreamPlan> {
        self.transport
            .request_as("resource.stream.open", self.params(json!({ "plan": plan })))
    }

    fn execute_export_plan(&self, plan: &ExportPlan) -> RuntimeResult<PlanReceipt> {
        self.transport
            .request_as("resource.export", self.params(json!({ "plan": plan })))
    }

    fn commit_write_plan(&self, plan: &WritePlan, bytes: Vec<u8>) -> RuntimeResult<PlanReceipt> {
        self.transport.request_as(
            "resource.write.commit",
            self.params(json!({ "plan": plan, "bytes": bytes })),
        )
    }

    fn execute_command_plan(&self, plan: &CommandPlan) -> RuntimeResult<PlanReceipt> {
        self.transport
            .request_as("resource.command", self.params(json!({ "plan": plan })))
    }

    fn execute_command_batch(&self, batch: &CommandBatch) -> RuntimeResult<Vec<PlanReceipt>> {
        self.transport.request_as(
            "resource.command_batch",
            self.params(json!({ "batch": batch })),
        )
    }

    fn execute_saga_plan(&self, saga: &SagaPlan) -> RuntimeResult<Vec<PlanReceipt>> {
        self.transport
            .request_as("resource.saga", self.params(json!({ "saga": saga })))
    }
}

impl<T: JsonRequestTransport> ResourceProviderGateway for TransportResourceProvider<T> {
    fn create_blob_resource(&self, schema: &str, bytes: Vec<u8>) -> RuntimeResult<ResourceRef> {
        self.transport.request_as(
            "resource.create_blob",
            self.params(json!({ "schema": schema, "bytes": bytes })),
        )
    }

    fn create_cow_state_resource(
        &self,
        kind_id: &str,
        schema: &str,
        bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        self.transport.request_as(
            "resource.create_cow_state",
            self.params(json!({ "kind_id": kind_id, "schema": schema, "bytes": bytes })),
        )
    }

    fn create_capability_resource(
        &self,
        kind_id: &str,
        schema: &str,
    ) -> RuntimeResult<ResourceRef> {
        self.transport.request_as(
            "resource.create_capability",
            self.params(json!({ "kind_id": kind_id, "schema": schema })),
        )
    }
}
