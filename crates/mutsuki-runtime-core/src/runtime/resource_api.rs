use mutsuki_runtime_contracts::{
    CommandBatch, CommandPlan, ExclusiveWriteLease, ExportPlan, PlanReceipt, ReadPlan,
    ResourceCellRef, ResourceLease, ResourceRef, SagaPlan, SnapshotDescriptor, StreamPlan,
    SurfaceOccupancyHandle, WritePlan,
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

    pub fn create_blob_resource(
        &mut self,
        schema: &str,
        bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        self.ensure_local_resource_surface(schema)?;
        Ok(self.resources.create_blob_resource(schema, bytes))
    }

    pub fn create_mmap_resource(
        &mut self,
        schema: &str,
        bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        self.ensure_local_resource_surface(schema)?;
        self.resources.create_mmap_resource(schema, bytes)
    }

    pub fn create_cow_state_resource(
        &mut self,
        kind_id: &str,
        schema: &str,
        bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        self.ensure_local_resource_surface(schema)?;
        self.resources
            .create_cow_state_resource(kind_id, schema, bytes)
    }

    pub fn create_capability_resource(
        &mut self,
        kind_id: &str,
        schema: &str,
    ) -> RuntimeResult<ResourceRef> {
        self.ensure_local_resource_surface(schema)?;
        Ok(self.resources.create_capability_resource(kind_id, schema))
    }

    pub fn build_read_plan(&self, ref_id: &str, operation: &str) -> RuntimeResult<ReadPlan> {
        self.resources.build_read_plan(ref_id, operation)
    }

    pub fn build_export_plan(&self, ref_id: &str, target: &str) -> RuntimeResult<ExportPlan> {
        self.resources.build_export_plan(ref_id, target)
    }

    pub fn collect_read_plan(&self, plan: &ReadPlan) -> RuntimeResult<Vec<u8>> {
        self.resources.collect_read_plan(plan)
    }

    pub fn snapshot_read_plan(
        &mut self,
        plan: &ReadPlan,
        kind_id: &str,
        schema: &str,
    ) -> RuntimeResult<SnapshotDescriptor> {
        self.resources.snapshot_read_plan(plan, kind_id, schema)
    }

    pub fn open_stream_plan(&self, plan: &ReadPlan) -> RuntimeResult<StreamPlan> {
        self.resources.open_stream_plan(plan)
    }

    pub fn execute_export_plan(&self, plan: &ExportPlan) -> RuntimeResult<PlanReceipt> {
        self.resources.execute_export_plan(plan)
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

    pub fn commit_write_plan(
        &mut self,
        plan: &WritePlan,
        bytes: Vec<u8>,
    ) -> RuntimeResult<PlanReceipt> {
        self.resources.commit_write_plan(plan, bytes)
    }

    pub fn execute_command_plan(&self, plan: &CommandPlan) -> RuntimeResult<PlanReceipt> {
        self.resources.execute_command_plan(plan)
    }

    pub fn execute_command_batch(&self, batch: &CommandBatch) -> RuntimeResult<Vec<PlanReceipt>> {
        self.resources.execute_command_batch(batch)
    }

    pub fn execute_saga_plan(&self, saga: &SagaPlan) -> RuntimeResult<Vec<PlanReceipt>> {
        self.resources.execute_saga_plan(saga)
    }

    pub fn open_resource(&self, ref_id: &str) -> RuntimeResult<ResourceRef> {
        self.resources.open_resource(ref_id)
    }

    pub fn read_resource(&self, ref_id: &str) -> RuntimeResult<Vec<u8>> {
        self.resources.read_resource_by_id(ref_id)
    }

    pub fn map_resource(&self, ref_id: &str) -> RuntimeResult<ResourceRef> {
        self.resources.map_resource(ref_id)
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

    pub fn write_resource(
        &mut self,
        lease: &ExclusiveWriteLease,
        bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        self.resources
            .write_with_lease(lease, bytes, self.current_step)
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
        self.resources.release_resource_lease(lease)
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

    fn ensure_local_resource_surface(&self, schema: &str) -> RuntimeResult<()> {
        self.ensure_resource_surfaces_not_deprecated(
            schema,
            Some("resource.local"),
            "runtime.resource_manager",
        )
    }
}
