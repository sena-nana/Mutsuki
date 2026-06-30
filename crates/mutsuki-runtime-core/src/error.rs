use mutsuki_runtime_contracts::RuntimeError;
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

pub(crate) fn runtime_error(
    code: impl Into<String>,
    source: impl Into<String>,
    route: impl Into<String>,
) -> RuntimeError {
    RuntimeError::new(code, source, route)
}

pub(crate) fn runtime_failure(
    code: impl Into<String>,
    source: impl Into<String>,
    route: impl Into<String>,
) -> RuntimeFailure {
    RuntimeFailure::new(runtime_error(code, source, route))
}
