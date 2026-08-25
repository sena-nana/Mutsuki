//! MutsukiTauriHost `.momoplug` ABI v2 端到端测试插件。
// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::doc_markdown,
    clippy::must_use_candidate,
    clippy::needless_pass_by_value
)]

use mutsuki_runtime_contracts::{
    ArtifactType, CompletionBatch, PluginArtifact, PluginManifest, RunnerDescriptor, WorkBatch,
};
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeResult};
use mutsuki_runtime_sdk::{
    AbiHostClient, PluginBuilder, RunnerDescriptorBuilder, map_work_batch_entries,
};
use serde_json::{Value, json};

pub const PLUGIN_ID: &str = "mutsuki.test.tauri-abi";
pub const RUNNER_ID: &str = "mutsuki.test.tauri-abi.runner";
pub const PROTOCOL_ID: &str = "mutsuki.test.tauri-abi.echo";

struct FixtureRunner {
    descriptor: RunnerDescriptor,
}

impl FixtureRunner {
    fn new() -> Self {
        Self {
            descriptor: RunnerDescriptorBuilder::new(RUNNER_ID, PLUGIN_ID)
                .accepted_protocol(PROTOCOL_ID)
                .build(),
        }
    }
}

impl Runner for FixtureRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        map_work_batch_entries(&batch, |task| {
            let mut result =
                mutsuki_runtime_contracts::RunnerResult::completed(task.task_id.clone());
            result.output = Some(json!({ "echo": task.payload }));
            Ok(result)
        })
    }
}

pub fn fixture_manifest(path: &str, sha256: &str) -> PluginManifest {
    build_plugin(path, sha256).manifest
}

fn create_plugin(
    _host: AbiHostClient,
    config: Value,
) -> RuntimeResult<mutsuki_runtime_sdk::LoadedPlugin> {
    if config.get("enabled").and_then(Value::as_bool) != Some(true) {
        return Err(mutsuki_runtime_core::RuntimeFailure::new(
            mutsuki_runtime_contracts::RuntimeError::new(
                mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
                PLUGIN_ID,
                "fixture.config_required",
            ),
        ));
    }
    let metadata = config.get("_mutsuki").and_then(Value::as_object);
    let valid_metadata = metadata
        .and_then(|metadata| metadata.get("plugin_id"))
        .and_then(Value::as_str)
        == Some(PLUGIN_ID)
        && metadata
            .and_then(|metadata| metadata.get("package_hash"))
            .and_then(Value::as_str)
            .is_some_and(|hash| hash.len() == 64)
        && metadata
            .and_then(|metadata| metadata.get("runtime_dir"))
            .and_then(Value::as_str)
            .is_some_and(|path| std::path::Path::new(path).is_dir())
        && config.get("caller").and_then(Value::as_str) == Some("preserved");
    if !valid_metadata {
        return Err(mutsuki_runtime_core::RuntimeFailure::new(
            mutsuki_runtime_contracts::RuntimeError::new(
                mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
                PLUGIN_ID,
                "fixture.runtime_metadata_required",
            ),
        ));
    }
    Ok(build_plugin("fixture", "sha256:fixture"))
}

fn build_plugin(path: &str, sha256: &str) -> mutsuki_runtime_sdk::LoadedPlugin {
    PluginBuilder::new(PLUGIN_ID)
        .runner(Box::new(FixtureRunner::new()))
        .artifact(PluginArtifact {
            artifact_type: ArtifactType::Abi,
            path: path.into(),
            sha256: sha256.into(),
            companion_artifacts: Vec::new(),
        })
        .build()
}

mutsuki_runtime_sdk::export_mutsuki_plugin_abi_v2!(create_plugin);
