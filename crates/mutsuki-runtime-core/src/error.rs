use mutsuki_runtime_contracts::{
    ERR_SCOPE_NO_MATCH, ERR_SOURCE_UNREGISTERED, Envelope, RuntimeError, ScalarValue,
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
