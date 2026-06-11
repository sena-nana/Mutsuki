use serde::{Deserialize, Serialize};

use crate::{AgentId, ScopeRuleSpec};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentPhase {
    Spawn,
    Awake,
    Sleep,
    Stop,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentParticipation {
    PrimaryCandidate,
    Observer,
    ExplicitHelper,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SideEffectPolicy {
    ReadOnly,
    AllowExternal,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentSpec {
    pub agent_id: AgentId,
    pub owner: Option<String>,
    pub priority: i64,
    pub participation: AgentParticipation,
    pub accepts: Vec<ScopeRuleSpec>,
    pub strategy_id: String,
    pub side_effect_policy: SideEffectPolicy,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentSnapshot {
    pub spec: AgentSpec,
    pub phase: AgentPhase,
    pub inbox_len: usize,
}
