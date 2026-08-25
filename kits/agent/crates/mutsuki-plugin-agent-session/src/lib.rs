// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(clippy::unused_async, clippy::wildcard_imports)]

pub mod plugin;

#[deprecated(note = "Session transcript state moved to mutsuki-agent-runtime")]
pub use mutsuki_agent_runtime::SessionStore;
pub use plugin::*;
