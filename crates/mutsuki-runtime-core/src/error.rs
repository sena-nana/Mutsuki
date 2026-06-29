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

macro_rules! runtime_error {
    ($code:expr, $source:expr, $route:expr) => {
        mutsuki_runtime_contracts::RuntimeError::new($code, $source, $route)
    };
}

macro_rules! runtime_failure {
    ($code:expr, $source:expr, $route:expr) => {
        crate::RuntimeFailure::new(runtime_error!($code, $source, $route))
    };
}
