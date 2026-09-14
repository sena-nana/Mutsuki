use std::sync::Arc;

use mutsuki_runtime_contracts::{CompletionBatch, RunnerDescriptor, WorkBatch};
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeResult};
use mutsuki_runtime_sdk::ResourceProviderGateway;
use mutsuki_runtime_wire::{
    CancelRunnerRequest, DisposeRunnerRequest, RunBatchRequest, WireRequest,
};

/// Transport-independent typed Runtime Wire request surface.
pub trait TypedRequestTransport: Send + Sync {
    fn request<R: WireRequest>(&self, request: &R) -> RuntimeResult<R::Response>;
}

pub struct TransportRunner<T> {
    descriptor: RunnerDescriptor,
    transport: Arc<T>,
}

impl<T> TransportRunner<T> {
    pub fn new(descriptor: RunnerDescriptor, transport: Arc<T>) -> Self {
        Self {
            descriptor,
            transport,
        }
    }
}

impl<T: TypedRequestTransport> Runner for TransportRunner<T> {
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
            runner_id: self.descriptor.runner_id.to_string(),
            ctx,
            batch,
        })
    }

    fn cancel(&mut self, invocation_id: &str) -> RuntimeResult<()> {
        self.transport.request(&CancelRunnerRequest {
            runner_id: self.descriptor.runner_id.to_string(),
            invocation_id: invocation_id.into(),
        })
    }

    fn dispose(&mut self) -> RuntimeResult<()> {
        self.transport.request(&DisposeRunnerRequest {
            runner_id: self.descriptor.runner_id.to_string(),
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

impl<T: TypedRequestTransport> ResourceProviderGateway for TransportResourceProvider<T> {
    fn execute(
        &self,
        operation: mutsuki_runtime_sdk::ResourceProviderRequest,
    ) -> mutsuki_runtime_sdk::ResourceProviderOutcome<mutsuki_runtime_sdk::ResourceProviderReply>
    {
        match self
            .transport
            .request(&mutsuki_runtime_wire::ExecuteResourceProviderRequest {
                provider_id: self.provider_id.clone(),
                operation,
            }) {
            Ok(response) => mutsuki_runtime_sdk::ResourceProviderOutcome {
                result: response
                    .result
                    .map_err(mutsuki_runtime_core::RuntimeFailure::new),
                invalidations: response.invalidations,
            },
            Err(error) => mutsuki_runtime_sdk::ResourceProviderOutcome::new(Err(error)),
        }
    }
    // The ABI does not expose concrete provider locks. Conservative ordering preserves lifecycle effects.
    fn ordering(&self) -> mutsuki_runtime_sdk::ResourceProviderOrdering {
        mutsuki_runtime_sdk::ResourceProviderOrdering::Ordered
    }
    fn execution(&self) -> mutsuki_runtime_sdk::ResourceProviderExecution {
        mutsuki_runtime_sdk::ResourceProviderExecution::Offloaded
    }
}
