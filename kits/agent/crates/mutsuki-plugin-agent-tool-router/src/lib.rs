// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(clippy::wildcard_imports)]

pub mod plugin;

#[deprecated(note = "Tool routing moved to mutsuki-agent-runtime")]
pub use mutsuki_agent_runtime::ToolRegistry;
pub use plugin::*;
