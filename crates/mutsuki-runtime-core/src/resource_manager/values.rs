use mutsuki_runtime_contracts::{
    ERR_RESOURCE_GENERATION_MISMATCH, ERR_RESOURCE_NOT_FOUND, ResourceLifetime, ResourceValue,
    ValueRef, ValueStorage,
};
use serde_json::Value;

use crate::{IdSource, RuntimeResult};

use super::{PackedValue, ResourceManager};

impl ResourceManager {
    pub fn pack_value(&mut self, schema: &str, value: Value) -> RuntimeResult<PackedValue> {
        let bytes = serde_json::to_vec(&value).map_err(|err| {
            crate::runtime_failure(
                "resource.encode_failed",
                "runtime.resource_manager",
                err.to_string(),
            )
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
            provider_id: "runtime.resource_manager".into(),
            schema: schema.into(),
            version: 1,
            generation: 1,
            size_hint: Some(bytes.len() as u64),
            content_hash: None,
            lifetime: ResourceLifetime::Persistent,
            storage: ValueStorage::LocalValueStore,
        };
        self.values.insert(ref_id, (value_ref.clone(), value));
        Ok(PackedValue::Value(value_ref))
    }

    pub fn get_value(&self, value_ref: &ValueRef) -> RuntimeResult<Value> {
        let (stored, value) = self.values.get(&value_ref.ref_id).ok_or_else(|| {
            crate::runtime_failure(
                ERR_RESOURCE_NOT_FOUND,
                "runtime.resource_manager",
                format!("value.{}", value_ref.ref_id),
            )
        })?;
        if stored.generation != value_ref.generation {
            return Err(crate::runtime_failure(
                ERR_RESOURCE_GENERATION_MISMATCH,
                "runtime.resource_manager",
                format!("value.{}", value_ref.ref_id),
            ));
        }
        Ok(value.clone())
    }
}
