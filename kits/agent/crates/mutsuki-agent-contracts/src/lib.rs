// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::doc_markdown,
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::return_self_not_must_use,
    clippy::struct_excessive_bools
)]

pub mod adapter;
pub mod code_index;
pub mod coding;
pub mod completion;
pub mod computer_use;
pub mod context;
pub mod credential;
pub mod editor_context;
pub mod error;
pub mod git;
pub mod knowledge;
pub mod lsp;
pub mod management;
pub mod mcp;
pub mod media;
pub mod memory;
pub mod message;
pub mod model;
pub mod next_edit;
pub mod placement;
pub mod plugin;
pub mod profile;
pub mod prompt;
pub mod resource;
pub mod run;
pub mod schedule;
pub mod search;
pub mod session;
pub mod skill;
pub mod state;
pub mod stream;
pub mod subagent;
pub mod tool;
pub mod usage;
pub mod wire;

pub use adapter::*;
pub use code_index::*;
pub use coding::*;
pub use completion::*;
pub use computer_use::*;
pub use context::*;
pub use credential::*;
pub use editor_context::*;
pub use error::*;
pub use git::*;
pub use knowledge::*;
pub use lsp::*;
pub use management::{
    AgentConnectionConfig, AgentConnectionManagementApi, AgentConnectionManagementSnapshot,
    AgentConnectionState, AgentConnectionStatus, AgentManagementError, AgentSessionManagementApi,
    AgentSessionPage, AgentSessionSummary,
};
pub use mcp::*;
pub use media::*;
pub use memory::*;
pub use message::*;
pub use model::*;
pub use next_edit::*;
pub use placement::*;
pub use plugin::*;
pub use profile::*;
pub use prompt::*;
pub use resource::*;
pub use run::*;
pub use schedule::*;
pub use search::*;
pub use session::*;
pub use skill::*;
pub use state::*;
pub use stream::*;
pub use subagent::*;
pub use tool::*;
pub use usage::*;
pub use wire::*;

pub const AGENT_RUN_PROTOCOL: &str = "mutsuki.agent/run@1";
pub const AGENT_CONTEXT_BUILD_PROTOCOL: &str = "mutsuki.agent.context/build@1";
pub const AGENT_CONTEXT_PROVIDER_COLLECT_PROTOCOL: &str =
    "mutsuki.agent.context.provider/collect@1";
pub const AGENT_TOOL_LIST_PROTOCOL: &str = "mutsuki.agent.tool/list@1";
pub const AGENT_TOOL_EXECUTE_PROTOCOL: &str = "mutsuki.agent.tool/execute@1";
pub const AGENT_SESSION_CREATE_PROTOCOL: &str = "mutsuki.agent.session/create@1";
pub const AGENT_SESSION_GET_PROTOCOL: &str = "mutsuki.agent.session/get@1";
pub const AGENT_SESSION_APPEND_PROTOCOL: &str = "mutsuki.agent.session/append@1";
pub const AGENT_SESSION_SNAPSHOT_PROTOCOL: &str = "mutsuki.agent.session/snapshot@1";
pub const AGENT_SESSION_FORK_PROTOCOL: &str = "mutsuki.agent.session/fork@1";
pub const AGENT_MEMORY_QUERY_PROTOCOL: &str = "mutsuki.agent.memory/query@1";
pub const AGENT_MEMORY_WRITE_PROTOCOL: &str = "mutsuki.agent.memory/write@1";
pub const AGENT_MEMORY_ACTIVATE_PROTOCOL: &str = "mutsuki.agent.memory/activate@1";
pub const AGENT_MEMORY_DISABLE_PROTOCOL: &str = "mutsuki.agent.memory/disable@1";
pub const AGENT_MEMORY_DELETE_PROTOCOL: &str = "mutsuki.agent.memory/delete@1";
pub const AGENT_MEMORY_REVISE_PROTOCOL: &str = "mutsuki.agent.memory/revise@1";
pub const AGENT_MEMORY_OPPORTUNITY_PROTOCOL: &str = "mutsuki.agent.memory/opportunity@1";
pub const AGENT_MEMORY_FEEDBACK_PROTOCOL: &str = "mutsuki.agent.memory/feedback@1";
pub const AGENT_MODEL_GENERATE_PROTOCOL: &str = "mutsuki.agent.model/generate@1";
pub const AGENT_MODEL_STREAM_PROTOCOL: &str = "mutsuki.agent.model/stream@1";
pub const AGENT_MODEL_HTTP_EFFECT_PROTOCOL: &str = "effect.mutsuki.agent.model/http@1";
pub const AGENT_MODEL_POLL_PROTOCOL: &str = "mutsuki.agent.model/poll@1";
pub const AGENT_PROMPT_RENDER_PROTOCOL: &str = "mutsuki.agent.prompt/render@1";
pub const AGENT_PROMPT_GET_PROTOCOL: &str = "mutsuki.agent.prompt/get@1";
pub const AGENT_TRANSCRIBE_PROTOCOL: &str = "mutsuki.agent.media/transcribe@1";
pub const AGENT_SPEECH_SYNTHESIZE_PROTOCOL: &str = "mutsuki.agent.media/speech_synthesize@1";
pub const AGENT_SCHEDULE_CREATE_PROTOCOL: &str = "mutsuki.agent.schedule/create@1";
pub const AGENT_SCHEDULE_LIST_PROTOCOL: &str = "mutsuki.agent.schedule/list@1";
pub const AGENT_SCHEDULE_GET_PROTOCOL: &str = "mutsuki.agent.schedule/get@1";
pub const AGENT_SCHEDULE_UPDATE_PROTOCOL: &str = "mutsuki.agent.schedule/update@1";
pub const AGENT_SCHEDULE_PAUSE_PROTOCOL: &str = "mutsuki.agent.schedule/pause@1";
pub const AGENT_SCHEDULE_RESUME_PROTOCOL: &str = "mutsuki.agent.schedule/resume@1";
pub const AGENT_SCHEDULE_CANCEL_PROTOCOL: &str = "mutsuki.agent.schedule/cancel@1";
pub const AGENT_SCHEDULE_HISTORY_PROTOCOL: &str = "mutsuki.agent.schedule/history@1";
pub const AGENT_SCHEDULE_DUE_PROTOCOL: &str = "mutsuki.agent.schedule/due@1";
pub const AGENT_SCHEDULE_COMPLETE_PROTOCOL: &str = "mutsuki.agent.schedule/complete@1";
pub const AGENT_SKILL_DISCOVER_PROTOCOL: &str = "mutsuki.agent.skill/discover@1";
pub const AGENT_SKILL_LOAD_PROTOCOL: &str = "mutsuki.agent.skill/load@1";
pub const AGENT_SKILL_RELOAD_PROTOCOL: &str = "mutsuki.agent.skill/reload@1";
pub const AGENT_KNOWLEDGE_INGEST_PROTOCOL: &str = "mutsuki.agent.knowledge/ingest@1";
pub const AGENT_KNOWLEDGE_RETRIEVE_PROTOCOL: &str = "mutsuki.agent.knowledge/retrieve@1";
pub const AGENT_KNOWLEDGE_DELETE_PROTOCOL: &str = "mutsuki.agent.knowledge/delete@1";
pub const AGENT_EDITOR_CONTEXT_PROTOCOL: &str = "mutsuki.agent.editor_context/call@1";
pub const AGENT_CODE_INDEX_PROTOCOL: &str = "mutsuki.agent.code_index/call@1";
pub const AGENT_CODE_COMPLETION_PROTOCOL: &str = "mutsuki.agent.code_completion/call@1";
pub const AGENT_NEXT_EDIT_PROTOCOL: &str = "mutsuki.agent.next_edit/call@1";
pub const AGENT_GIT_PROTOCOL: &str = "mutsuki.agent.git/call@1";
pub const AGENT_LSP_PROTOCOL: &str = "mutsuki.agent.lsp/call@1";
pub const AGENT_COMPUTER_USE_PROTOCOL: &str = "mutsuki.agent.computer_use/call@1";
pub const AGENT_MCP_PROTOCOL: &str = "mutsuki.agent.mcp/call@1";
pub const AGENT_CREDENTIAL_LOGIN_PROTOCOL: &str = "mutsuki.agent.credential/login@1";
pub const AGENT_CREDENTIAL_IMPORT_PROTOCOL: &str = "mutsuki.agent.credential/import@1";
pub const AGENT_CREDENTIAL_REFRESH_PROTOCOL: &str = "mutsuki.agent.credential/refresh@1";
pub const AGENT_CREDENTIAL_REVOKE_PROTOCOL: &str = "mutsuki.agent.credential/revoke@1";
pub const AGENT_CREDENTIAL_STATUS_PROTOCOL: &str = "mutsuki.agent.credential/status@1";
pub const AGENT_CREDENTIAL_ISSUE_HANDLE_PROTOCOL: &str = "mutsuki.agent.credential/issue_handle@1";
pub const AGENT_WIRE_PROTOCOL: &str = "mutsuki.agent.wire@1";
