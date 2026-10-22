//! Structured runner failures shared by Bot plugins.
//!
//! Every Bot runner reports infrastructure failures as `runtime.host_failed` owned by its plugin,
//! with a machine route and the human-readable detail kept in `evidence.message` so routes stay
//! stable and never embed free text.

use mutsuki_runtime_contracts::{ERR_RUNTIME_HOST_FAILED, RuntimeError, ScalarValue};
use mutsuki_runtime_sdk::RuntimeFailure;

/// `runtime.host_failed` owned by `plugin_id` at `route`.
pub fn runtime_error(plugin_id: &str, route: impl Into<String>) -> RuntimeError {
    RuntimeError::new(ERR_RUNTIME_HOST_FAILED, plugin_id, route.into())
}

/// [`runtime_error`] carrying `message` as `evidence.message`.
pub fn runtime_error_with_message(
    plugin_id: &str,
    route: impl Into<String>,
    message: impl std::fmt::Display,
) -> RuntimeError {
    let mut error = runtime_error(plugin_id, route);
    error
        .evidence
        .insert("message".into(), ScalarValue::String(message.to_string()));
    error
}

/// [`runtime_error_with_message`] wrapped as a batch-level [`RuntimeFailure`].
pub fn runtime_failure(
    plugin_id: &str,
    route: impl Into<String>,
    message: impl std::fmt::Display,
) -> RuntimeFailure {
    RuntimeFailure::new(runtime_error_with_message(plugin_id, route, message))
}
