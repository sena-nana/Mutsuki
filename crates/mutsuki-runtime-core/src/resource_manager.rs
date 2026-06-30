use std::collections::HashMap;

use mutsuki_runtime_contracts::{
    ERR_CAPABILITY_EXHAUSTED, ERR_RESOURCE_NOT_FOUND, ResourceCellRef, ResourceLease, ResourceRef,
    ResourceValue, SurfaceOccupancyHandle, ValueRef,
};
use serde_json::Value;

use crate::{RuntimeFailure, SequentialIdSource};

mod hub;
mod leases;
mod occupancy;
mod plans;
mod resources;
mod values;

use hub::ResourceHub;

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
}

impl Default for ResourceManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceManager {
    pub fn new() -> Self {
        Self {
            values: HashMap::new(),
            hub: ResourceHub::default(),
            resource_cells: HashMap::new(),
            occupancy_handles: HashMap::new(),
            id_source: SequentialIdSource::new(),
            inline_value_max_bytes: 4096,
        }
    }
}

fn resource_not_found(route: String) -> RuntimeFailure {
    crate::runtime_failure(ERR_RESOURCE_NOT_FOUND, "runtime.resource_manager", route)
}

fn capability_exhausted(route: String) -> RuntimeFailure {
    crate::runtime_failure(ERR_CAPABILITY_EXHAUSTED, "runtime.resource_manager", route)
}

fn simple_hash(bytes: &[u8]) -> String {
    let sum = bytes
        .iter()
        .fold(0u64, |acc, byte| acc.wrapping_add(*byte as u64));
    format!("sum:{sum}:len:{}", bytes.len())
}
