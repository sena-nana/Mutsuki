// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::assigning_clones,
    clippy::if_not_else,
    clippy::match_same_arms,
    clippy::needless_pass_by_value,
    clippy::too_many_lines,
    clippy::wildcard_imports
)]

pub mod plugin;

#[deprecated(note = "Agent loop state moved to mutsuki-agent-runtime")]
pub use mutsuki_agent_runtime::AgentLoop;
pub use plugin::*;
