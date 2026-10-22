// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::assigning_clones,
    clippy::cast_possible_truncation,
    clippy::doc_markdown,
    clippy::format_push_string,
    clippy::large_stack_arrays,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::return_self_not_must_use,
    clippy::single_match_else,
    clippy::too_many_lines,
    clippy::uninlined_format_args,
    clippy::unused_self
)]
//! AgentKit Git plugin: the `GitGateway` backend trait, in-memory and `git` CLI
//! backends, and the `SharedGitService` consumed by Agent tools and product Git UI.

mod cli;
mod gateway;
mod memory;
mod service;
mod support;

pub use cli::CliGitBackend;
pub use gateway::GitGateway;
pub use memory::InMemoryGitBackend;
pub use service::SharedGitService;

pub const PLUGIN_ID: &str = "mutsuki.plugin.agent.git";
pub const SERVICE_ID: &str = mutsuki_agent_contracts::AGENT_GIT_PROTOCOL;
pub const CONTEXT_PROVIDER_ID: &str = "mutsuki.agent.context.git";
pub const INLINE_LIMIT: usize = 2_048;
pub const SUMMARY_LIMIT: usize = 512;
