pub mod plugin;

#[deprecated(note = "Tool routing moved to mutsuki-agent-runtime")]
pub use mutsuki_agent_runtime::ToolRegistry;
pub use plugin::*;
