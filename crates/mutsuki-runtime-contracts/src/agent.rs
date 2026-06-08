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
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub priority: i64,
    #[serde(default = "default_participation")]
    pub participation: AgentParticipation,
    #[serde(default)]
    pub accepts: Vec<ScopeRuleSpec>,
    #[serde(default)]
    pub strategy_id: String,
    #[serde(default = "default_side_effect_policy")]
    pub side_effect_policy: SideEffectPolicy,
}

fn default_participation() -> AgentParticipation {
    AgentParticipation::PrimaryCandidate
}

fn default_side_effect_policy() -> SideEffectPolicy {
    SideEffectPolicy::ReadOnly
}
