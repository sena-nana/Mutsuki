use mutsuki_runtime_contracts::{
    CommandPlan, ExclusiveWriteLease, ExportPlan, PlanReceipt, ReadPlan, ResourceCellRef,
    ResourceLease, ResourceRef, StreamPlan, SurfaceOccupancyHandle, WritePlan,
};
use serde_json::Value;

use crate::{ResourceManager, RuntimeResult};

use super::CoreRuntime;

impl CoreRuntime {
    pub fn open_stream(
        &mut self,
        stream_id: &str,
        schema: &str,
        provider_id: &str,
        endpoint: &str,
    ) -> RuntimeResult<ResourceRef> {
        let surface_id = format!("stream:{stream_id}");
        self.ensure_surface_not_deprecated(&surface_id, "runtime.resource_manager")?;
        self.ensure_resource_surfaces_not_deprecated(
            schema,
            Some(provider_id),
            "runtime.resource_manager",
        )?;
        Ok(self
            .resources
            .create_stream_resource(stream_id, schema, provider_id, endpoint))
    }

    pub fn close_stream(&mut self, ref_id: &str) -> RuntimeResult<()> {
        self.resources.close_stream_resource(ref_id)?;
        Ok(())
    }

    pub fn register_resource_descriptor(
        &mut self,
        descriptor: ResourceRef,
    ) -> RuntimeResult<ResourceRef> {
        self.ensure_resource_descriptor_not_deprecated(&descriptor)?;
        self.resources.register_resource_descriptor(descriptor)
    }

    pub fn sync_plan_receipt(&mut self, receipt: &PlanReceipt) -> RuntimeResult<Vec<ResourceRef>> {
        if let Some(resource) = &receipt.resource_ref {
            self.ensure_resource_descriptor_not_deprecated(resource)?;
        }
        if let Some(snapshot) = &receipt.snapshot {
            self.ensure_resource_descriptor_not_deprecated(&snapshot.snapshot_ref)?;
        }
        for descriptor in &receipt.descriptor_updates {
            self.ensure_resource_descriptor_not_deprecated(descriptor)?;
        }
        self.resources.sync_plan_receipt(receipt)
    }

    pub fn sync_plan_receipts(
        &mut self,
        receipts: &[PlanReceipt],
    ) -> RuntimeResult<Vec<ResourceRef>> {
        let mut synced = Vec::new();
        for receipt in receipts {
            synced.extend(self.sync_plan_receipt(receipt)?);
        }
        Ok(synced)
    }

    pub fn build_read_plan(&self, ref_id: &str, operation: &str) -> RuntimeResult<ReadPlan> {
        self.resources.build_read_plan(ref_id, operation)
    }

    pub fn build_export_plan(&self, ref_id: &str, target: &str) -> RuntimeResult<ExportPlan> {
        self.resources.build_export_plan(ref_id, target)
    }

    pub fn open_stream_plan(&self, plan: &ReadPlan) -> RuntimeResult<StreamPlan> {
        self.resources.open_stream_plan(plan)
    }

    pub fn build_command_plan(
        &self,
        ref_id: &str,
        operation: &str,
        args: Value,
        idempotency_key: Option<String>,
    ) -> RuntimeResult<CommandPlan> {
        self.resources
            .build_command_plan(ref_id, operation, args, idempotency_key)
    }

    pub fn build_write_plan(
        &self,
        ref_id: &str,
        conflict_policy: &str,
        operations: Value,
    ) -> RuntimeResult<WritePlan> {
        self.resources
            .build_write_plan(ref_id, conflict_policy, operations)
    }

    pub fn open_resource(&self, ref_id: &str) -> RuntimeResult<ResourceRef> {
        self.resources.open_resource(ref_id)
    }

    pub fn lock_resource(
        &mut self,
        ref_id: &str,
        owner: &str,
        expires_at_step: Option<u64>,
    ) -> RuntimeResult<ExclusiveWriteLease> {
        self.resources
            .acquire_write_lease(ref_id, owner, expires_at_step)
    }

    pub fn release_write_lease(&mut self, lease: &ExclusiveWriteLease) -> RuntimeResult<()> {
        self.resources
            .release_write_lease_at(lease, self.current_step)
    }

    pub fn create_resource_cell(
        &mut self,
        cell_id: &str,
        resource_kind: &str,
        owner_plugin_id: &str,
        schema: &str,
        reload_policy: &str,
    ) -> RuntimeResult<ResourceCellRef> {
        self.ensure_resource_surfaces_not_deprecated(schema, None, "runtime.resource_manager")?;
        Ok(self.resources.create_resource_cell(
            cell_id,
            resource_kind,
            owner_plugin_id,
            schema,
            reload_policy,
        ))
    }

    pub fn acquire_resource_lease(
        &mut self,
        cell_id: &str,
        borrower_task_id: &str,
        borrower_executor_id: &str,
        mode: &str,
        expires_at_step: Option<u64>,
    ) -> RuntimeResult<ResourceLease> {
        self.resources.acquire_resource_lease(
            cell_id,
            borrower_task_id,
            borrower_executor_id,
            mode,
            expires_at_step,
        )
    }

    pub fn release_resource_lease(&mut self, lease: &ResourceLease) -> RuntimeResult<()> {
        self.resources
            .release_resource_lease_at(lease, self.current_step)
    }

    pub fn reclaim_expired_resource_leases(&mut self) -> Vec<ResourceLease> {
        self.resources
            .reclaim_expired_resource_leases(self.current_step)
    }

    pub fn register_surface_occupancy(
        &mut self,
        handle: SurfaceOccupancyHandle,
    ) -> RuntimeResult<()> {
        self.ensure_surface_not_deprecated(&handle.surface_id, "runtime.resource_manager")?;
        self.resources.register_surface_occupancy(handle)
    }

    pub fn release_surface_occupancy(
        &mut self,
        handle_id: &str,
    ) -> RuntimeResult<SurfaceOccupancyHandle> {
        self.resources.release_surface_occupancy(handle_id)
    }

    pub fn resources(&self) -> &ResourceManager {
        &self.resources
    }

    pub fn resources_mut(&mut self) -> &mut ResourceManager {
        &mut self.resources
    }

    fn ensure_resource_descriptor_not_deprecated(
        &self,
        descriptor: &ResourceRef,
    ) -> RuntimeResult<()> {
        self.ensure_resource_surfaces_not_deprecated(
            &descriptor.schema,
            Some(&descriptor.provider_id),
            "runtime.resource_manager",
        )
    }
}
