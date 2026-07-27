use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime};

use mutsuki_runtime_contracts::resource::experimental::{CommandBatch, SagaPlan};
use mutsuki_runtime_contracts::{
    ArtifactType, CancelPolicy, CommandPlan, CompanionArtifact, ExportPlan, PlanReceipt,
    PluginArtifact, PluginManifest, ReadPlan, SnapshotDescriptor, StreamPlan, TaskBatch,
    TaskHandle, TaskOutcome, WritePlan,
};
use mutsuki_runtime_core::{RuntimeFailure, RuntimeResult};
use mutsuki_runtime_sdk::{
    PluginBuilder, ResourcePlanGateway, RunnerDescriptorBuilder, TaskSubmitter,
};
use serde_json::json;

use crate::{AbiPluginLoadRequest, NativeRunner, load_abi_plugin_v2};

const PLUGIN_ID: &str = "mutsuki.test.runtime-host-abi-v2";
const RUNNER_ID: &str = "mutsuki.test.runtime-host-abi-v2.runner";
const PROVIDER_ID: &str = "mutsuki.test.runtime-host-abi-v2.provider";
const PROTOCOL_ID: &str = "mutsuki.test.runtime-host-abi-v2.echo";

#[test]
fn real_abi_v2_library_loads_callbacks_resources_and_closes() {
    let library_path = build_real_fixture();
    let close_marker = fixture_output_dir().join(format!(
        "close-{}-{}.marker",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&close_marker);
    let gateways = Arc::new(TestHostGateways::default());
    let plugin = load(
        library_path,
        expected_manifest(),
        json!({
            "host_callback": true,
            "close_marker": close_marker.to_string_lossy()
        }),
        gateways.clone(),
    )
    .unwrap();

    assert_eq!(gateways.submitted.load(Ordering::SeqCst), 1);
    assert_eq!(plugin.runners.len(), 1);
    assert_eq!(plugin.resource_providers.len(), 1);
    let provider = plugin.resource_providers[0].provider.as_ref();
    let resource = provider
        .create_blob_resource("fixture.v1", b"input".to_vec())
        .unwrap();
    assert_eq!(
        provider
            .collect_read_plan(&ReadPlan {
                plan_id: "fixture-read".into(),
                resource,
                operation: "collect".into(),
                args: json!({}),
            })
            .unwrap(),
        b"fixture-resource"
    );

    drop(plugin);
    for _ in 0..100 {
        if close_marker.is_file() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(close_marker.is_file(), "ABI close callback did not run");
    let _ = std::fs::remove_file(close_marker);
}

#[test]
fn abi_v2_loader_rejects_missing_symbol_and_wrong_version() {
    let gateways = Arc::new(TestHostGateways::default());
    let missing = expect_load_failure(load(
        compile_minimal_fixture("missing-symbol.rs", "missing_symbol"),
        expected_manifest(),
        json!({}),
        gateways.clone(),
    ));
    assert_eq!(missing.error().route, "abi.v2.symbol_missing");

    let wrong = expect_load_failure(load(
        compile_minimal_fixture("wrong-version.rs", "wrong_version"),
        expected_manifest(),
        json!({}),
        gateways,
    ));
    assert_eq!(wrong.error().route, "abi.v2.entry_invalid");
}

#[test]
fn abi_v2_loader_requires_successful_initialize_and_matching_manifest() {
    let library_path = build_real_fixture();
    let gateways = Arc::new(TestHostGateways::default());
    let initialize = expect_load_failure(load(
        library_path.clone(),
        expected_manifest(),
        json!({ "fail_initialize": true }),
        gateways.clone(),
    ));
    assert_eq!(initialize.error().route, "abi.v2.initialize");

    let mut mismatch = expected_manifest();
    mismatch.version = "9.9.9".into();
    let manifest = expect_load_failure(load(library_path, mismatch, json!({}), gateways));
    assert_eq!(manifest.error().route, "abi.v2.manifest_mismatch");
}

#[test]
fn abi_v2_loader_rejects_provider_surface_mismatch() {
    let error = expect_load_failure(load(
        build_real_fixture(),
        expected_manifest(),
        json!({ "provider_mismatch": true }),
        Arc::new(TestHostGateways::default()),
    ));
    assert_eq!(error.error().route, "abi.v2.provider_surface_mismatch");
}

#[test]
fn abi_v2_host_callback_contains_panics() {
    let gateways = Arc::new(TestHostGateways {
        panic_on_submit: true,
        ..TestHostGateways::default()
    });
    let error = expect_load_failure(load(
        build_real_fixture(),
        expected_manifest(),
        json!({ "host_callback": true }),
        gateways,
    ));
    assert_eq!(error.error().route, "abi.v2.initialize");
}

fn load(
    library_path: PathBuf,
    expected_manifest: PluginManifest,
    config: serde_json::Value,
    gateways: Arc<TestHostGateways>,
) -> RuntimeResult<mutsuki_runtime_sdk::LoadedPlugin> {
    let task_submitter: Arc<dyn TaskSubmitter> = gateways.clone();
    let resource_gateway: Arc<dyn ResourcePlanGateway> = gateways;
    load_abi_plugin_v2(AbiPluginLoadRequest {
        library_path,
        expected_manifest,
        config: Some(config),
        task_submitter,
        resource_gateway,
    })
}

fn expect_load_failure(result: RuntimeResult<mutsuki_runtime_sdk::LoadedPlugin>) -> RuntimeFailure {
    match result {
        Ok(_) => panic!("ABI plugin loading unexpectedly succeeded"),
        Err(error) => error,
    }
}

fn expected_manifest() -> PluginManifest {
    let descriptor = RunnerDescriptorBuilder::new(RUNNER_ID, PLUGIN_ID)
        .accepted_protocol(PROTOCOL_ID)
        .build();
    PluginBuilder::new(PLUGIN_ID)
        .runner(Box::new(NativeRunner::new(descriptor, |_ctx, task| {
            Ok(mutsuki_runtime_contracts::RunnerResult::completed(
                task.task_id,
            ))
        })))
        .resource_provider(PROVIDER_ID)
        .artifact(PluginArtifact {
            artifact_type: ArtifactType::Abi,
            path: library_file_name().into(),
            sha256: "sha256:installed".into(),
            companion_artifacts: vec![CompanionArtifact {
                path: "helpers/fixture-helper".into(),
                sha256: "sha256:helper".into(),
                executable: true,
                role: Some("fixture".into()),
            }],
        })
        .build()
        .manifest
}

fn build_real_fixture() -> PathBuf {
    static LIBRARY: OnceLock<PathBuf> = OnceLock::new();
    LIBRARY
        .get_or_init(|| {
            let manifest = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("fixtures")
                .join("abi-v2-plugin")
                .join("Cargo.toml");
            let status = Command::new(env!("CARGO"))
                .args(["build", "--manifest-path"])
                .arg(&manifest)
                .status()
                .expect("build real ABI v2 fixture");
            assert!(status.success(), "real ABI v2 fixture build failed");
            let library = workspace_root()
                .join("target")
                .join("debug")
                .join(library_file_name());
            assert!(library.is_file(), "fixture artifact: {}", library.display());
            library
        })
        .clone()
}

fn compile_minimal_fixture(source_name: &str, crate_name: &str) -> PathBuf {
    let output = fixture_output_dir().join(dynamic_library_name(crate_name));
    if output.is_file() {
        return output;
    }
    std::fs::create_dir_all(fixture_output_dir()).unwrap();
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(source_name);
    let status = Command::new("rustc")
        .args(["--edition=2024", "--crate-type=cdylib", "--crate-name"])
        .arg(crate_name)
        .arg(&source)
        .arg("-o")
        .arg(&output)
        .status()
        .expect("compile minimal ABI fixture");
    assert!(status.success(), "minimal ABI fixture build failed");
    output
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn fixture_output_dir() -> PathBuf {
    workspace_root().join("target").join("abi-loader-fixtures")
}

fn library_file_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "mutsuki_runtime_host_abi_v2_fixture.dll"
    } else if cfg!(target_os = "macos") {
        "libmutsuki_runtime_host_abi_v2_fixture.dylib"
    } else {
        "libmutsuki_runtime_host_abi_v2_fixture.so"
    }
}

fn dynamic_library_name(crate_name: &str) -> String {
    if cfg!(target_os = "windows") {
        format!("{crate_name}.dll")
    } else if cfg!(target_os = "macos") {
        format!("lib{crate_name}.dylib")
    } else {
        format!("lib{crate_name}.so")
    }
}

#[derive(Default)]
struct TestHostGateways {
    submitted: AtomicUsize,
    panic_on_submit: bool,
}

impl TaskSubmitter for TestHostGateways {
    fn submit_batch(&self, batch: TaskBatch) -> RuntimeResult<Vec<TaskHandle>> {
        assert!(!self.panic_on_submit, "requested host callback panic");
        self.submitted
            .fetch_add(batch.tasks.len(), Ordering::SeqCst);
        Ok(batch
            .tasks
            .into_iter()
            .map(|task| TaskHandle {
                task_id: task.task_id,
                protocol_id: task.protocol_id,
                target_binding_id: task.target_binding_id,
                cancel_policy: CancelPolicy::Cascade,
                trace_id: task.trace_id,
                correlation_id: task.correlation_id,
            })
            .collect())
    }

    fn cancel_task(&self, _handle: &TaskHandle) -> RuntimeResult<()> {
        Ok(())
    }

    fn task_outcome(&self, _handle: &TaskHandle) -> RuntimeResult<Option<TaskOutcome>> {
        Ok(None)
    }
}

impl ResourcePlanGateway for TestHostGateways {
    fn collect_read_plan(&self, _plan: &ReadPlan) -> RuntimeResult<Vec<u8>> {
        Err(unsupported_host_gateway("collect"))
    }

    fn snapshot_read_plan(
        &self,
        _plan: &ReadPlan,
        _kind_id: &str,
        _schema: &str,
    ) -> RuntimeResult<SnapshotDescriptor> {
        Err(unsupported_host_gateway("snapshot"))
    }

    fn open_stream_plan(&self, _plan: &ReadPlan) -> RuntimeResult<StreamPlan> {
        Err(unsupported_host_gateway("stream"))
    }

    fn execute_export_plan(&self, _plan: &ExportPlan) -> RuntimeResult<PlanReceipt> {
        Err(unsupported_host_gateway("export"))
    }

    fn commit_write_plan(&self, _plan: &WritePlan, _bytes: Vec<u8>) -> RuntimeResult<PlanReceipt> {
        Err(unsupported_host_gateway("write"))
    }

    fn execute_command_plan(&self, _plan: &CommandPlan) -> RuntimeResult<PlanReceipt> {
        Err(unsupported_host_gateway("command"))
    }

    fn execute_command_batch(&self, _batch: &CommandBatch) -> RuntimeResult<Vec<PlanReceipt>> {
        Err(unsupported_host_gateway("command_batch"))
    }

    fn execute_saga_plan(&self, _saga: &SagaPlan) -> RuntimeResult<Vec<PlanReceipt>> {
        Err(unsupported_host_gateway("saga"))
    }
}

fn unsupported_host_gateway(route: &str) -> RuntimeFailure {
    RuntimeFailure::new(mutsuki_runtime_contracts::RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RESOURCE_UNSUPPORTED,
        "abi-v2-test-host",
        route,
    ))
}
