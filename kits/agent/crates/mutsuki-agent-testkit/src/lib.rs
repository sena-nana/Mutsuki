// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::cast_possible_truncation,
    clippy::map_unwrap_or,
    clippy::match_same_arms,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::ref_option,
    clippy::return_self_not_must_use,
    clippy::too_many_lines,
    clippy::unnecessary_literal_bound
)]

pub mod auth_conformance;
pub mod benchmark;
pub mod code_completion;
pub mod coding_events;
pub mod conformance;
pub mod fake_editor;
pub mod fake_memory;
pub mod fake_model;
pub mod fake_session;
pub mod fake_tool;

pub use auth_conformance::*;
pub use benchmark::*;
pub use code_completion::*;
pub use coding_events::*;
pub use conformance::*;
pub use fake_editor::*;
pub use fake_memory::*;
pub use fake_model::*;
pub use fake_session::*;
pub use fake_tool::*;
