use std::sync::Arc;

use mutsuki_runtime_contracts::resource::experimental::{CommandBatch, SagaPlan};
use mutsuki_runtime_contracts::{
    CommandPlan, CompletionBatch, ExportPlan, PlanReceipt, ReadPlan, ResourceRef, RunnerDescriptor,
    SnapshotDescriptor, StreamPlan, WorkBatch, WritePlan,
};
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeResult};
use mutsuki_runtime_sdk::{ResourcePlanGateway, ResourceProviderGateway};
use mutsuki_runtime_wire::{
    CancelRunnerRequest, CollectReadPlanRequest, CommandBatchRequest, CommandPlanRequest,
    CommitWritePlanRequest, CreateBlobRequest, CreateCapabilityRequest, CreateCowStateRequest,
    DisposeRunnerRequest, ExportPlanRequest, OpenStreamPlanRequest, RunBatchRequest,
    SagaPlanRequest, SnapshotReadPlanRequest, WireRequest,
};

/// Transport-independent typed Runtime Wire request surface.
pub trait TypedRequestTransport: Send + Sync {
    fn request<R: WireRequest>(&self, request: &R) -> RuntimeResult<R::Response>;
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

impl<T: TypedRequestTransport> Runner for TransportJsonlRunner<T> {
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
        self.transport.request(&RunBatchRequest {
            runner_id: self.descriptor.runner_id.clone(),
            ctx,
            batch,
        })
    }

    fn cancel(&mut self, invocation_id: &str) -> RuntimeResult<()> {
        self.transport.request(&CancelRunnerRequest {
            runner_id: self.descriptor.runner_id.clone(),
            invocation_id: invocation_id.into(),
        })
    }

    fn dispose(&mut self) -> RuntimeResult<()> {
        self.transport.request(&DisposeRunnerRequest {
            runner_id: self.descriptor.runner_id.clone(),
        })
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
}

impl<T: TypedRequestTransport> ResourcePlanGateway for TransportResourceProvider<T> {
    fn collect_read_plan(&self, plan: &ReadPlan) -> RuntimeResult<Vec<u8>> {
        self.transport.request(&CollectReadPlanRequest {
            provider_id: Some(self.provider_id.clone()),
            plan: plan.clone(),
        })
    }

    fn snapshot_read_plan(
        &self,
        plan: &ReadPlan,
        kind_id: &str,
        schema: &str,
    ) -> RuntimeResult<SnapshotDescriptor> {
        self.transport.request(&SnapshotReadPlanRequest {
            provider_id: Some(self.provider_id.clone()),
            plan: plan.clone(),
            kind_id: kind_id.into(),
            schema: schema.into(),
        })
    }

    fn open_stream_plan(&self, plan: &ReadPlan) -> RuntimeResult<StreamPlan> {
        self.transport.request(&OpenStreamPlanRequest {
            provider_id: Some(self.provider_id.clone()),
            plan: plan.clone(),
        })
    }

    fn execute_export_plan(&self, plan: &ExportPlan) -> RuntimeResult<PlanReceipt> {
        self.transport.request(&ExportPlanRequest {
            provider_id: Some(self.provider_id.clone()),
            plan: plan.clone(),
        })
    }

    fn commit_write_plan(&self, plan: &WritePlan, bytes: Vec<u8>) -> RuntimeResult<PlanReceipt> {
        self.transport.request(&CommitWritePlanRequest {
            provider_id: Some(self.provider_id.clone()),
            plan: plan.clone(),
            bytes,
        })
    }

    fn execute_command_plan(&self, plan: &CommandPlan) -> RuntimeResult<PlanReceipt> {
        self.transport.request(&CommandPlanRequest {
            provider_id: Some(self.provider_id.clone()),
            plan: plan.clone(),
        })
    }

    fn execute_command_batch(&self, batch: &CommandBatch) -> RuntimeResult<Vec<PlanReceipt>> {
        self.transport.request(&CommandBatchRequest {
            provider_id: Some(self.provider_id.clone()),
            batch: batch.clone(),
        })
    }

    fn execute_saga_plan(&self, saga: &SagaPlan) -> RuntimeResult<Vec<PlanReceipt>> {
        self.transport.request(&SagaPlanRequest {
            provider_id: Some(self.provider_id.clone()),
            saga: saga.clone(),
        })
    }
}

impl<T: TypedRequestTransport> ResourceProviderGateway for TransportResourceProvider<T> {
    fn create_blob_resource(&self, schema: &str, bytes: Vec<u8>) -> RuntimeResult<ResourceRef> {
        self.transport.request(&CreateBlobRequest {
            provider_id: Some(self.provider_id.clone()),
            schema: schema.into(),
            bytes,
        })
    }

    fn create_cow_state_resource(
        &self,
        kind_id: &str,
        schema: &str,
        bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        self.transport.request(&CreateCowStateRequest {
            provider_id: Some(self.provider_id.clone()),
            kind_id: kind_id.into(),
            schema: schema.into(),
            bytes,
        })
    }

    fn create_capability_resource(
        &self,
        kind_id: &str,
        schema: &str,
    ) -> RuntimeResult<ResourceRef> {
        self.transport.request(&CreateCapabilityRequest {
            provider_id: Some(self.provider_id.clone()),
            kind_id: kind_id.into(),
            schema: schema.into(),
        })
    }
}
