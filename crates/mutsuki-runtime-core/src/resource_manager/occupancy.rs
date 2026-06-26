use std::collections::HashMap;

use mutsuki_runtime_contracts::{
    ContractSurface, ContractSurfaceKind, ResourceAccess, SurfaceOccupancy, SurfaceOccupancyHandle,
    SurfaceOccupancyHandleKind,
};

use crate::RuntimeResult;

use super::{ResourceEntry, ResourceManager, capability_exhausted, resource_not_found};

#[derive(Default)]
struct HandleCounts {
    streams: u64,
    subscriptions: u64,
    timers: u64,
}

impl ResourceManager {
    pub fn register_surface_occupancy(
        &mut self,
        handle: SurfaceOccupancyHandle,
    ) -> RuntimeResult<()> {
        if self.occupancy_handles.contains_key(&handle.handle_id) {
            return Err(capability_exhausted(format!(
                "surface.occupancy.{}",
                handle.handle_id
            )));
        }
        self.occupancy_handles
            .insert(handle.handle_id.clone(), handle);
        Ok(())
    }

    pub fn release_surface_occupancy(
        &mut self,
        handle_id: &str,
    ) -> RuntimeResult<SurfaceOccupancyHandle> {
        self.occupancy_handles
            .remove(handle_id)
            .ok_or_else(|| resource_not_found(format!("surface.occupancy.{handle_id}")))
    }

    pub fn surface_occupancy(&self, surfaces: &[ContractSurface]) -> Vec<SurfaceOccupancy> {
        let resource_counts = self.resource_surface_counts();
        let stream_counts = self.stream_surface_counts();
        let handle_counts = self.handle_surface_counts();
        let mut occupancy = Vec::new();
        for surface in surfaces {
            let mut item = zero_occupancy(&surface.surface_id);
            match surface.kind {
                ContractSurfaceKind::ResourceSchema | ContractSurfaceKind::ResourceProvider => {
                    if let Some((resource_refs, active_leases)) =
                        resource_counts.get(&surface.surface_id)
                    {
                        item.resource_refs = *resource_refs;
                        item.active_leases = *active_leases;
                    }
                }
                ContractSurfaceKind::Schema => {
                    item.resource_refs = self
                        .values
                        .values()
                        .filter(|(value_ref, _)| {
                            surface.surface_id == format!("schema:{}", value_ref.schema)
                                || surface.surface_id == value_ref.schema
                        })
                        .count() as u64;
                }
                ContractSurfaceKind::Stream => {
                    let handle_counts = handle_counts.get(&surface.surface_id);
                    item.open_streams =
                        stream_counts.get(&surface.surface_id).copied().unwrap_or(0)
                            + handle_counts.map_or(0, |counts| counts.streams);
                }
                ContractSurfaceKind::Subscription => {
                    item.subscriptions = handle_counts
                        .get(&surface.surface_id)
                        .map_or(0, |counts| counts.subscriptions);
                }
                ContractSurfaceKind::Timer => {
                    item.timers = handle_counts
                        .get(&surface.surface_id)
                        .map_or(0, |counts| counts.timers);
                }
                _ => {}
            }
            if !item.is_zero() {
                occupancy.push(item);
            }
        }
        occupancy
    }

    fn resource_surface_counts(&self) -> HashMap<String, (u64, u64)> {
        let mut counts = HashMap::new();
        for entry in self.resources.values() {
            count_resource_surface(
                &mut counts,
                &entry.descriptor.schema,
                ContractSurfaceKind::ResourceSchema,
                entry,
            );
            count_resource_surface(
                &mut counts,
                &entry.descriptor.provider_id,
                ContractSurfaceKind::ResourceProvider,
                entry,
            );
        }
        counts
    }

    fn stream_surface_counts(&self) -> HashMap<String, u64> {
        let mut counts = HashMap::new();
        for entry in self
            .resources
            .values()
            .filter(|entry| matches!(entry.descriptor.access, ResourceAccess::Stream { .. }))
        {
            increment_count(&mut counts, entry.descriptor.resource_kind.clone());
            increment_count(
                &mut counts,
                format!("stream:{}", entry.descriptor.resource_kind),
            );
        }
        counts
    }

    fn handle_surface_counts(&self) -> HashMap<String, HandleCounts> {
        let mut counts: HashMap<String, HandleCounts> = HashMap::new();
        for handle in self.occupancy_handles.values() {
            let item = counts.entry(handle.surface_id.clone()).or_default();
            match handle.kind {
                SurfaceOccupancyHandleKind::Stream => item.streams += 1,
                SurfaceOccupancyHandleKind::Subscription => item.subscriptions += 1,
                SurfaceOccupancyHandleKind::Timer => item.timers += 1,
            }
        }
        counts
    }
}

fn count_resource_surface(
    counts: &mut HashMap<String, (u64, u64)>,
    value: &str,
    kind: ContractSurfaceKind,
    entry: &ResourceEntry,
) {
    let prefix = match kind {
        ContractSurfaceKind::ResourceSchema => "resource_schema",
        ContractSurfaceKind::ResourceProvider => "resource_provider",
        _ => return,
    };
    increment_resource_count(counts, value.to_string(), entry);
    increment_resource_count(counts, format!("{prefix}:{value}"), entry);
}

fn increment_resource_count(
    counts: &mut HashMap<String, (u64, u64)>,
    surface_id: String,
    entry: &ResourceEntry,
) {
    let item = counts.entry(surface_id).or_insert((0, 0));
    item.0 += 1;
    if entry.writer.is_some() {
        item.1 += 1;
    }
}

fn increment_count(counts: &mut HashMap<String, u64>, key: String) {
    *counts.entry(key).or_insert(0) += 1;
}

fn zero_occupancy(surface_id: &str) -> SurfaceOccupancy {
    SurfaceOccupancy {
        surface_id: surface_id.into(),
        ready_tasks: 0,
        running_invocations: 0,
        resource_refs: 0,
        state_refs: 0,
        active_leases: 0,
        open_streams: 0,
        subscriptions: 0,
        timers: 0,
        effect_inflight: 0,
    }
}
