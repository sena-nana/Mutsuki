use mutsuki_runtime_contracts::{CommandPlan, ResourceRef};
use mutsuki_runtime_core::{CoreRuntime, RuntimeResult};
use mutsuki_runtime_sdk::ResourceProviderGateway;

use crate::commands::{HostRuntimeCommand, HostRuntimeReply};
use crate::error::{resource_provider_missing, resource_provider_unsupported};
use crate::host::HostRuntimeConfig;

pub(crate) fn handle_resource_command(
    command: HostRuntimeCommand,
    core: &mut CoreRuntime,
    config: &HostRuntimeConfig,
) -> RuntimeResult<HostRuntimeReply> {
    match command {
        HostRuntimeCommand::CreateBlobResource {
            provider_id,
            schema,
            bytes,
        } => {
            let descriptor = require_resource_provider(config, &provider_id)?
                .create_blob_resource(&schema, bytes)?;
            validate_created_provider(&provider_id, &descriptor)?;
            let descriptor = core.register_resource_descriptor(descriptor)?;
            Ok(HostRuntimeReply::ResourceCreated(descriptor))
        }
        HostRuntimeCommand::CreateCowStateResource {
            provider_id,
            kind_id,
            schema,
            bytes,
        } => {
            let descriptor = require_resource_provider(config, &provider_id)?
                .create_cow_state_resource(&kind_id, &schema, bytes)?;
            validate_created_provider(&provider_id, &descriptor)?;
            let descriptor = core.register_resource_descriptor(descriptor)?;
            Ok(HostRuntimeReply::ResourceCreated(descriptor))
        }
        HostRuntimeCommand::CreateCapabilityResource {
            provider_id,
            kind_id,
            schema,
        } => {
            let descriptor = require_resource_provider(config, &provider_id)?
                .create_capability_resource(&kind_id, &schema)?;
            validate_created_provider(&provider_id, &descriptor)?;
            let descriptor = core.register_resource_descriptor(descriptor)?;
            Ok(HostRuntimeReply::ResourceCreated(descriptor))
        }
        HostRuntimeCommand::CollectReadPlan(plan) => Ok(HostRuntimeReply::ResourceBytes(
            require_resource_provider(config, &plan.resource.provider_id)?
                .collect_read_plan(&plan)?,
        )),
        HostRuntimeCommand::SnapshotReadPlan {
            plan,
            kind_id,
            schema,
        } => {
            let snapshot = require_resource_provider(config, &plan.resource.provider_id)?
                .snapshot_read_plan(&plan, &kind_id, &schema)?;
            core.sync_plan_receipt(&mutsuki_runtime_contracts::PlanReceipt {
                plan_id: format!("snapshot-receipt:{}", snapshot.snapshot_ref.ref_id),
                status: "snapshotted".into(),
                resource_ref: None,
                snapshot: Some(snapshot.clone()),
                descriptor_updates: Vec::new(),
                new_version: Some(snapshot.snapshot_ref.version),
                output: serde_json::Value::Null,
            })?;
            Ok(HostRuntimeReply::Snapshot(snapshot))
        }
        HostRuntimeCommand::OpenStreamPlan(plan) => Ok(HostRuntimeReply::StreamPlan(
            require_resource_provider(config, &plan.resource.provider_id)?
                .open_stream_plan(&plan)?,
        )),
        HostRuntimeCommand::ExecuteExportPlan(plan) => {
            let receipt = require_resource_provider(config, &plan.resource.provider_id)?
                .execute_export_plan(&plan)?;
            core.sync_plan_receipt(&receipt)?;
            Ok(HostRuntimeReply::PlanReceipt(receipt))
        }
        HostRuntimeCommand::CommitWritePlan { plan, bytes } => {
            let receipt = require_resource_provider(config, &plan.resource.provider_id)?
                .commit_write_plan(&plan, bytes)?;
            core.sync_plan_receipt(&receipt)?;
            Ok(HostRuntimeReply::PlanReceipt(receipt))
        }
        HostRuntimeCommand::ExecuteCommandPlan(plan) => {
            let receipt = require_resource_provider(config, &plan.capability.provider_id)?
                .execute_command_plan(&plan)?;
            core.sync_plan_receipt(&receipt)?;
            Ok(HostRuntimeReply::PlanReceipt(receipt))
        }
        HostRuntimeCommand::ExecuteCommandBatch(batch) => {
            let provider_id = single_command_provider(batch.commands.iter())?;
            let receipts =
                require_resource_provider(config, &provider_id)?.execute_command_batch(&batch)?;
            core.sync_plan_receipts(&receipts)?;
            Ok(HostRuntimeReply::PlanReceipts(receipts))
        }
        HostRuntimeCommand::ExecuteSagaPlan(saga) => {
            let provider_id =
                single_command_provider(saga.steps.iter().chain(saga.compensations.iter()))?;
            let receipts =
                require_resource_provider(config, &provider_id)?.execute_saga_plan(&saga)?;
            core.sync_plan_receipts(&receipts)?;
            Ok(HostRuntimeReply::PlanReceipts(receipts))
        }
        _ => unreachable!("non-resource commands stay in actor"),
    }
}

fn require_resource_provider<'a>(
    config: &'a HostRuntimeConfig,
    provider_id: &str,
) -> RuntimeResult<&'a dyn ResourceProviderGateway> {
    config
        .resource_providers
        .get(provider_id)
        .map(|provider| provider.as_ref())
        .ok_or_else(|| resource_provider_missing(provider_id))
}

fn validate_created_provider(provider_id: &str, descriptor: &ResourceRef) -> RuntimeResult<()> {
    if descriptor.provider_id == provider_id {
        return Ok(());
    }
    Err(resource_provider_unsupported(format!(
        "provider {provider_id} returned descriptor owned by {}",
        descriptor.provider_id
    )))
}

fn single_command_provider<'a>(
    commands: impl Iterator<Item = &'a CommandPlan>,
) -> RuntimeResult<String> {
    let mut provider_id = None;
    for command in commands {
        match provider_id {
            Some(existing) if existing != command.capability.provider_id => {
                return Err(resource_provider_unsupported(
                    "command collection spans multiple resource providers",
                ));
            }
            Some(_) => {}
            None => provider_id = Some(command.capability.provider_id.clone()),
        }
    }
    provider_id
        .ok_or_else(|| resource_provider_unsupported("command collection has no provider route"))
}
