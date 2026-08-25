// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::default_trait_access,
    clippy::format_push_string,
    clippy::map_unwrap_or,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::needless_pass_by_value,
    clippy::redundant_closure_for_method_calls,
    clippy::return_self_not_must_use,
    clippy::too_many_lines
)]

mod dispatch;
mod matcher;
mod parser;

pub use dispatch::*;
pub use matcher::*;
pub use mutsuki_bot_protocol::BotCommandEvent;
pub use parser::*;
