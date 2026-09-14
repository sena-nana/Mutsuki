use crate::commands::{HostRuntimeCommand, HostRuntimeReply};
use crate::error::{resource_provider_missing, resource_provider_unsupported};
use crate::host::HostRuntimeConfig;
use mutsuki_runtime_contracts::{CommandPlan, ResourceRef};
use mutsuki_runtime_core::{CoreRuntime, RuntimeResult};
use mutsuki_runtime_sdk::{
    ResourceProviderGateway, ResourceProviderOutcome, ResourceProviderReply as R,
    ResourceProviderRequest as Q,
};

pub type ResourceCommandFuture = std::pin::Pin<
    Box<
        dyn std::future::Future<Output = ResourceProviderOutcome<HostRuntimeReply>>
            + Send
            + 'static,
    >,
>;

fn request(command: HostRuntimeCommand) -> RuntimeResult<Q> {
    Ok(match command {
        HostRuntimeCommand::CreateBlobResource { schema, bytes, .. } => {
            Q::CreateBlob { schema, bytes }
        }
        HostRuntimeCommand::CreateCowStateResource {
            kind_id,
            schema,
            bytes,
            ..
        } => Q::CreateCow {
            kind_id,
            schema,
            bytes,
        },
        HostRuntimeCommand::CreateCapabilityResource {
            kind_id, schema, ..
        } => Q::CreateCapability { kind_id, schema },
        HostRuntimeCommand::CollectReadPlan(plan) => Q::Collect(*plan),
        HostRuntimeCommand::SnapshotReadPlan {
            plan,
            kind_id,
            schema,
        } => Q::Snapshot {
            plan: *plan,
            kind_id,
            schema,
        },
        HostRuntimeCommand::OpenStreamPlan(plan) => Q::OpenStream(*plan),
        HostRuntimeCommand::ExecuteExportPlan(plan) => Q::Export(*plan),
        HostRuntimeCommand::CommitWritePlan { plan, bytes } => Q::Commit { plan, bytes },
        HostRuntimeCommand::ExecuteCommandPlan(plan) => Q::Command(*plan),
        HostRuntimeCommand::ExecuteCommandBatch(batch) => Q::Batch(*batch),
        HostRuntimeCommand::ExecuteSagaPlan(saga) => Q::Saga(*saga),
        _ => return Err(resource_provider_unsupported("not a resource command")),
    })
}

fn reply(value: R) -> HostRuntimeReply {
    match value {
        R::Created(value) => HostRuntimeReply::ResourceCreated(value),
        R::Bytes(value) => HostRuntimeReply::ResourceBytes(value),
        R::Snapshot(value) => HostRuntimeReply::Snapshot(*value),
        R::Stream(value) => HostRuntimeReply::StreamPlan(value),
        R::Receipt(value) => HostRuntimeReply::PlanReceipt(*value),
        R::Receipts(value) => HostRuntimeReply::PlanReceipts(value),
    }
}

pub(crate) fn handle_resource_command(
    command: HostRuntimeCommand,
    core: &mut CoreRuntime,
    config: &HostRuntimeConfig,
) -> RuntimeResult<HostRuntimeReply> {
    let id = resource_command_provider(&command)
        .ok_or_else(|| resource_provider_unsupported("missing provider route"))?;
    let provider = config
        .resource_providers
        .get(&id)
        .ok_or_else(|| resource_provider_missing(&id))?;
    apply_resource_outcome(core, &id, provider.execute(request(command)?).map(reply))
}

/// The only provider result application path; called by the actor, never by a worker.
pub(crate) fn apply_resource_outcome(
    core: &mut CoreRuntime,
    provider_id: &str,
    outcome: ResourceProviderOutcome<HostRuntimeReply>,
) -> RuntimeResult<HostRuntimeReply> {
    core.invalidate_resource_descriptors(provider_id, &outcome.invalidations)?;
    let mut value = outcome.result?;
    // Index only this outcome: a large batch must not scan all deletions for
    // every descriptor, and no deletion history survives actor application.
    let invalidated: std::collections::HashSet<_> = outcome
        .invalidations
        .iter()
        .map(|item| (&item.ref_id, item.generation))
        .collect();
    let removed = |descriptor: &ResourceRef| {
        descriptor.provider_id == provider_id
            && invalidated.contains(&(&descriptor.ref_id, descriptor.generation))
    };
    let clean_receipt = |receipt: &mut mutsuki_runtime_contracts::PlanReceipt| {
        if receipt.resource_ref.as_ref().is_some_and(&removed) {
            receipt.resource_ref = None;
        }
        if receipt
            .snapshot
            .as_ref()
            .is_some_and(|s| removed(&s.snapshot_ref))
        {
            receipt.snapshot = None;
        }
        receipt.descriptor_updates.retain(|d| !removed(d));
    };
    match &mut value {
        HostRuntimeReply::PlanReceipt(receipt) => clean_receipt(receipt),
        HostRuntimeReply::PlanReceipts(receipts) => receipts.iter_mut().for_each(clean_receipt),
        HostRuntimeReply::ResourceCreated(descriptor) if removed(descriptor) => {
            return Err(mutsuki_runtime_core::RuntimeFailure::new(
                mutsuki_runtime_contracts::RuntimeError::new(
                    mutsuki_runtime_contracts::ERR_RESOURCE_NOT_FOUND,
                    "runtime.resource_provider",
                    descriptor.ref_id.to_string(),
                ),
            ));
        }
        _ => {}
    }
    if let HostRuntimeReply::Snapshot(snapshot) = &value
        && removed(&snapshot.snapshot_ref)
    {
        return Err(mutsuki_runtime_core::RuntimeFailure::new(
            mutsuki_runtime_contracts::RuntimeError::new(
                mutsuki_runtime_contracts::ERR_RESOURCE_NOT_FOUND,
                "runtime.resource_provider",
                snapshot.snapshot_ref.ref_id.to_string(),
            ),
        ));
    }
    // Validate every descriptor against the captured provider route before syncing.
    let validate = |d: &ResourceRef| validate_created_provider(provider_id, d);
    match &value {
        HostRuntimeReply::ResourceCreated(d) => validate(d)?,
        HostRuntimeReply::Snapshot(s) => validate(&s.snapshot_ref)?,
        HostRuntimeReply::PlanReceipt(r) => validate_receipt(r, &validate)?,
        HostRuntimeReply::PlanReceipts(rs) => {
            for r in rs {
                validate_receipt(r, &validate)?;
            }
        }
        _ => {}
    }
    sync_resource_reply(core, &value)?;
    Ok(value)
}

fn validate_receipt(
    receipt: &mutsuki_runtime_contracts::PlanReceipt,
    validate: &impl Fn(&ResourceRef) -> RuntimeResult<()>,
) -> RuntimeResult<()> {
    if let Some(d) = &receipt.resource_ref {
        validate(d)?;
    }
    if let Some(s) = &receipt.snapshot {
        validate(&s.snapshot_ref)?;
    }
    for d in &receipt.descriptor_updates {
        validate(d)?;
    }
    Ok(())
}

pub(crate) fn resource_command_provider(command: &HostRuntimeCommand) -> Option<String> {
    match command {
        HostRuntimeCommand::CreateBlobResource { provider_id, .. }
        | HostRuntimeCommand::CreateCowStateResource { provider_id, .. }
        | HostRuntimeCommand::CreateCapabilityResource { provider_id, .. } => {
            Some(provider_id.clone())
        }
        HostRuntimeCommand::CollectReadPlan(plan)
        | HostRuntimeCommand::SnapshotReadPlan { plan, .. }
        | HostRuntimeCommand::OpenStreamPlan(plan) => Some(plan.resource.provider_id.clone()),
        HostRuntimeCommand::ExecuteExportPlan(plan) => Some(plan.resource.provider_id.clone()),
        HostRuntimeCommand::CommitWritePlan { plan, .. } => Some(plan.resource.provider_id.clone()),
        HostRuntimeCommand::ExecuteCommandPlan(plan) => Some(plan.capability.provider_id.clone()),
        HostRuntimeCommand::ExecuteCommandBatch(batch) => {
            single_command_provider(batch.commands.iter()).ok()
        }
        HostRuntimeCommand::ExecuteSagaPlan(saga) => {
            single_command_provider(saga.steps.iter().chain(saga.compensations.iter())).ok()
        }
        _ => None,
    }
}

pub(crate) fn payload_bytes(command: &HostRuntimeCommand) -> usize {
    fn size(value: &impl serde::Serialize) -> usize {
        struct ByteCount(usize);
        impl std::io::Write for ByteCount {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0 = self.0.saturating_add(bytes.len());
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut count = ByteCount(0);
        serde_json::to_writer(&mut count, value).map_or(usize::MAX, |()| count.0)
    }
    match command {
        HostRuntimeCommand::CreateBlobResource {
            bytes,
            schema,
            provider_id,
        } => bytes
            .len()
            .saturating_add(schema.len())
            .saturating_add(provider_id.len()),
        HostRuntimeCommand::CreateCowStateResource {
            bytes,
            schema,
            kind_id,
            provider_id,
        } => bytes
            .len()
            .saturating_add(schema.len())
            .saturating_add(kind_id.len())
            .saturating_add(provider_id.len()),
        HostRuntimeCommand::CreateCapabilityResource {
            schema,
            kind_id,
            provider_id,
        } => schema
            .len()
            .saturating_add(kind_id.len())
            .saturating_add(provider_id.len()),
        HostRuntimeCommand::CollectReadPlan(p) | HostRuntimeCommand::OpenStreamPlan(p) => size(p),
        HostRuntimeCommand::SnapshotReadPlan {
            plan,
            kind_id,
            schema,
        } => size(plan)
            .saturating_add(kind_id.len())
            .saturating_add(schema.len()),
        HostRuntimeCommand::CommitWritePlan { plan, bytes } => {
            size(plan).saturating_add(bytes.len())
        }
        HostRuntimeCommand::ExecuteExportPlan(p) => size(p),
        HostRuntimeCommand::ExecuteCommandPlan(p) => size(p),
        HostRuntimeCommand::ExecuteCommandBatch(p) => size(p),
        HostRuntimeCommand::ExecuteSagaPlan(p) => size(p),
        _ => 0,
    }
}

fn prepare_offloaded_resource_command(
    command: HostRuntimeCommand,
    provider: std::sync::Arc<dyn ResourceProviderGateway>,
) -> (ResourceCommandFuture, usize) {
    let bytes = payload_bytes(&command);
    (
        Box::pin(async move {
            // A panic propagates to the executor, which poisons the ordered lane.
            match tokio::task::spawn_blocking(move || match request(command) {
                Ok(request) => provider.execute(request).map(reply),
                Err(error) => ResourceProviderOutcome::new(Err(error)),
            })
            .await
            {
                Ok(outcome) => outcome,
                Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
                Err(error) => ResourceProviderOutcome::new(Err(crate::error::host_failure(
                    "host.resource.join",
                    error.to_string(),
                ))),
            }
        }),
        bytes,
    )
}

pub(crate) fn prepare_resource_command(
    command: HostRuntimeCommand,
    config: &HostRuntimeConfig,
) -> RuntimeResult<(String, ResourceCommandFuture, usize)> {
    let id = resource_command_provider(&command)
        .ok_or_else(|| resource_provider_unsupported("missing provider route"))?;
    if let Some(provider) = config.resource_providers.get(&id) {
        let (future, bytes) = prepare_offloaded_resource_command(command, provider.clone());
        return Ok((id, future, bytes));
    }
    let bytes = payload_bytes(&command);
    let provider = config
        .async_resource_providers
        .get(&id)
        .cloned()
        .ok_or_else(|| resource_provider_missing(&id))?;
    let request = request(command)?;
    // Even Future construction belongs behind executor admission and panic
    // isolation: execute() itself may perform work before returning its Future.
    Ok((
        id,
        Box::pin(async move { provider.execute(request).await.map(reply) }),
        bytes,
    ))
}

pub(crate) fn sync_resource_reply(
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
        // An offloaded create built the descriptor on the executor; registering
        // it is a Core mutation, so it lands here on the actor thread. The
        // registry returns the descriptor unchanged, which is what the caller
        // already has in this reply.
        HostRuntimeReply::ResourceCreated(descriptor) => core
            .register_resource_descriptor(descriptor.clone())
            .map(|_| ()),
        _ => Ok(()),
    }
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

pub(crate) fn single_command_provider<'a>(
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
