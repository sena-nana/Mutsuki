pub mod plugin;

#[deprecated(note = "Context assembly moved to mutsuki-agent-runtime")]
pub use mutsuki_agent_runtime::ContextBuilder;
pub use mutsuki_agent_runtime::{AgentContextBuildPreparation, AgentContextModelSummary};
pub use plugin::*;
