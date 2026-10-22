//! SQLite store for the Bot plugins: one actor thread owns one `state.sqlite3` connection and
//! implements the conversation, ICL, persona, delivery, reply delivery, interaction,
//! management and sandbox history stores on top of it.

// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::doc_markdown,
    clippy::ignored_unit_patterns,
    clippy::implicit_clone,
    clippy::map_unwrap_or,
    clippy::missing_errors_doc,
    clippy::needless_borrow,
    clippy::needless_pass_by_value,
    clippy::semicolon_if_nothing_returned,
    clippy::too_many_lines,
    clippy::type_complexity
)]

mod codec;
mod context;
mod conversation;
mod delivery;
mod error;
mod inspect;
mod interaction;
mod management;
mod migrations;
mod persona;
mod reply_delivery;
mod repository;
mod sandbox_history;
#[cfg(test)]
mod tests;

pub use error::BotStateDbError;
pub use inspect::{
    BotDatabaseColumnInfo, BotDatabaseSnapshot, BotDatabaseTableInfo, BotDatabaseTablePage,
};
pub use management::{BotManagementAuditRecord, BotManagementOperationReservation};
pub use repository::{BotStateDbMetrics, BotStateDbRepository, BotStatePage};
