use mutsuki_runtime_contracts::{PluginDeploymentKind, RuntimeError, ScalarValue};
use mutsuki_runtime_core::RuntimeFailure;

pub(crate) fn host_failure(route: &str, detail: impl Into<String>) -> RuntimeFailure {
    let mut error = RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
        "runtime.host",
        route,
    );
    error
        .evidence
        .insert("detail".into(), ScalarValue::String(detail.into()));
    RuntimeFailure::new(error)
}

pub(crate) fn plugin_not_found(plugin_id: &str) -> RuntimeFailure {
    let mut error = RuntimeError::new(
        mutsuki_runtime_contracts::ERR_PLUGIN_NOT_FOUND,
        "runtime.host",
        format!("host.plugin.{plugin_id}"),
    );
    error
        .evidence
        .insert("plugin_id".into(), ScalarValue::String(plugin_id.into()));
    RuntimeFailure::new(error)
}

pub(crate) fn runner_for_disabled_plugin(plugin_id: &str, runner_id: &str) -> RuntimeFailure {
    let mut error = RuntimeError::new(
        mutsuki_runtime_contracts::ERR_PLUGIN_DISABLED,
        "runtime.host",
        format!("host.plugin.runner_disabled.{runner_id}"),
    );
    insert_plugin_and_runner(&mut error, plugin_id, runner_id);
    RuntimeFailure::new(error)
}

pub(crate) fn deployment_mismatch(
    route: &str,
    plugin_id: &str,
    actual: &PluginDeploymentKind,
    expected: &PluginDeploymentKind,
) -> RuntimeFailure {
    let mut error = RuntimeError::new(
        mutsuki_runtime_contracts::ERR_REGISTRY_UNAUTHORIZED,
        "runtime.host",
        format!("{route}.{plugin_id}"),
    );
    error
        .evidence
        .insert("plugin_id".into(), ScalarValue::String(plugin_id.into()));
    error.evidence.insert(
        "actual_deployment".into(),
        ScalarValue::String(format!("{actual:?}")),
    );
    error.evidence.insert(
        "expected_deployment".into(),
        ScalarValue::String(format!("{expected:?}")),
    );
    RuntimeFailure::new(error)
}

pub(crate) fn required_capability_missing(capability: &str) -> RuntimeFailure {
    let mut error = RuntimeError::new(
        mutsuki_runtime_contracts::ERR_REGISTRY_UNAUTHORIZED,
        "runtime.host",
        format!("host.plugin.required_capability_missing.{capability}"),
    );
    error
        .evidence
        .insert("capability".into(), ScalarValue::String(capability.into()));
    RuntimeFailure::new(error)
}

pub(crate) fn capability_version_mismatch(
    capability: &str,
    constraint: &str,
    actual_version: Option<&str>,
) -> RuntimeFailure {
    let mut error = RuntimeError::new(
        mutsuki_runtime_contracts::ERR_REGISTRY_UNAUTHORIZED,
        "runtime.host",
        format!("host.plugin.capability_version_mismatch.{capability}"),
    );
    error
        .evidence
        .insert("capability".into(), ScalarValue::String(capability.into()));
    error.evidence.insert(
        "version_constraint".into(),
        ScalarValue::String(constraint.into()),
    );
    if let Some(actual_version) = actual_version {
        error.evidence.insert(
            "actual_version".into(),
            ScalarValue::String(actual_version.into()),
        );
    }
    RuntimeFailure::new(error)
}

pub(crate) fn capability_provider_missing(capability: &str) -> RuntimeFailure {
    let mut error = RuntimeError::new(
        mutsuki_runtime_contracts::ERR_REGISTRY_UNAUTHORIZED,
        "runtime.host",
        format!("host.plugin.capability_provider_missing.{capability}"),
    );
    error
        .evidence
        .insert("capability".into(), ScalarValue::String(capability.into()));
    RuntimeFailure::new(error)
}

pub(crate) fn permission_unauthorized(
    plugin_id: &str,
    permission_kind: &str,
    permission: &str,
) -> RuntimeFailure {
    let mut error = RuntimeError::new(
        mutsuki_runtime_contracts::ERR_REGISTRY_UNAUTHORIZED,
        "runtime.host",
        format!("host.plugin.permission_unauthorized.{plugin_id}.{permission_kind}"),
    );
    error
        .evidence
        .insert("plugin_id".into(), ScalarValue::String(plugin_id.into()));
    error.evidence.insert(
        "permission_kind".into(),
        ScalarValue::String(permission_kind.into()),
    );
    error
        .evidence
        .insert("permission".into(), ScalarValue::String(permission.into()));
    RuntimeFailure::new(error)
}

pub(crate) fn runner_binding_invalid(
    protocol_id: &str,
    runner_id: &str,
    detail: &str,
) -> RuntimeFailure {
    let mut error = RuntimeError::new(
        mutsuki_runtime_contracts::ERR_REGISTRY_UNAUTHORIZED,
        "runtime.host",
        format!("host.plugin.runner_binding_invalid.{protocol_id}"),
    );
    error.evidence.insert(
        "protocol_id".into(),
        ScalarValue::String(protocol_id.into()),
    );
    error
        .evidence
        .insert("runner_id".into(), ScalarValue::String(runner_id.into()));
    error
        .evidence
        .insert("detail".into(), ScalarValue::String(detail.into()));
    RuntimeFailure::new(error)
}

pub(crate) fn runner_missing_for_deployment(
    plugin_id: &str,
    runner_id: &str,
    deployment: &PluginDeploymentKind,
) -> RuntimeFailure {
    let mut error = RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RUNNER_NOT_FOUND,
        "runtime.host",
        format!("host.plugin.runner_missing.{runner_id}"),
    );
    insert_plugin_and_runner(&mut error, plugin_id, runner_id);
    error.evidence.insert(
        "deployment".into(),
        ScalarValue::String(format!("{deployment:?}")),
    );
    RuntimeFailure::new(error)
}

fn insert_plugin_and_runner(error: &mut RuntimeError, plugin_id: &str, runner_id: &str) {
    error
        .evidence
        .insert("plugin_id".into(), ScalarValue::String(plugin_id.into()));
    error
        .evidence
        .insert("runner_id".into(), ScalarValue::String(runner_id.into()));
}
