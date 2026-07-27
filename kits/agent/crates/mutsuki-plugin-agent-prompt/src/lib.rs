pub mod plugin;

#[deprecated(note = "Prompt assembly moved to mutsuki-agent-runtime")]
pub use mutsuki_agent_runtime::PromptRegistry;
pub use plugin::*;
