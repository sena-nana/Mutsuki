use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{AgentId, RuntimeError, ScalarValue};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeEventKind {
    Lifecycle,
    Routing,
    Operation,
    Resource,
    Trace,
    Backend,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuntimeEvent {
    pub sequence: u64,
    pub kind: RuntimeEventKind,
    pub name: String,
    #[serde(default)]
    pub agent_id: Option<AgentId>,
    #[serde(default)]
    pub attributes: BTreeMap<String, ScalarValue>,
    #[serde(default)]
    pub error: Option<RuntimeError>,
}
