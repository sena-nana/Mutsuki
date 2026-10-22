//! Typed contracts between the Bot Web Console and its WebExtensions.
//!
//! Pure DTOs and [`TypedRpc`](mutsuki_web_protocol::TypedRpc) markers; no service, Host or
//! runtime types. Each module is one WebExtension namespace, and its TypeScript contract is
//! generated from these markers (see `ts_contract` tests in the owning extension crate).

pub mod platforms;
