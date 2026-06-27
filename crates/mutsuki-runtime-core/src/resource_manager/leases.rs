use mutsuki_runtime_contracts::{
    ERR_CAPABILITY_EXHAUSTED, ERR_RESOURCE_GENERATION_MISMATCH, ERR_RESOURCE_LEASE_EXPIRED,
    ERR_RESOURCE_NOT_FOUND, ExclusiveWriteLease, LeaseToken, ResourceCellRef, ResourceLease,
    ResourceRef, RuntimeError,
};

use crate::{IdSource, RuntimeFailure, RuntimeResult};

use super::{ResourceManager, simple_hash};

impl ResourceManager {
    pub fn create_resource_cell(
        &mut self,
        cell_id: &str,
        resource_kind: &str,
        owner_plugin_id: &str,
        schema: &str,
        reload_policy: &str,
    ) -> ResourceCellRef {
        let descriptor = ResourceCellRef {
            cell_id: cell_id.into(),
            resource_kind: resource_kind.into(),
            owner_plugin_id: owner_plugin_id.into(),
            schema: schema.into(),
            generation: 1,
            health: "healthy".into(),
            reload_policy: reload_policy.into(),
        };
        self.resource_cells.insert(
            descriptor.cell_id.clone(),
            super::ResourceCellEntry::new(descriptor.clone()),
        );
        descriptor
    }

    pub fn acquire_resource_lease(
        &mut self,
        cell_id: &str,
        borrower_task_id: &str,
        borrower_executor_id: &str,
        mode: &str,
        expires_at_step: Option<u64>,
    ) -> RuntimeResult<ResourceLease> {
        let cell = self.resource_cells.get_mut(cell_id).ok_or_else(|| {
            RuntimeFailure::new(RuntimeError::new(
                ERR_RESOURCE_NOT_FOUND,
                "runtime.resource_manager",
                format!("resource_cell.lease.{cell_id}"),
            ))
        })?;
        if mode == "exclusive" && !cell.active_leases.is_empty() {
            return Err(RuntimeFailure::new(RuntimeError::new(
                ERR_CAPABILITY_EXHAUSTED,
                "runtime.resource_manager",
                format!("resource_cell.lease.{cell_id}"),
            )));
        }
        if mode != "shared" && mode != "exclusive" {
            return Err(RuntimeFailure::new(RuntimeError::new(
                "resource.lease_mode_invalid",
                "runtime.resource_manager",
                format!("resource_cell.lease.{cell_id}.{mode}"),
            )));
        }
        if mode == "shared"
            && cell
                .active_leases
                .values()
                .any(|lease| lease.mode == "exclusive")
        {
            return Err(RuntimeFailure::new(RuntimeError::new(
                ERR_CAPABILITY_EXHAUSTED,
                "runtime.resource_manager",
                format!("resource_cell.lease.{cell_id}"),
            )));
        }
        let lease = ResourceLease {
            lease_id: self.id_source.next_id("resource-lease"),
            cell_id: cell_id.into(),
            borrower_task_id: borrower_task_id.into(),
            borrower_executor_id: borrower_executor_id.into(),
            mode: mode.into(),
            expires_at_step,
            generation: cell.descriptor.generation,
        };
        cell.active_leases
            .insert(lease.lease_id.clone(), lease.clone());
        Ok(lease)
    }

    pub fn release_resource_lease(&mut self, lease: &ResourceLease) -> RuntimeResult<()> {
        let cell = self.resource_cells.get_mut(&lease.cell_id).ok_or_else(|| {
            RuntimeFailure::new(RuntimeError::new(
                ERR_RESOURCE_NOT_FOUND,
                "runtime.resource_manager",
                format!("resource_cell.release.{}", lease.cell_id),
            ))
        })?;
        if cell.active_leases.remove(&lease.lease_id).is_none() {
            return Err(RuntimeFailure::new(RuntimeError::new(
                ERR_RESOURCE_NOT_FOUND,
                "runtime.resource_manager",
                format!("resource_cell.release.{}", lease.lease_id),
            )));
        }
        Ok(())
    }

    pub fn active_mutable_lease_routes_for_task(&self, task_id: &str) -> Vec<String> {
        let mut routes = Vec::new();
        for cell in self.resource_cells.values() {
            for lease in cell.active_leases.values() {
                if lease.borrower_task_id == task_id && lease.mode == "exclusive" {
                    routes.push(format!("resource_cell.lease.{}", lease.lease_id));
                }
            }
        }
        for entry in self.resources.values() {
            if let Some(writer) = &entry.writer
                && writer.owner == task_id
            {
                routes.push(format!("resource.write_lease.{}", writer.token_id));
            }
        }
        routes.sort();
        routes
    }

    pub fn acquire_write_lease(
        &mut self,
        ref_id: &str,
        owner: &str,
        expires_at_step: Option<u64>,
    ) -> RuntimeResult<ExclusiveWriteLease> {
        let entry = self.resources.get_mut(ref_id).ok_or_else(|| {
            RuntimeFailure::new(RuntimeError::new(
                ERR_RESOURCE_NOT_FOUND,
                "runtime.resource_manager",
                format!("resource.lease.{ref_id}"),
            ))
        })?;
        if entry.writer.is_some() {
            return Err(RuntimeFailure::new(RuntimeError::new(
                ERR_CAPABILITY_EXHAUSTED,
                "runtime.resource_manager",
                format!("resource.lease.{ref_id}"),
            )));
        }
        let token = LeaseToken {
            token_id: self.id_source.next_id("lease"),
            ref_id: ref_id.into(),
            owner: owner.into(),
            mode: "exclusive_write".into(),
            expires_at_step,
            generation: entry.descriptor.generation,
        };
        entry.writer = Some(token.clone());
        Ok(ExclusiveWriteLease { token })
    }

    pub fn write_with_lease(
        &mut self,
        lease: &ExclusiveWriteLease,
        bytes: Vec<u8>,
        current_step: u64,
    ) -> RuntimeResult<ResourceRef> {
        if lease
            .token
            .expires_at_step
            .is_some_and(|expires| current_step > expires)
        {
            return Err(RuntimeFailure::new(RuntimeError::new(
                ERR_RESOURCE_LEASE_EXPIRED,
                "runtime.resource_manager",
                format!("resource.write.{}", lease.token.ref_id),
            )));
        }
        let entry = self.resources.get_mut(&lease.token.ref_id).ok_or_else(|| {
            RuntimeFailure::new(RuntimeError::new(
                ERR_RESOURCE_NOT_FOUND,
                "runtime.resource_manager",
                format!("resource.write.{}", lease.token.ref_id),
            ))
        })?;
        if entry.writer.as_ref() != Some(&lease.token) {
            return Err(RuntimeFailure::new(RuntimeError::new(
                ERR_RESOURCE_GENERATION_MISMATCH,
                "runtime.resource_manager",
                format!("resource.write.{}", lease.token.ref_id),
            )));
        }
        entry.descriptor.generation += 1;
        entry.descriptor.version += 1;
        entry.descriptor.size_hint = Some(bytes.len() as u64);
        entry.descriptor.content_hash = Some(simple_hash(&bytes));
        self.backend.write(&mut entry.descriptor, &bytes)?;
        entry.bytes = bytes;
        entry.writer = None;
        Ok(entry.descriptor.clone())
    }
}
