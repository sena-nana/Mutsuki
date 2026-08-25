// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::doc_markdown,
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::return_self_not_must_use,
    clippy::wildcard_imports
)]

pub mod agent_client;
pub mod agent_context;
pub mod memory_client;
pub mod message_builder;
pub mod model_client;
pub mod plugin;
pub mod prelude;
pub mod prompt_builder;
pub mod protocol;
pub mod resource;
pub mod task;
pub mod tool_builder;
pub mod transport;

pub use agent_client::*;
pub use agent_context::*;
pub use memory_client::*;
pub use message_builder::*;
pub use model_client::*;
pub use plugin::*;
pub use prompt_builder::*;
pub use protocol::*;
pub use resource::*;
pub use task::*;
pub use tool_builder::*;
pub use transport::*;

pub use mutsuki_agent_contracts as protocol_types;
pub use mutsuki_agent_macros::{agent_profile, agent_tool};
pub use mutsuki_runtime_sdk::{RuntimeClient, SdkProtocol, TaskSubmitter};
