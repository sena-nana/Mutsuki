use mutsukicore_runtime_contracts::{
    ERR_PLUGIN_DISABLED, ERR_PLUGIN_NOT_FOUND, ERR_RUNTIME_BACKEND_FAILED,
    ERR_RUNTIME_BACKEND_GENERATION_MISMATCH, ERR_SCOPE_NO_MATCH, ERR_SOURCE_UNREGISTERED, Envelope,
    OperationSnapshot, RuntimeError, ScalarValue,
};
use thiserror::Error;

#[derive(Debug, Error)]
#[error("runtime error: {0:?}")]
pub struct RuntimeFailure(pub Box<RuntimeError>);

impl RuntimeFailure {
    pub fn new(error: RuntimeError) -> Self {
        Self(Box::new(error))
    }

    pub fn error(&self) -> &RuntimeError {
        &self.0
    }
}

pub type RuntimeResult<T> = Result<T, RuntimeFailure>;

pub fn scope_no_match_error() -> RuntimeError {
    RuntimeError::new(ERR_SCOPE_NO_MATCH, "runtime.route", "runtime.publish")
}

pub(crate) fn backend_failed(source: impl Into<String>, route: impl Into<String>) -> RuntimeError {
    RuntimeError::new(ERR_RUNTIME_BACKEND_FAILED, source, route)
}

pub(crate) fn operation_not_active_failure(
    snapshot: &OperationSnapshot,
    agent_id: &str,
    op_id: &str,
) -> RuntimeFailure {
    let mut err = backend_failed(
        snapshot.descriptor.plugin_id.clone(),
        format!("runtime.invoke.{agent_id}.{op_id}"),
    );
    err.evidence.insert(
        "operation_status".into(),
        ScalarValue::String(format!("{:?}", snapshot.status)),
    );
    RuntimeFailure::new(err)
}

pub(crate) fn source_unregistered_failure(envelope: &Envelope) -> RuntimeFailure {
    let mut err = RuntimeError::new(
        ERR_SOURCE_UNREGISTERED,
        "runtime.source_registry",
        format!("runtime.publish.{}", envelope.source.source_id),
    );
    err.evidence.insert(
        "source_id".into(),
        ScalarValue::String(envelope.source.source_id.clone()),
    );
    RuntimeFailure::new(err)
}

pub(crate) fn scope_no_match_failure(envelope: &Envelope) -> RuntimeFailure {
    let mut err = scope_no_match_error();
    err.route = format!("runtime.publish.{}", envelope.source.source_id);
    err.evidence.insert(
        "source_id".into(),
        ScalarValue::String(envelope.source.source_id.clone()),
    );
    err.evidence.insert(
        "payload_schema_id".into(),
        ScalarValue::String(envelope.payload_schema_id.clone()),
    );
    RuntimeFailure::new(err)
}

pub(crate) fn plugin_disabled_failure(plugin_id: &str, route: impl Into<String>) -> RuntimeFailure {
    let mut err = RuntimeError::new(ERR_PLUGIN_DISABLED, "runtime.plugin_registry", route);
    err.evidence.insert(
        "plugin_id".into(),
        ScalarValue::String(plugin_id.to_string()),
    );
    RuntimeFailure::new(err)
}

pub(crate) fn plugin_not_found_failure(
    plugin_id: &str,
    route: impl Into<String>,
) -> RuntimeFailure {
    let mut err = RuntimeError::new(ERR_PLUGIN_NOT_FOUND, "runtime.plugin_registry", route);
    err.evidence.insert(
        "plugin_id".into(),
        ScalarValue::String(plugin_id.to_string()),
    );
    RuntimeFailure::new(err)
}

pub(crate) fn plugin_generation_mismatch_failure(
    plugin_id: &str,
    expected: u64,
    actual: u64,
    route: impl Into<String>,
) -> RuntimeFailure {
    let mut err = RuntimeError::new(
        ERR_RUNTIME_BACKEND_GENERATION_MISMATCH,
        "runtime.plugin_registry",
        route,
    );
    err.evidence.insert(
        "plugin_id".into(),
        ScalarValue::String(plugin_id.to_string()),
    );
    err.evidence.insert(
        "expected_generation".into(),
        ScalarValue::Int(expected as i64),
    );
    err.evidence
        .insert("actual_generation".into(), ScalarValue::Int(actual as i64));
    RuntimeFailure::new(err)
}
