use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{AgentId, RuntimeError, ScalarValue};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeEventKind {
    Lifecycle,
    Routing,
    Operation,
    Plugin,
    Resource,
    Trace,
    Backend,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuntimeEvent {
    pub sequence: u64,
    pub kind: RuntimeEventKind,
    pub name: String,
    pub agent_id: Option<AgentId>,
    pub attributes: BTreeMap<String, ScalarValue>,
    pub error: Option<RuntimeError>,
}
