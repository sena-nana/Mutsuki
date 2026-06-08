use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{RefId, ScalarValue};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RefDescriptor {
    pub ref_id: RefId,
    pub kind: String,
    pub schema_id_target: String,
    pub schema_version_target: String,
    #[serde(default)]
    pub attributes: BTreeMap<String, ScalarValue>,
    #[serde(default)]
    pub lineage: Vec<RefId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LeaseToken {
    pub token_id: String,
    pub ref_id: RefId,
    pub owner: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResourceRecord {
    pub descriptor: RefDescriptor,
    pub owner: String,
    #[serde(default)]
    pub lease_count: u64,
}
