//! The Bilibili error type and its mapping onto structured runtime errors.

use std::fmt;

use mutsuki_bot_management::BilibiliManagementError;
use mutsuki_runtime_contracts::{RuntimeError, ScalarValue, Task};
use mutsuki_runtime_sdk::RuntimeFailure;

use crate::PLUGIN_ID;

#[derive(Debug, thiserror::Error)]
pub enum BilibiliError {
    #[error("Bilibili cookie is missing or expired")]
    CookieExpired,
    #[error("Bilibili request was rate limited")]
    RateLimited,
    #[error("Bilibili risk control rejected the request (code 352)")]
    RiskControl352,
    #[error("Bilibili domain is not allowed: {0}")]
    DomainDenied(String),
    #[error("Bilibili response is invalid: {0}")]
    InvalidResponse(String),
    #[error("Bilibili transport failed: {0}")]
    Transport(String),
    #[error("Bilibili management is unavailable: {0}")]
    ManagementUnavailable(String),
    #[error("Bilibili management request is forbidden")]
    Forbidden,
}

pub(crate) fn bili_error(task: &Task, error: BilibiliError) -> RuntimeError {
    let code = match &error {
        BilibiliError::CookieExpired => "bilibili.cookie_expired",
        BilibiliError::RateLimited => "bilibili.rate_limited",
        BilibiliError::RiskControl352 => "bilibili.risk_control_352",
        BilibiliError::Forbidden => "bilibili.management_forbidden",
        BilibiliError::ManagementUnavailable(_) => "bilibili.management_unavailable",
        _ => "bilibili.request_failed",
    };
    let mut runtime = RuntimeError::new(code, PLUGIN_ID, format!("bilibili.{}", task.task_id));
    runtime
        .evidence
        .insert("detail".into(), ScalarValue::String(error.to_string()));
    if code == "bilibili.risk_control_352" {
        for (key, value) in [
            ("risk_control_code", "352"),
            ("fallback_status", "not_configured"),
            ("degraded", "true"),
        ] {
            runtime
                .evidence
                .insert(key.into(), ScalarValue::String(value.into()));
        }
    }
    runtime
}

pub(crate) fn bili_management_error(task: &Task, error: BilibiliManagementError) -> RuntimeError {
    let mut runtime =
        RuntimeError::new(error.code, PLUGIN_ID, format!("bilibili.{}", task.task_id));
    runtime
        .evidence
        .insert("detail".into(), ScalarValue::String(error.message));
    runtime
}

pub(crate) fn risk_control_failure(
    task: &Task,
    route: &str,
    detail: impl fmt::Display,
) -> RuntimeFailure {
    let mut error = RuntimeError::new(
        "bilibili.risk_control_fallback_failed",
        PLUGIN_ID,
        format!("bilibili.risk_control.{route}.{}", task.task_id),
    );
    for (key, value) in [
        ("risk_control_code", "352"),
        ("backend", "chromium"),
        ("fallback_status", "failed"),
        ("degraded", "true"),
    ] {
        error
            .evidence
            .insert(key.into(), ScalarValue::String(value.into()));
    }
    error
        .evidence
        .insert("detail".into(), ScalarValue::String(detail.to_string()));
    RuntimeFailure::new(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::POLL_LIVE;
    use serde_json::Value;

    #[test]
    fn rate_limit_cookie_and_352_have_distinct_runtime_codes() {
        let task = Task::new("error", POLL_LIVE, Value::Null);
        assert_eq!(
            bili_error(&task, BilibiliError::RateLimited).code,
            "bilibili.rate_limited"
        );
        assert_eq!(
            bili_error(&task, BilibiliError::CookieExpired).code,
            "bilibili.cookie_expired"
        );
        let risk_control = bili_error(&task, BilibiliError::RiskControl352);
        assert_eq!(risk_control.code, "bilibili.risk_control_352");
        assert_eq!(
            risk_control.evidence.get("fallback_status"),
            Some(&ScalarValue::String("not_configured".into()))
        );
    }
}
