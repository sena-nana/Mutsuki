use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

use mutsuki_runtime_contracts::resource::experimental::{CommandBatch, SagaPlan};
use mutsuki_runtime_contracts::{
    CommandPlan, ExportPlan, PlanReceipt, ReadPlan, SnapshotDescriptor, StreamPlan, TaskBatch,
    TaskHandle, TaskOutcome, WritePlan,
};
use mutsuki_runtime_core::{RuntimeFailure, RuntimeResult};
use mutsuki_runtime_sdk::{
    ConfigProvider, HostContext as SdkHostContext, HostServiceRegistry, NoopEventBridge,
    ResourcePlanGateway, ShutdownController, StaticConfigProvider, TaskSubmitter,
};

use crate::actor::CoreActorMsg;
use crate::capabilities::HostCapabilityRegistry;
use crate::commands::{HostRuntimeCommand, HostRuntimeReply};
use crate::error::host_failure;

pub(crate) fn build_host_context(
    tx: mpsc::Sender<CoreActorMsg>,
    capabilities: Arc<HostCapabilityRegistry>,
    services: Arc<HostServiceRegistry>,
    profile_id: String,
    registry_generation: u64,
) -> SdkHostContext {
    let command_client = Arc::new(ActorCommandClient { tx: tx.clone() });
    let task_submitter: Arc<dyn TaskSubmitter> = command_client.clone();
    let resource_gateway: Arc<dyn ResourcePlanGateway> = command_client;
    let shutdown = Arc::new(ActorShutdownController {
        tx,
        requested: AtomicBool::new(false),
    });
    let config_provider: Arc<dyn ConfigProvider> = Arc::new(StaticConfigProvider::empty());
    SdkHostContext::new(
        "mutsuki.host",
        profile_id,
        registry_generation,
        capabilities,
        services,
        config_provider,
        Arc::new(NoopEventBridge),
        task_submitter,
        resource_gateway,
        shutdown,
    )
}

struct ActorCommandClient {
    tx: mpsc::Sender<CoreActorMsg>,
}

impl ActorCommandClient {
    fn dispatch(&self, command: HostRuntimeCommand) -> RuntimeResult<HostRuntimeReply> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx
            .send(CoreActorMsg::Command(command, reply_tx))
            .map_err(|error| host_failure("host.actor.command", error.to_string()))?;
        reply_rx
            .recv()
            .map_err(|error| host_failure("host.actor.reply", error.to_string()))?
    }
}

impl TaskSubmitter for ActorCommandClient {
    fn submit_batch(&self, batch: TaskBatch) -> RuntimeResult<Vec<TaskHandle>> {
        match self.dispatch(HostRuntimeCommand::SubmitBatch(Box::new(batch)))? {
            HostRuntimeReply::TaskBatchSubmitted(handles) => Ok(handles),
            reply => Err(unexpected_reply("task.submit_batch", reply)),
        }
    }

    fn cancel_task(&self, handle: &TaskHandle) -> RuntimeResult<()> {
        match self.dispatch(HostRuntimeCommand::CancelTask(handle.clone()))? {
            HostRuntimeReply::TaskCancelled(_) => Ok(()),
            reply => Err(unexpected_reply("task.cancel", reply)),
        }
    }

    fn task_outcome(&self, handle: &TaskHandle) -> RuntimeResult<Option<TaskOutcome>> {
        match self.dispatch(HostRuntimeCommand::TaskOutcome(handle.clone()))? {
            HostRuntimeReply::TaskOutcome(outcome) => Ok(outcome),
            reply => Err(unexpected_reply("task.outcome", reply)),
        }
    }
}

impl ResourcePlanGateway for ActorCommandClient {
    fn collect_read_plan(&self, plan: &ReadPlan) -> RuntimeResult<Vec<u8>> {
        match self.dispatch(HostRuntimeCommand::CollectReadPlan(Box::new(plan.clone())))? {
            HostRuntimeReply::ResourceBytes(bytes) => Ok(bytes),
            reply => Err(unexpected_reply("resource.read.collect", reply)),
        }
    }

    fn snapshot_read_plan(
        &self,
        plan: &ReadPlan,
        kind_id: &str,
        schema: &str,
    ) -> RuntimeResult<SnapshotDescriptor> {
        match self.dispatch(HostRuntimeCommand::SnapshotReadPlan {
            plan: Box::new(plan.clone()),
            kind_id: kind_id.into(),
            schema: schema.into(),
        })? {
            HostRuntimeReply::Snapshot(snapshot) => Ok(snapshot),
            reply => Err(unexpected_reply("resource.read.snapshot", reply)),
        }
    }

    fn open_stream_plan(&self, plan: &ReadPlan) -> RuntimeResult<StreamPlan> {
        match self.dispatch(HostRuntimeCommand::OpenStreamPlan(Box::new(plan.clone())))? {
            HostRuntimeReply::StreamPlan(stream) => Ok(stream),
            reply => Err(unexpected_reply("resource.stream.open", reply)),
        }
    }

    fn execute_export_plan(&self, plan: &ExportPlan) -> RuntimeResult<PlanReceipt> {
        match self.dispatch(HostRuntimeCommand::ExecuteExportPlan(Box::new(
            plan.clone(),
        )))? {
            HostRuntimeReply::PlanReceipt(receipt) => Ok(receipt),
            reply => Err(unexpected_reply("resource.export", reply)),
        }
    }

    fn commit_write_plan(&self, plan: &WritePlan, bytes: Vec<u8>) -> RuntimeResult<PlanReceipt> {
        match self.dispatch(HostRuntimeCommand::CommitWritePlan {
            plan: Box::new(plan.clone()),
            bytes,
        })? {
            HostRuntimeReply::PlanReceipt(receipt) => Ok(receipt),
            reply => Err(unexpected_reply("resource.write.commit", reply)),
        }
    }

    fn execute_command_plan(&self, plan: &CommandPlan) -> RuntimeResult<PlanReceipt> {
        match self.dispatch(HostRuntimeCommand::ExecuteCommandPlan(Box::new(
            plan.clone(),
        )))? {
            HostRuntimeReply::PlanReceipt(receipt) => Ok(receipt),
            reply => Err(unexpected_reply("resource.command", reply)),
        }
    }

    fn execute_command_batch(&self, batch: &CommandBatch) -> RuntimeResult<Vec<PlanReceipt>> {
        match self.dispatch(HostRuntimeCommand::ExecuteCommandBatch(Box::new(
            batch.clone(),
        )))? {
            HostRuntimeReply::PlanReceipts(receipts) => Ok(receipts),
            reply => Err(unexpected_reply("resource.command_batch", reply)),
        }
    }

    fn execute_saga_plan(&self, saga: &SagaPlan) -> RuntimeResult<Vec<PlanReceipt>> {
        match self.dispatch(HostRuntimeCommand::ExecuteSagaPlan(Box::new(saga.clone())))? {
            HostRuntimeReply::PlanReceipts(receipts) => Ok(receipts),
            reply => Err(unexpected_reply("resource.saga", reply)),
        }
    }
}

struct ActorShutdownController {
    tx: mpsc::Sender<CoreActorMsg>,
    requested: AtomicBool,
}

impl ShutdownController for ActorShutdownController {
    fn is_shutdown_requested(&self) -> bool {
        self.requested.load(Ordering::SeqCst)
    }

    fn request_shutdown(&self, _reason: &str) -> RuntimeResult<()> {
        self.requested.store(true, Ordering::SeqCst);
        self.tx
            .send(CoreActorMsg::Shutdown)
            .map_err(|error| host_failure("host.actor.shutdown", error.to_string()))
    }
}

fn unexpected_reply(route: &str, reply: HostRuntimeReply) -> RuntimeFailure {
    host_failure(route, format!("unexpected reply: {reply:?}"))
}
