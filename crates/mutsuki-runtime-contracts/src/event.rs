use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{RuntimeError, ScalarValue};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeEventKind {
    Lifecycle,
    Plugin,
    Resource,
    Trace,
    Host,
    Task,
    Runner,
    State,
    Effect,
    Reload,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuntimeEvent {
    pub sequence: u64,
    pub kind: RuntimeEventKind,
    pub name: String,
    pub subject_id: Option<String>,
    pub attributes: BTreeMap<String, ScalarValue>,
    pub error: Option<RuntimeError>,
}
