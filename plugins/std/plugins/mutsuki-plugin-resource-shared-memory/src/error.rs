use mutsuki_runtime_contracts::{ERR_RESOURCE_UNSUPPORTED, RuntimeError, ScalarValue};
use mutsuki_runtime_core::RuntimeFailure;

use crate::constants::RUNTIME_TARGET;

pub(crate) fn unsupported(route: &str, detail: &str) -> RuntimeFailure {
    detailed_failure(ERR_RESOURCE_UNSUPPORTED, route, detail.to_string())
}

pub(crate) fn detailed_failure(code: &str, route: &str, detail: String) -> RuntimeFailure {
    let mut error = RuntimeError::new(code, RUNTIME_TARGET, route);
    error
        .evidence
        .insert("detail".into(), ScalarValue::String(detail));
    RuntimeFailure::new(error)
}

pub(crate) fn runtime_failure(code: &str, route: String) -> RuntimeFailure {
    RuntimeFailure::new(RuntimeError::new(code, RUNTIME_TARGET, route))
}
