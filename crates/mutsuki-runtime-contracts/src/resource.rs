use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::RefId;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceSealState {
    Writable,
    Sealed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceLifetime {
    BorrowedUntilTaskEnd,
    LeaseUntil(u64),
    Persistent,
    ExternalManaged,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResourceAccess {
    Inline,
    MmapFile {
        path: String,
        offset: u64,
        len: u64,
        readonly: bool,
    },
    SharedMemory {
        name: String,
        offset: u64,
        len: u64,
        readonly: bool,
    },
    Blob {
        store_id: String,
        key: String,
    },
    Stream {
        endpoint: String,
    },
    ProviderRpc {
        provider_id: String,
        method: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResourceRef {
    pub ref_id: RefId,
    pub provider_id: String,
    pub resource_kind: String,
    pub schema: String,
    pub version: u64,
    pub generation: u64,
    pub access: ResourceAccess,
    pub size_hint: Option<u64>,
    pub content_hash: Option<String>,
    pub lifetime: ResourceLifetime,
    pub lease: Option<LeaseToken>,
    pub seal_state: ResourceSealState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseToken {
    pub token_id: String,
    pub ref_id: RefId,
    pub owner: String,
    pub mode: String,
    pub expires_at_step: Option<u64>,
    pub generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExclusiveWriteLease {
    pub token: LeaseToken,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueStorage {
    InlineSmall,
    LocalValueStore,
    Blob,
    Stream,
    ProviderRpc,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ValueRef {
    pub ref_id: RefId,
    pub provider_id: String,
    pub schema: String,
    pub version: u64,
    pub generation: u64,
    pub size_hint: Option<u64>,
    pub content_hash: Option<String>,
    pub lifetime: ResourceLifetime,
    pub storage: ValueStorage,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResourceValue {
    Inline {
        schema: String,
        value: Value,
        version: u64,
    },
    ValueRef(ValueRef),
    ResourceRef(ResourceRef),
}
