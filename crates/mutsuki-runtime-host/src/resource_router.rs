use mutsuki_runtime_core::{CoreRuntime, RuntimeResult};
use mutsuki_runtime_sdk::ResourceProviderGateway;

use crate::commands::{HostRuntimeCommand, HostRuntimeReply};
use crate::error::resource_provider_missing;
use crate::host::HostRuntimeConfig;

pub(crate) fn handle_resource_command(
    command: HostRuntimeCommand,
    core: &mut CoreRuntime,
    config: &HostRuntimeConfig,
) -> RuntimeResult<HostRuntimeReply> {
    match command {
        HostRuntimeCommand::CreateBlobResource { schema, bytes } => {
            let descriptor =
                require_resource_provider(config)?.create_blob_resource(&schema, bytes)?;
            let descriptor = core.register_resource_descriptor(descriptor)?;
            Ok(HostRuntimeReply::ResourceCreated(descriptor))
        }
        HostRuntimeCommand::CreateCowStateResource {
            kind_id,
            schema,
            bytes,
        } => {
            let descriptor = require_resource_provider(config)?
                .create_cow_state_resource(&kind_id, &schema, bytes)?;
            let descriptor = core.register_resource_descriptor(descriptor)?;
            Ok(HostRuntimeReply::ResourceCreated(descriptor))
        }
        HostRuntimeCommand::CreateCapabilityResource { kind_id, schema } => {
            let descriptor =
                require_resource_provider(config)?.create_capability_resource(&kind_id, &schema)?;
            let descriptor = core.register_resource_descriptor(descriptor)?;
            Ok(HostRuntimeReply::ResourceCreated(descriptor))
        }
        HostRuntimeCommand::CollectReadPlan(plan) => Ok(HostRuntimeReply::ResourceBytes(
            require_resource_provider(config)?.collect_read_plan(&plan)?,
        )),
        HostRuntimeCommand::SnapshotReadPlan {
            plan,
            kind_id,
            schema,
        } => Ok(HostRuntimeReply::Snapshot(
            require_resource_provider(config)?.snapshot_read_plan(&plan, &kind_id, &schema)?,
        )),
        HostRuntimeCommand::OpenStreamPlan(plan) => Ok(HostRuntimeReply::StreamPlan(
            require_resource_provider(config)?.open_stream_plan(&plan)?,
        )),
        HostRuntimeCommand::ExecuteExportPlan(plan) => Ok(HostRuntimeReply::PlanReceipt(
            require_resource_provider(config)?.execute_export_plan(&plan)?,
        )),
        HostRuntimeCommand::CommitWritePlan { plan, bytes } => {
            let receipt = require_resource_provider(config)?.commit_write_plan(&plan, bytes)?;
            core.sync_plan_receipt(&receipt)?;
            Ok(HostRuntimeReply::PlanReceipt(receipt))
        }
        HostRuntimeCommand::ExecuteCommandPlan(plan) => {
            let receipt = require_resource_provider(config)?.execute_command_plan(&plan)?;
            core.sync_plan_receipt(&receipt)?;
            Ok(HostRuntimeReply::PlanReceipt(receipt))
        }
        HostRuntimeCommand::ExecuteCommandBatch(batch) => {
            let receipts = require_resource_provider(config)?.execute_command_batch(&batch)?;
            core.sync_plan_receipts(&receipts)?;
            Ok(HostRuntimeReply::PlanReceipts(receipts))
        }
        HostRuntimeCommand::ExecuteSagaPlan(saga) => {
            let receipts = require_resource_provider(config)?.execute_saga_plan(&saga)?;
            core.sync_plan_receipts(&receipts)?;
            Ok(HostRuntimeReply::PlanReceipts(receipts))
        }
        _ => unreachable!("non-resource commands stay in actor"),
    }
}

fn require_resource_provider(
    config: &HostRuntimeConfig,
) -> RuntimeResult<&dyn ResourceProviderGateway> {
    config
        .resource_provider
        .as_deref()
        .ok_or_else(|| resource_provider_missing("host.resource_provider"))
}
