use std::collections::BTreeMap;

use mutsuki_runtime_contracts::{
    ExecutionClass, ObservabilityProfile, RunnerDescriptor, RunnerPurity, RuntimeProfile,
    RuntimeProfileMode,
};
use mutsuki_runtime_host::{NativeRunner, RuntimeBootstrapper, runner_manifest};
use serde_json::json;

pub const BENCH_PLUGIN_ID: &str = "mutsuki.bench.runtime";
pub const BENCH_PROTOCOL_ID: &str = "bench.work";

pub fn runner_descriptor(
    runner_id: impl Into<String>,
    protocols: Vec<String>,
    batch_size: usize,
) -> RunnerDescriptor {
    let runner_id = runner_id.into();
    let mut descriptor = RunnerDescriptor {
        runner_id: runner_id.clone(),
        plugin_id: BENCH_PLUGIN_ID.into(),
        plugin_generation: 1,
        accepted_protocol_ids: protocols,
        purity: RunnerPurity::Pure,
        execution_class: ExecutionClass::Cpu,
        invocation_mode: Default::default(),
        concurrency: Default::default(),
        input_schema: json!({}),
        output_schema: json!({}),
        batch: Default::default(),
        payload: Default::default(),
        resources: Default::default(),
        ordering: Default::default(),
        control: Default::default(),
        metadata: BTreeMap::new(),
        contract_surfaces: vec![format!("runner:{runner_id}")],
    };
    descriptor.batch.preferred_batch_size = batch_size;
    descriptor.batch.max_batch_entries = batch_size;
    descriptor.batch.max_entry_concurrency = batch_size;
    descriptor
}

pub fn runtime_profile(observability: ObservabilityProfile) -> RuntimeProfile {
    RuntimeProfile {
        profile_id: "benchmark".into(),
        mode: RuntimeProfileMode::FullDev,
        enabled_plugins: vec![BENCH_PLUGIN_ID.into()],
        bindings: BTreeMap::new(),
        plugin_deployments: BTreeMap::new(),
        observability,
        allow_dynamic_registration: false,
        allow_hot_reload: true,
    }
}

pub fn echo_bootstrapper(descriptor: RunnerDescriptor) -> RuntimeBootstrapper {
    let mut bootstrapper = RuntimeBootstrapper::new();
    bootstrapper.register_manifest(runner_manifest(BENCH_PLUGIN_ID, vec![descriptor.clone()]));
    bootstrapper.register_runner(Box::new(NativeRunner::new_borrowed(
        descriptor,
        |_ctx, task| {
            Ok(mutsuki_runtime_contracts::RunnerResult::completed(
                task.task_id.clone(),
            ))
        },
    )));
    bootstrapper
}
