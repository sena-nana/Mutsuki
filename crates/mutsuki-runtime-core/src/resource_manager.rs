use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use mutsuki_runtime_contracts::{
    ERR_CAPABILITY_EXHAUSTED, ERR_RESOURCE_NOT_FOUND, ResourceCellRef, ResourceLease, ResourceRef,
    ResourceValue, SurfaceOccupancyHandle, ValueRef,
};
use serde_json::Value;

use crate::{RuntimeFailure, SequentialIdSource};

mod backend;
mod hub;
mod leases;
mod occupancy;
mod plans;
mod resources;
mod values;

use backend::LocalResourceBackend;
use hub::ResourceHub;

static NEXT_MANAGER_NAMESPACE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq)]
pub enum PackedValue {
    Inline(ResourceValue),
    Value(ValueRef),
    Resource(ResourceRef),
}

#[derive(Clone, Debug)]
struct ResourceCellEntry {
    descriptor: ResourceCellRef,
    active_leases: HashMap<String, ResourceLease>,
}

impl ResourceCellEntry {
    fn new(descriptor: ResourceCellRef) -> Self {
        Self {
            descriptor,
            active_leases: HashMap::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ResourceManager {
    values: HashMap<String, (ValueRef, Value)>,
    hub: ResourceHub,
    resource_cells: HashMap<String, ResourceCellEntry>,
    occupancy_handles: HashMap<String, SurfaceOccupancyHandle>,
    id_source: SequentialIdSource,
    inline_value_max_bytes: usize,
    backend: LocalResourceBackend,
}

impl Default for ResourceManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceManager {
    pub fn new() -> Self {
        let namespace = NEXT_MANAGER_NAMESPACE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir()
            .join("mutsuki-resource-manager")
            .join(format!("manager-{namespace}"));
        Self {
            values: HashMap::new(),
            hub: ResourceHub::default(),
            resource_cells: HashMap::new(),
            occupancy_handles: HashMap::new(),
            id_source: SequentialIdSource::new(),
            inline_value_max_bytes: 4096,
            backend: LocalResourceBackend::new(root),
        }
    }
}

fn resource_not_found(route: String) -> RuntimeFailure {
    runtime_failure!(ERR_RESOURCE_NOT_FOUND, "runtime.resource_manager", route)
}

fn capability_exhausted(route: String) -> RuntimeFailure {
    runtime_failure!(ERR_CAPABILITY_EXHAUSTED, "runtime.resource_manager", route)
}

fn io_failure(err: std::io::Error) -> RuntimeFailure {
    runtime_failure!(
        "resource.io_failed",
        "runtime.resource_manager",
        err.to_string()
    )
}

fn simple_hash(bytes: &[u8]) -> String {
    let sum = bytes
        .iter()
        .fold(0u64, |acc, byte| acc.wrapping_add(*byte as u64));
    format!("sum:{sum}:len:{}", bytes.len())
}
