use std::collections::{BTreeMap, HashMap};

use mutsuki_runtime_contracts::{
    ERR_CAPABILITY_EXHAUSTED, ERR_RESOURCE_GENERATION_MISMATCH, ERR_RESOURCE_NOT_FOUND,
    PlanReceipt, ResourceCellRef, ResourceLease, ResourceRef, ResourceValue,
    SurfaceOccupancyHandle, ValueRef,
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

fn resource_generation_mismatch(route: String) -> RuntimeFailure {
    crate::runtime_failure(
        ERR_RESOURCE_GENERATION_MISMATCH,
        "runtime.resource_manager",
        route,
    )
}

fn capability_exhausted(route: String) -> RuntimeFailure {
    crate::runtime_failure(ERR_CAPABILITY_EXHAUSTED, "runtime.resource_manager", route)
}

fn receipt_descriptors(receipt: &PlanReceipt) -> Vec<ResourceRef> {
    let mut descriptors = BTreeMap::<String, ResourceRef>::new();
    if let Some(resource) = &receipt.resource_ref {
        merge_descriptor(&mut descriptors, resource.clone());
    }
    if let Some(snapshot) = &receipt.snapshot {
        merge_descriptor(&mut descriptors, snapshot.snapshot_ref.clone());
    }
    for descriptor in &receipt.descriptor_updates {
        merge_descriptor(&mut descriptors, descriptor.clone());
    }
    descriptors.into_values().collect()
}

fn merge_descriptor(descriptors: &mut BTreeMap<String, ResourceRef>, descriptor: ResourceRef) {
    descriptors
        .entry(descriptor.ref_id.clone())
        .and_modify(|current| {
            if descriptor.generation > current.generation
                || (descriptor.generation == current.generation
                    && descriptor.version >= current.version)
            {
                *current = descriptor.clone();
            }
        })
        .or_insert(descriptor);
}

fn simple_hash(bytes: &[u8]) -> String {
    let sum = bytes
        .iter()
        .fold(0u64, |acc, byte| acc.wrapping_add(*byte as u64));
    format!("sum:{sum}:len:{}", bytes.len())
}
