use std::collections::BTreeMap;

use mutsuki_runtime_contracts::SurfaceOccupancy;

use super::CoreRuntime;

impl CoreRuntime {
    pub fn surface_occupancy(&self) -> Vec<SurfaceOccupancy> {
        merge_occupancy(
            self.tasks.surface_occupancy(),
            self.resources.surface_occupancy(&self.surfaces),
        )
    }
}

fn merge_occupancy(
    task_occupancy: Vec<SurfaceOccupancy>,
    resource_occupancy: Vec<SurfaceOccupancy>,
) -> Vec<SurfaceOccupancy> {
    let mut by_surface: BTreeMap<String, SurfaceOccupancy> = BTreeMap::new();
    for item in task_occupancy.into_iter().chain(resource_occupancy) {
        let entry = by_surface
            .entry(item.surface_id.clone())
            .or_insert_with(|| zero_occupancy(&item.surface_id));
        entry.ready_tasks += item.ready_tasks;
        entry.running_invocations += item.running_invocations;
        entry.resource_refs += item.resource_refs;
        entry.state_refs += item.state_refs;
        entry.active_leases += item.active_leases;
        entry.open_streams += item.open_streams;
        entry.subscriptions += item.subscriptions;
        entry.timers += item.timers;
        entry.effect_inflight += item.effect_inflight;
    }
    by_surface.into_values().collect()
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
