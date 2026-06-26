use std::collections::BTreeMap;

use mutsuki_runtime_contracts::{RunnerDescriptor, RuntimeLoadPlan, ScalarValue};

pub(super) fn runner_attrs(
    runner: &RunnerDescriptor,
    load_plan: &RuntimeLoadPlan,
) -> BTreeMap<String, ScalarValue> {
    let mut attrs = BTreeMap::new();
    attrs.insert(
        "runner_id".into(),
        ScalarValue::String(runner.runner_id.clone()),
    );
    attrs.insert(
        "plugin_id".into(),
        ScalarValue::String(runner.plugin_id.clone()),
    );
    attrs.insert(
        "plugin_generation".into(),
        ScalarValue::Int(runner.plugin_generation as i64),
    );
    attrs.insert(
        "artifact_hash".into(),
        ScalarValue::String(
            load_plan
                .plugins
                .iter()
                .find(|plugin| plugin.plugin_id == runner.plugin_id)
                .map(|plugin| plugin.artifact.sha256.clone())
                .unwrap_or_else(|| "unknown".into()),
        ),
    );
    attrs.insert(
        "descriptor_hash".into(),
        ScalarValue::String(descriptor_fingerprint(runner)),
    );
    attrs.insert(
        "contract_fingerprint".into(),
        ScalarValue::String(contract_fingerprint(runner, load_plan)),
    );
    attrs
}

pub(super) fn trace_attrs(
    span: &mutsuki_runtime_contracts::TraceSpan,
) -> BTreeMap<String, ScalarValue> {
    let mut attrs = BTreeMap::new();
    attrs.insert(
        "trace_id".into(),
        ScalarValue::String(span.trace_id.clone()),
    );
    attrs.insert("span_id".into(), ScalarValue::String(span.span_id.clone()));
    attrs.insert("span_name".into(), ScalarValue::String(span.name.clone()));
    attrs
}

fn descriptor_fingerprint(runner: &RunnerDescriptor) -> String {
    format!(
        "runner:{}:{}:{}:{}",
        runner.runner_id,
        runner.plugin_id,
        runner.plugin_generation,
        runner.accepted_task_kinds.join(",")
    )
}

fn contract_fingerprint(runner: &RunnerDescriptor, load_plan: &RuntimeLoadPlan) -> String {
    let mut fingerprints = Vec::new();
    for surface_id in &runner.contract_surfaces {
        let fingerprint = load_plan
            .contract_surfaces
            .iter()
            .find(|surface| &surface.surface_id == surface_id)
            .map(|surface| surface.fingerprint.clone())
            .unwrap_or_else(|| "missing".into());
        fingerprints.push(format!("{surface_id}={fingerprint}"));
    }
    fingerprints.sort();
    fingerprints.join(";")
}
