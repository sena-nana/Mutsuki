use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Envelope, RuntimeError};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategyResultStatus {
    Continue,
    WaitInput,
    Completed,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StrategyResult {
    pub status: StrategyResultStatus,
    #[serde(default)]
    pub decision: Option<Value>,
    #[serde(default)]
    pub emitted: Vec<Envelope>,
    #[serde(default)]
    pub error: Option<RuntimeError>,
}

impl StrategyResult {
    pub fn wait_input() -> Self {
        Self {
            status: StrategyResultStatus::WaitInput,
            decision: None,
            emitted: Vec::new(),
            error: None,
        }
    }
}
