// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::assigning_clones,
    clippy::default_trait_access,
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::needless_pass_by_value,
    clippy::return_self_not_must_use,
    clippy::unnecessary_wraps
)]

mod command;
mod context;
mod message_builder;
pub mod prelude;

pub use command::*;
pub use context::*;
pub use message_builder::*;
