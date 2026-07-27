pub mod plugin;

#[deprecated(note = "Memory routing moved to mutsuki-agent-runtime")]
pub use mutsuki_agent_runtime::MemoryRouter;
pub use plugin::*;
