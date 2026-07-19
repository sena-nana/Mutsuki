use mutsuki_runtime_contracts::{CommandPlan, ResourceRef};
use mutsuki_runtime_core::{CoreRuntime, RuntimeResult};
use mutsuki_runtime_sdk::{BoxRuntimeFuture, ResourceProviderGateway};

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
            let descriptor = if let Some(provider) = config.resource_providers.get(&provider_id) {
                provider.create_blob_resource(&schema, bytes)?
            } else if let Some(provider) = config.async_resource_providers.get(&provider_id) {
                provider.create_blob_resource(&schema, bytes)?
            } else {
                return Err(resource_provider_missing(&provider_id));
            };
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
            let descriptor = if let Some(provider) = config.resource_providers.get(&provider_id) {
                provider.create_cow_state_resource(&kind_id, &schema, bytes)?
            } else if let Some(provider) = config.async_resource_providers.get(&provider_id) {
                provider.create_cow_state_resource(&kind_id, &schema, bytes)?
            } else {
                return Err(resource_provider_missing(&provider_id));
            };
            validate_created_provider(&provider_id, &descriptor)?;
            let descriptor = core.register_resource_descriptor(descriptor)?;
            Ok(HostRuntimeReply::ResourceCreated(descriptor))
        }
        HostRuntimeCommand::CreateCapabilityResource {
            provider_id,
            kind_id,
            schema,
        } => {
            let descriptor = if let Some(provider) = config.resource_providers.get(&provider_id) {
                provider.create_capability_resource(&kind_id, &schema)?
            } else if let Some(provider) = config.async_resource_providers.get(&provider_id) {
                provider.create_capability_resource(&kind_id, &schema)?
            } else {
                return Err(resource_provider_missing(&provider_id));
            };
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

pub(crate) fn sync_async_resource_reply(
    core: &mut CoreRuntime,
    reply: &HostRuntimeReply,
) -> RuntimeResult<()> {
    match reply {
        HostRuntimeReply::PlanReceipt(receipt) => core.sync_plan_receipt(receipt).map(|_| ()),
        HostRuntimeReply::PlanReceipts(receipts) => core.sync_plan_receipts(receipts).map(|_| ()),
        HostRuntimeReply::Snapshot(snapshot) => core
            .sync_plan_receipt(&mutsuki_runtime_contracts::PlanReceipt {
                plan_id: format!("snapshot-receipt:{}", snapshot.snapshot_ref.ref_id),
                status: "snapshotted".into(),
                resource_ref: None,
                snapshot: Some(snapshot.clone()),
                descriptor_updates: Vec::new(),
                new_version: Some(snapshot.snapshot_ref.version),
                output: serde_json::Value::Null,
            })
            .map(|_| ()),
        _ => Ok(()),
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

pub(crate) fn prepare_async_resource_command(
    command: HostRuntimeCommand,
    config: &HostRuntimeConfig,
) -> RuntimeResult<(String, BoxRuntimeFuture<HostRuntimeReply>, usize)> {
    match command {
        HostRuntimeCommand::CollectReadPlan(plan) => {
            let payload_bytes = serialized_payload_bytes(&*plan)?;
            let provider_id = plan.resource.provider_id.clone();
            let provider = require_async_resource_provider(config, &provider_id)?;
            Ok((
                provider_id,
                Box::pin(async move {
                    provider
                        .collect_read_plan(*plan)
                        .await
                        .map(HostRuntimeReply::ResourceBytes)
                }),
                payload_bytes,
            ))
        }
        HostRuntimeCommand::SnapshotReadPlan {
            plan,
            kind_id,
            schema,
        } => {
            let payload_bytes = serialized_payload_bytes(&(&*plan, &kind_id, &schema))?;
            let provider_id = plan.resource.provider_id.clone();
            let provider = require_async_resource_provider(config, &provider_id)?;
            Ok((
                provider_id,
                Box::pin(async move {
                    provider
                        .snapshot_read_plan(*plan, kind_id, schema)
                        .await
                        .map(HostRuntimeReply::Snapshot)
                }),
                payload_bytes,
            ))
        }
        HostRuntimeCommand::OpenStreamPlan(plan) => {
            let payload_bytes = serialized_payload_bytes(&*plan)?;
            let provider_id = plan.resource.provider_id.clone();
            let provider = require_async_resource_provider(config, &provider_id)?;
            Ok((
                provider_id,
                Box::pin(async move {
                    provider
                        .open_stream_plan(*plan)
                        .await
                        .map(HostRuntimeReply::StreamPlan)
                }),
                payload_bytes,
            ))
        }
        HostRuntimeCommand::ExecuteExportPlan(plan) => {
            let payload_bytes = serialized_payload_bytes(&*plan)?;
            let provider_id = plan.resource.provider_id.clone();
            let provider = require_async_resource_provider(config, &provider_id)?;
            Ok((
                provider_id,
                Box::pin(async move {
                    provider
                        .execute_export_plan(*plan)
                        .await
                        .map(HostRuntimeReply::PlanReceipt)
                }),
                payload_bytes,
            ))
        }
        HostRuntimeCommand::CommitWritePlan { plan, bytes } => {
            let payload_bytes = serialized_payload_bytes(&*plan)?.saturating_add(bytes.len());
            let provider_id = plan.resource.provider_id.clone();
            let provider = require_async_resource_provider(config, &provider_id)?;
            Ok((
                provider_id,
                Box::pin(async move {
                    provider
                        .commit_write_plan(*plan, bytes)
                        .await
                        .map(HostRuntimeReply::PlanReceipt)
                }),
                payload_bytes,
            ))
        }
        HostRuntimeCommand::ExecuteCommandPlan(plan) => {
            let payload_bytes = serialized_payload_bytes(&*plan)?;
            let provider_id = plan.capability.provider_id.clone();
            let provider = require_async_resource_provider(config, &provider_id)?;
            Ok((
                provider_id,
                Box::pin(async move {
                    provider
                        .execute_command_plan(*plan)
                        .await
                        .map(HostRuntimeReply::PlanReceipt)
                }),
                payload_bytes,
            ))
        }
        HostRuntimeCommand::ExecuteCommandBatch(batch) => {
            let payload_bytes = serialized_payload_bytes(&*batch)?;
            let provider_id = single_command_provider(batch.commands.iter())?;
            let provider = require_async_resource_provider(config, &provider_id)?;
            Ok((
                provider_id,
                Box::pin(async move {
                    provider
                        .execute_command_batch(*batch)
                        .await
                        .map(HostRuntimeReply::PlanReceipts)
                }),
                payload_bytes,
            ))
        }
        HostRuntimeCommand::ExecuteSagaPlan(saga) => {
            let payload_bytes = serialized_payload_bytes(&*saga)?;
            let provider_id =
                single_command_provider(saga.steps.iter().chain(saga.compensations.iter()))?;
            let provider = require_async_resource_provider(config, &provider_id)?;
            Ok((
                provider_id,
                Box::pin(async move {
                    provider
                        .execute_saga_plan(*saga)
                        .await
                        .map(HostRuntimeReply::PlanReceipts)
                }),
                payload_bytes,
            ))
        }
        _ => Err(resource_provider_unsupported(
            "command is not an async resource plan operation",
        )),
    }
}

fn serialized_payload_bytes(value: &impl serde::Serialize) -> RuntimeResult<usize> {
    serde_json::to_vec(value)
        .map(|payload| payload.len().max(1))
        .map_err(|error| {
            resource_provider_unsupported(format!(
                "async resource payload cannot be measured: {error}"
            ))
        })
}

fn require_async_resource_provider(
    config: &HostRuntimeConfig,
    provider_id: &str,
) -> RuntimeResult<std::sync::Arc<dyn mutsuki_runtime_sdk::AsyncResourceProviderGateway>> {
    config
        .async_resource_providers
        .get(provider_id)
        .cloned()
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
