use std::collections::HashMap;
use std::path::PathBuf;

use mutsuki_runtime_contracts::{
    ERR_CAPABILITY_EXHAUSTED, ERR_RESOURCE_NOT_FOUND, LeaseToken, ResourceRef, ResourceValue,
    RuntimeError, SurfaceOccupancyHandle, ValueRef,
};
use serde_json::Value;

use crate::{RuntimeFailure, SequentialIdSource};

mod leases;
mod occupancy;
mod resources;
mod values;

#[derive(Clone, Debug, PartialEq)]
pub enum PackedValue {
    Inline(ResourceValue),
    Value(ValueRef),
    Resource(ResourceRef),
}

#[derive(Clone, Debug)]
struct ResourceEntry {
    descriptor: ResourceRef,
    bytes: Vec<u8>,
    writer: Option<LeaseToken>,
}

impl ResourceEntry {
    fn new(descriptor: ResourceRef, bytes: Vec<u8>) -> Self {
        Self {
            descriptor,
            bytes,
            writer: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ResourceManager {
    values: HashMap<String, (ValueRef, Value)>,
    resources: HashMap<String, ResourceEntry>,
    occupancy_handles: HashMap<String, SurfaceOccupancyHandle>,
    id_source: SequentialIdSource,
    inline_value_max_bytes: usize,
    root: PathBuf,
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
            resources: HashMap::new(),
            occupancy_handles: HashMap::new(),
            id_source: SequentialIdSource::new(),
            inline_value_max_bytes: 4096,
            root: std::env::temp_dir().join("mutsuki-resource-manager"),
        }
    }
}

fn resource_not_found(route: String) -> RuntimeFailure {
    RuntimeFailure::new(RuntimeError::new(
        ERR_RESOURCE_NOT_FOUND,
        "runtime.resource_manager",
        route,
    ))
}

fn capability_exhausted(route: String) -> RuntimeFailure {
    RuntimeFailure::new(RuntimeError::new(
        ERR_CAPABILITY_EXHAUSTED,
        "runtime.resource_manager",
        route,
    ))
}

fn io_failure(err: std::io::Error) -> RuntimeFailure {
    RuntimeFailure::new(RuntimeError::new(
        "resource.io_failed",
        "runtime.resource_manager",
        err.to_string(),
    ))
}

fn simple_hash(bytes: &[u8]) -> String {
    let sum = bytes
        .iter()
        .fold(0u64, |acc, byte| acc.wrapping_add(*byte as u64));
    format!("sum:{sum}:len:{}", bytes.len())
}
