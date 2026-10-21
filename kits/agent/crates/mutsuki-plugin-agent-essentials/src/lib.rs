//! The small Agent service plugins, one module per plugin.
//!
//! Each module keeps its own `PLUGIN_ID`, `RUNNER_ID`, `plugin()` and `runner()`: the modules
//! are separate loadable plugins that only share a package, so manifests and wire ids are the
//! same as when they were six crates.
// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::doc_markdown,
    clippy::must_use_candidate,
    clippy::unused_async,
    clippy::wildcard_imports
)]

/// Credential broker plugin (`mutsuki.plugin.agent.credential`) and its adapter bridge.
pub mod credential;
/// Knowledge service plugin (`mutsuki.plugin.agent.knowledge`).
pub mod knowledge;
/// Memory router plugin (`mutsuki.plugin.agent.memory_router`).
pub mod memory_router;
/// Prompt template registry plugin (`mutsuki.plugin.agent.prompt`).
pub mod prompt;
/// Session store plugin (`mutsuki.plugin.agent.session`).
pub mod session;
/// Skill registry plugin (`mutsuki.plugin.agent.skills`).
pub mod skills;
