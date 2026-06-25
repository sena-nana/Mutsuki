use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use mutsuki_runtime_contracts::{
    ContractSurface, ContractSurfaceKind, ERR_CAPABILITY_EXHAUSTED,
    ERR_RESOURCE_GENERATION_MISMATCH, ERR_RESOURCE_LEASE_EXPIRED, ERR_RESOURCE_NOT_FOUND,
    ExclusiveWriteLease, LeaseToken, ResourceAccess, ResourceLifetime, ResourceRef,
    ResourceSealState, ResourceValue, RuntimeError, SurfaceOccupancy, ValueRef, ValueStorage,
};
use serde_json::Value;

use crate::{IdSource, RuntimeFailure, RuntimeResult, SequentialIdSource};

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
            id_source: SequentialIdSource::new(),
            inline_value_max_bytes: 4096,
            root: std::env::temp_dir().join("mutsuki-resource-manager"),
        }
    }

    pub fn pack_value(&mut self, schema: &str, value: Value) -> RuntimeResult<PackedValue> {
        let bytes = serde_json::to_vec(&value).map_err(|err| {
            RuntimeFailure::new(RuntimeError::new(
                "resource.encode_failed",
                "runtime.resource_manager",
                err.to_string(),
            ))
        })?;
        if bytes.len() <= self.inline_value_max_bytes {
            return Ok(PackedValue::Inline(ResourceValue::Inline {
                schema: schema.to_string(),
                value,
                version: 1,
            }));
        }
        let ref_id = self.id_source.next_id("value");
        let value_ref = ValueRef {
            ref_id: ref_id.clone(),
            provider_id: "resource.local".into(),
            schema: schema.into(),
            version: 1,
            generation: 1,
            size_hint: Some(bytes.len() as u64),
            content_hash: Some(simple_hash(&bytes)),
            lifetime: ResourceLifetime::Persistent,
            storage: ValueStorage::LocalValueStore,
        };
        self.values.insert(ref_id, (value_ref.clone(), value));
        Ok(PackedValue::Value(value_ref))
    }

    pub fn get_value(&self, value_ref: &ValueRef) -> RuntimeResult<Value> {
        let (stored, value) = self.values.get(&value_ref.ref_id).ok_or_else(|| {
            RuntimeFailure::new(RuntimeError::new(
                ERR_RESOURCE_NOT_FOUND,
                "runtime.resource_manager",
                format!("value.{}", value_ref.ref_id),
            ))
        })?;
        if stored.generation != value_ref.generation {
            return Err(RuntimeFailure::new(RuntimeError::new(
                ERR_RESOURCE_GENERATION_MISMATCH,
                "runtime.resource_manager",
                format!("value.{}", value_ref.ref_id),
            )));
        }
        Ok(value.clone())
    }

    pub fn create_mmap_resource(
        &mut self,
        schema: &str,
        bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        fs::create_dir_all(&self.root).map_err(io_failure)?;
        let ref_id = self.id_source.next_id("resource");
        let path = self.root.join(format!("{ref_id}.bin"));
        fs::write(&path, &bytes).map_err(io_failure)?;
        let descriptor = ResourceRef {
            ref_id: ref_id.clone(),
            provider_id: "resource.local".into(),
            resource_kind: "bytes".into(),
            schema: schema.into(),
            version: 1,
            generation: 1,
            access: ResourceAccess::MmapFile {
                path: path.to_string_lossy().to_string(),
                offset: 0,
                len: bytes.len() as u64,
                readonly: true,
            },
            size_hint: Some(bytes.len() as u64),
            content_hash: Some(simple_hash(&bytes)),
            lifetime: ResourceLifetime::Persistent,
            lease: None,
            seal_state: ResourceSealState::Sealed,
        };
        self.resources
            .insert(ref_id, ResourceEntry::new(descriptor.clone(), bytes));
        Ok(descriptor)
    }

    pub fn create_blob_resource(&mut self, schema: &str, bytes: Vec<u8>) -> ResourceRef {
        let ref_id = self.id_source.next_id("resource");
        let descriptor = ResourceRef {
            ref_id: ref_id.clone(),
            provider_id: "resource.local".into(),
            resource_kind: "blob".into(),
            schema: schema.into(),
            version: 1,
            generation: 1,
            access: ResourceAccess::Blob {
                store_id: "resource.local.blob".into(),
                key: ref_id.clone(),
            },
            size_hint: Some(bytes.len() as u64),
            content_hash: Some(simple_hash(&bytes)),
            lifetime: ResourceLifetime::Persistent,
            lease: None,
            seal_state: ResourceSealState::Sealed,
        };
        self.resources
            .insert(ref_id, ResourceEntry::new(descriptor.clone(), bytes));
        descriptor
    }

    pub fn read_resource(&self, resource_ref: &ResourceRef) -> RuntimeResult<Vec<u8>> {
        let entry = self.resources.get(&resource_ref.ref_id).ok_or_else(|| {
            RuntimeFailure::new(RuntimeError::new(
                ERR_RESOURCE_NOT_FOUND,
                "runtime.resource_manager",
                format!("resource.{}", resource_ref.ref_id),
            ))
        })?;
        if entry.descriptor.generation != resource_ref.generation {
            return Err(RuntimeFailure::new(RuntimeError::new(
                ERR_RESOURCE_GENERATION_MISMATCH,
                "runtime.resource_manager",
                format!("resource.{}", resource_ref.ref_id),
            )));
        }
        match &entry.descriptor.access {
            ResourceAccess::MmapFile { path, .. } => fs::read(path).map_err(io_failure),
            _ => Ok(entry.bytes.clone()),
        }
    }

    pub fn copy_on_write(
        &mut self,
        base_ref: &ResourceRef,
        bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        self.read_resource(base_ref)?;
        self.create_mmap_resource(&base_ref.schema, bytes)
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
        entry.bytes = bytes.clone();
        if let ResourceAccess::MmapFile { path, len, .. } = &mut entry.descriptor.access {
            fs::write(path, &bytes).map_err(io_failure)?;
            *len = bytes.len() as u64;
        }
        entry.writer = None;
        Ok(entry.descriptor.clone())
    }

    pub fn surface_occupancy(&self, surfaces: &[ContractSurface]) -> Vec<SurfaceOccupancy> {
        let mut occupancy = Vec::new();
        for surface in surfaces {
            let mut item = zero_occupancy(&surface.surface_id);
            match surface.kind {
                ContractSurfaceKind::ResourceSchema => {
                    let (resource_refs, active_leases) =
                        self.count_resources_for_surface("resource_schema", &surface.surface_id);
                    item.resource_refs = resource_refs;
                    item.active_leases = active_leases;
                }
                ContractSurfaceKind::ResourceProvider => {
                    let (resource_refs, active_leases) =
                        self.count_resources_for_surface("resource_provider", &surface.surface_id);
                    item.resource_refs = resource_refs;
                    item.active_leases = active_leases;
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
                _ => {}
            }
            if !item.is_zero() {
                occupancy.push(item);
            }
        }
        occupancy
    }

    fn count_resources_for_surface(&self, prefix: &str, surface_id: &str) -> (u64, u64) {
        let mut resource_refs = 0;
        let mut active_leases = 0;
        for entry in self
            .resources
            .values()
            .filter(|entry| resource_surface_matches(entry, prefix, surface_id))
        {
            resource_refs += 1;
            if entry.writer.is_some() {
                active_leases += 1;
            }
        }
        (resource_refs, active_leases)
    }
}

fn resource_surface_matches(entry: &ResourceEntry, prefix: &str, surface_id: &str) -> bool {
    let value = match prefix {
        "resource_schema" => &entry.descriptor.schema,
        "resource_provider" => &entry.descriptor.provider_id,
        _ => return false,
    };
    surface_id == value || surface_id == format!("{prefix}:{value}")
}

fn zero_occupancy(surface_id: &str) -> SurfaceOccupancy {
    SurfaceOccupancy {
        surface_id: surface_id.into(),
        pending_tasks: 0,
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
