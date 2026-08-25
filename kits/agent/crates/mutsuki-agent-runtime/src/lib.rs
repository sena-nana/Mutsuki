// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::assigning_clones,
    clippy::cast_lossless,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::default_trait_access,
    clippy::doc_markdown,
    clippy::map_unwrap_or,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::needless_pass_by_value,
    clippy::redundant_closure_for_method_calls,
    clippy::ref_option,
    clippy::return_self_not_must_use,
    clippy::single_char_pattern,
    clippy::too_many_lines,
    clippy::uninlined_format_args,
    clippy::unnecessary_wraps,
    clippy::unused_self
)]

mod agent_loop;
mod context_assembly;
mod context_batch;
mod context_compaction;
mod context_planner;
mod coordinator;
mod coordinator_fence;
mod credential_broker;
mod knowledge_service;
mod memory_routing;
mod profile;
mod prompt_assembly;
mod resource_store;
mod session_store;
mod skill_registry;
mod subagent;
mod tool_routing;
mod transcript;
mod transcript_context;

pub use agent_loop::*;
pub use context_assembly::*;
pub use context_batch::*;
pub use context_compaction::*;
pub use context_planner::*;
pub use coordinator::*;
pub use coordinator_fence::*;
pub use credential_broker::*;
pub use knowledge_service::*;
pub use memory_routing::*;
pub use mutsuki_agent_contracts::{
    AgentSessionState, AgentSessionStatus, AgentStepState, AgentStepStatus, AgentTurnState,
    AgentTurnStatus, PendingApproval, SessionVersion, StopReason,
};
pub use profile::*;
pub use prompt_assembly::*;
pub use resource_store::*;
pub use session_store::*;
pub use skill_registry::*;
pub use subagent::*;
pub use tool_routing::*;
pub use transcript::*;
pub use transcript_context::*;
