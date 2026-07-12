use crate::config::MutsukiTauriConfig;
use crate::error::{HostError, HostResult};
use crate::health::HostHealthState;
use mutsuki_runtime_contracts::{
    ArtifactType, CompletionBatch, HostExtensionDescriptor, HostExtensionKind,
    PluginBackendDescriptor, PluginDeploymentKind, PluginManifest, RunnerDescriptor, RuntimeError,
    ScalarValue, WorkBatch,
};
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeFailure, RuntimeResult};
use mutsuki_runtime_host::{ProcessRunnerSpec, SpawnedJsonlRunner};
use mutsuki_tauri_bridge::{
    EventHub, FrontendLogRecord, MutsukiFrontendEvent, PluginSummary, RunnerSummary,
    redact_log_record,
};
use serde::Deserialize;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::ChildStderr;
use std::sync::Arc;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

pub(crate) struct PluginRunnerLoad {
    pub(crate) manifests: Vec<PluginManifest>,
    pub(crate) process_runners: Vec<Box<dyn Runner>>,
    pub(crate) plugins: Vec<PluginSummary>,
    pub(crate) runners: Vec<RunnerSummary>,
}

#[derive(Clone, Debug, Deserialize)]
struct RunnerLaunchSpec {
    runner_id: String,
    plugin_id: String,
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    cwd: Option<PathBuf>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    env_inherit: Vec<String>,
}

#[derive(Clone, Debug)]
struct LocatedRunnerSpec {
    spec: RunnerLaunchSpec,
    source: PathBuf,
}

pub(crate) fn scan_plugin_runners(
    config: &MutsukiTauriConfig,
    events: Arc<EventHub>,
    health: Arc<HostHealthState>,
) -> HostResult<PluginRunnerLoad> {
    let plugin_files = find_named_files(&config.paths.plugins_dir, "plugin.toml")?;
    let runner_files = find_named_files(&config.paths.runners_dir, "runner.toml")?;

    let mut plugins = Vec::new();
    let mut runners = Vec::new();
    let mut manifests_by_id = BTreeMap::new();
    let mut seen_plugin_ids = BTreeSet::new();
    for path in plugin_files {
        match read_plugin_manifest(&path) {
            Ok(manifest) if !seen_plugin_ids.insert(manifest.plugin_id.clone()) => {
                plugins.push(failed_plugin_with_deployment(
                    manifest.plugin_id,
                    manifest.version,
                    deployment_label(&PluginDeploymentKind::default_for_artifact(
                        &manifest.artifact.artifact_type,
                    )),
                    "duplicate plugin id",
                ));
            }
            Ok(manifest) => {
                manifests_by_id.insert(manifest.plugin_id.clone(), (manifest, path));
            }
            Err(error) => plugins.push(failed_plugin(
                invalid_id(&config.paths.plugins_dir, &path),
                "unknown",
                error,
            )),
        }
    }

    let mut specs_by_runner_id = BTreeMap::new();
    let mut seen_runner_ids = BTreeSet::new();
    for path in runner_files {
        match read_runner_spec(&path) {
            Ok(spec) if !seen_runner_ids.insert(spec.runner_id.clone()) => {
                runners.push(failed_runner(
                    spec.runner_id,
                    spec.plugin_id,
                    "process",
                    "duplicate runner id",
                ));
            }
            Ok(spec) => {
                specs_by_runner_id.insert(
                    spec.runner_id.clone(),
                    LocatedRunnerSpec { spec, source: path },
                );
            }
            Err(error) => runners.push(failed_runner(
                invalid_id(&config.paths.runners_dir, &path),
                String::new(),
                "process",
                error,
            )),
        }
    }

    let mut claimed_runner_specs = BTreeSet::new();
    let mut load = PluginRunnerLoad {
        manifests: Vec::new(),
        process_runners: Vec::new(),
        plugins,
        runners,
    };

    for (plugin_id, (manifest, manifest_path)) in manifests_by_id {
        if manifest.artifact.artifact_type != ArtifactType::Process {
            let deployment =
                PluginDeploymentKind::default_for_artifact(&manifest.artifact.artifact_type);
            load.plugins.push(failed_plugin_with_deployment(
                plugin_id,
                manifest.version,
                deployment_label(&deployment),
                format!(
                    "unsupported plugin artifact {:?} in {}",
                    manifest.artifact.artifact_type,
                    manifest_path.display()
                ),
            ));
            continue;
        }

        let mut plugin_runners = Vec::new();
        let mut plugin_runner_summaries = Vec::new();
        let mut plugin_error = None;
        for descriptor in &manifest.provides.runners {
            let Some(located) = specs_by_runner_id.get(&descriptor.runner_id) else {
                plugin_error = Some(format!(
                    "missing runner.toml for runner {}",
                    descriptor.runner_id
                ));
                plugin_runner_summaries.push(failed_runner(
                    descriptor.runner_id.clone(),
                    plugin_id.clone(),
                    "process",
                    "missing runner launch spec",
                ));
                continue;
            };
            claimed_runner_specs.insert(descriptor.runner_id.clone());
            if located.spec.plugin_id != plugin_id {
                plugin_error = Some(format!(
                    "runner {} belongs to plugin {}",
                    descriptor.runner_id, located.spec.plugin_id
                ));
                plugin_runner_summaries.push(failed_runner(
                    descriptor.runner_id.clone(),
                    plugin_id.clone(),
                    "process",
                    "runner launch spec plugin_id mismatch",
                ));
                continue;
            }

            match ExternalProcessRunner::spawn(
                descriptor.clone(),
                located,
                &config.profile_id,
                events.clone(),
                health.clone(),
            ) {
                Ok(runner) => {
                    plugin_runner_summaries.push(loaded_runner_summary(descriptor, "process"));
                    plugin_runners.push(Box::new(runner) as Box<dyn Runner>);
                }
                Err(error) => {
                    let message = error.to_string();
                    plugin_error = Some(message.clone());
                    plugin_runner_summaries.push(failed_runner(
                        descriptor.runner_id.clone(),
                        plugin_id.clone(),
                        "process",
                        message,
                    ));
                }
            }
        }

        if let Some(error) = plugin_error {
            for mut runner in plugin_runners {
                let _ = runner.dispose();
            }
            load.plugins
                .push(failed_plugin(plugin_id, manifest.version, error));
            load.runners.extend(plugin_runner_summaries);
            continue;
        }

        let manifest = ensure_process_plugin_backend(manifest);
        load.plugins.push(loaded_plugin_summary(
            &manifest,
            PluginDeploymentKind::Process,
        ));
        load.runners.extend(plugin_runner_summaries);
        load.process_runners.extend(plugin_runners);
        load.manifests.push(manifest);
    }

    for (runner_id, located) in specs_by_runner_id {
        if !claimed_runner_specs.contains(&runner_id) {
            load.runners.push(failed_runner(
                runner_id,
                located.spec.plugin_id,
                "process",
                "runner spec is not referenced by a loaded plugin manifest",
            ));
        }
    }

    load.plugins
        .sort_by(|left, right| left.plugin_id.cmp(&right.plugin_id));
    load.runners
        .sort_by(|left, right| left.runner_id.cmp(&right.runner_id));
    Ok(load)
}

pub(crate) fn loaded_builtin_plugin_summary(manifest: &PluginManifest) -> PluginSummary {
    loaded_plugin_summary(manifest, PluginDeploymentKind::Builtin)
}

pub(crate) fn loaded_builtin_runner_summary(descriptor: &RunnerDescriptor) -> RunnerSummary {
    loaded_runner_summary(descriptor, "builtin")
}

fn read_plugin_manifest(path: &Path) -> Result<PluginManifest, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    toml::from_str(&text).map_err(|error| format!("failed to parse {}: {error}", path.display()))
}

fn ensure_process_plugin_backend(mut manifest: PluginManifest) -> PluginManifest {
    let has_process_backend = manifest
        .provides
        .plugin_backends
        .iter()
        .any(|backend| backend.deployment_kind == PluginDeploymentKind::Process);
    if has_process_backend {
        return manifest;
    }

    let plugin_id = &manifest.plugin_id;
    manifest
        .provides
        .host_extensions
        .push(HostExtensionDescriptor {
            extension_id: format!("host.extension.{plugin_id}.process"),
            kind: HostExtensionKind::PluginBackend,
            supported_deployments: vec![PluginDeploymentKind::Process],
            reload_policy: "drain_and_swap".into(),
            drain_required: true,
        });
    manifest
        .provides
        .plugin_backends
        .push(PluginBackendDescriptor {
            backend_id: format!("plugin.backend.{plugin_id}.process"),
            deployment_kind: PluginDeploymentKind::Process,
            task_client_protocol: "mutsuki.task.v1".into(),
            resource_client_protocol: "mutsuki.resource-plan.v1".into(),
            codec_id: None,
            bridge_id: None,
        });
    manifest
}

fn read_runner_spec(path: &Path) -> Result<RunnerLaunchSpec, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    toml::from_str(&text).map_err(|error| format!("failed to parse {}: {error}", path.display()))
}

fn find_named_files(root: &Path, file_name: &str) -> HostResult<Vec<PathBuf>> {
    let mut files = Vec::new();
    if !root.exists() {
        return Ok(files);
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut entries = fs::read_dir(&dir)
            .map_err(|error| {
                HostError::Config(format!("failed to read {}: {error}", dir.display()))
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| HostError::Config(error.to_string()))?;
        entries.sort_by_key(|entry| entry.path());
        for entry in entries {
            let path = entry.path();
            let file_type = entry.file_type().map_err(|error| {
                HostError::Config(format!("failed to inspect {}: {error}", path.display()))
            })?;
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file()
                && path.file_name().and_then(|name| name.to_str()) == Some(file_name)
            {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

fn loaded_plugin_summary(
    manifest: &PluginManifest,
    deployment: PluginDeploymentKind,
) -> PluginSummary {
    PluginSummary {
        plugin_id: manifest.plugin_id.clone(),
        version: manifest.version.clone(),
        enabled: true,
        deployment: deployment_label(&deployment).into(),
        status: "loaded".into(),
        error: None,
    }
}

fn loaded_runner_summary(descriptor: &RunnerDescriptor, deployment: &str) -> RunnerSummary {
    RunnerSummary {
        runner_id: descriptor.runner_id.clone(),
        plugin_id: descriptor.plugin_id.clone(),
        enabled: true,
        deployment: deployment.into(),
        status: "loaded".into(),
        error: None,
    }
}

fn failed_plugin(
    plugin_id: impl Into<String>,
    version: impl Into<String>,
    error: impl Into<String>,
) -> PluginSummary {
    failed_plugin_with_deployment(plugin_id, version, "process", error)
}

fn failed_plugin_with_deployment(
    plugin_id: impl Into<String>,
    version: impl Into<String>,
    deployment: impl Into<String>,
    error: impl Into<String>,
) -> PluginSummary {
    PluginSummary {
        plugin_id: plugin_id.into(),
        version: version.into(),
        enabled: false,
        deployment: deployment.into(),
        status: "failed".into(),
        error: Some(error.into()),
    }
}

fn failed_runner(
    runner_id: impl Into<String>,
    plugin_id: impl Into<String>,
    deployment: impl Into<String>,
    error: impl Into<String>,
) -> RunnerSummary {
    RunnerSummary {
        runner_id: runner_id.into(),
        plugin_id: plugin_id.into(),
        enabled: false,
        deployment: deployment.into(),
        status: "failed".into(),
        error: Some(error.into()),
    }
}

fn invalid_id(root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    format!("invalid:{}", relative.display())
}

fn deployment_label(deployment: &PluginDeploymentKind) -> &'static str {
    match deployment {
        PluginDeploymentKind::Builtin => "builtin",
        PluginDeploymentKind::Abi => "abi",
        PluginDeploymentKind::Wasm => "wasm",
        PluginDeploymentKind::Process => "process",
        PluginDeploymentKind::Python => "python",
    }
}

pub(crate) struct ExternalProcessRunner {
    descriptor: RunnerDescriptor,
    inner: SpawnedJsonlRunner,
    stderr_thread: Option<thread::JoinHandle<()>>,
    events: Arc<EventHub>,
    health: Arc<HostHealthState>,
}

impl ExternalProcessRunner {
    fn spawn(
        descriptor: RunnerDescriptor,
        located: &LocatedRunnerSpec,
        profile_id: &str,
        events: Arc<EventHub>,
        health: Arc<HostHealthState>,
    ) -> HostResult<Self> {
        let source_dir = located.source.parent().ok_or_else(|| {
            HostError::Config(format!(
                "runner spec has no parent: {}",
                located.source.display()
            ))
        })?;
        let cwd = resolve_cwd(source_dir, located.spec.cwd.as_deref());
        let command_path = resolve_command(source_dir, &located.spec.command);
        let session_token = Uuid::new_v4().to_string();

        let mut env = BTreeMap::new();
        inherit_required_env(&mut env);
        for key in &located.spec.env_inherit {
            if let Ok(value) = std::env::var(key) {
                env.insert(key.clone(), value);
            }
        }
        for (key, value) in &located.spec.env {
            env.insert(key.clone(), value.clone());
        }
        env.insert("MUTSUKI_RUNNER_SESSION_TOKEN".into(), session_token);
        env.insert("MUTSUKI_PLUGIN_ID".into(), descriptor.plugin_id.clone());
        env.insert("MUTSUKI_RUNNER_ID".into(), descriptor.runner_id.clone());
        env.insert("MUTSUKI_PROFILE_ID".into(), profile_id.into());
        let spec = ProcessRunnerSpec {
            command: command_path,
            args: located.spec.args.clone(),
            cwd: Some(cwd),
            env,
        };
        let mut inner = SpawnedJsonlRunner::spawn(descriptor.clone(), &spec).map_err(|error| {
            HostError::Config(format!(
                "failed to spawn runner {} from {}: {error}",
                descriptor.runner_id,
                located.source.display()
            ))
        })?;
        let stderr = inner.take_stderr().ok_or_else(|| {
            HostError::Config(format!(
                "runner {} stderr was not piped",
                descriptor.runner_id
            ))
        })?;
        let stderr_thread = spawn_stderr_forwarder(
            descriptor.plugin_id.clone(),
            descriptor.runner_id.clone(),
            stderr,
            events.clone(),
        )?;
        emit_runner_status(&events, &descriptor.runner_id, "started");
        Ok(Self {
            descriptor: descriptor.clone(),
            inner,
            stderr_thread: Some(stderr_thread),
            events,
            health,
        })
    }

    fn stop_child(&mut self) -> RuntimeResult<()> {
        let _ = self.inner.kill();
        if let Err(error) = self.inner.wait() {
            return Err(runtime_failure(
                "process.wait",
                "exception_repr",
                error.to_string(),
            ));
        }
        if let Some(handle) = self.stderr_thread.take() {
            let _ = handle.join();
        }
        emit_runner_status(&self.events, &self.descriptor.runner_id, "stopped");
        Ok(())
    }
}

impl Runner for ExternalProcessRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        let result = self.inner.run_batch(ctx, batch);
        if let Err(error) = &result {
            self.health.record_runner_runtime_error(
                &self.descriptor.runner_id,
                &self.descriptor.plugin_id,
                error.error(),
            );
            emit_runner_status(&self.events, &self.descriptor.runner_id, "failed");
        }
        result
    }

    fn cancel(&mut self, invocation_id: &str) -> RuntimeResult<()> {
        self.inner.cancel(invocation_id)
    }

    fn dispose(&mut self) -> RuntimeResult<()> {
        self.inner.dispose()?;
        self.stop_child()
    }
}

impl Drop for ExternalProcessRunner {
    fn drop(&mut self) {
        let _ = self.stop_child();
    }
}

fn resolve_cwd(source_dir: &Path, cwd: Option<&Path>) -> PathBuf {
    match cwd {
        Some(path) if path.is_absolute() => path.to_path_buf(),
        Some(path) => source_dir.join(path),
        None => source_dir.to_path_buf(),
    }
}

fn resolve_command(source_dir: &Path, command: &str) -> PathBuf {
    let path = PathBuf::from(command);
    if path.is_absolute() || command.contains('\\') || command.contains('/') {
        if path.is_absolute() {
            path
        } else {
            source_dir.join(path)
        }
    } else {
        path
    }
}

fn inherit_required_env(env: &mut BTreeMap<String, String>) {
    for key in ["SystemRoot", "WINDIR", "ComSpec", "PATHEXT"] {
        if let Ok(value) = std::env::var(key) {
            env.insert(key.into(), value);
        }
    }
}

fn spawn_stderr_forwarder(
    plugin_id: String,
    runner_id: String,
    stderr: ChildStderr,
    events: Arc<EventHub>,
) -> HostResult<thread::JoinHandle<()>> {
    thread::Builder::new()
        .name(format!("mutsuki-runner-stderr-{runner_id}"))
        .spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                match line {
                    Ok(line) if !line.is_empty() => {
                        emit_runner_log(&events, &plugin_id, &runner_id, line);
                        emit_runner_status(&events, &runner_id, "stderr");
                    }
                    Ok(_) => {}
                    Err(error) => {
                        emit_runner_log(
                            &events,
                            &plugin_id,
                            &runner_id,
                            format!("stderr read failed: {error}"),
                        );
                        break;
                    }
                }
            }
        })
        .map_err(|error| HostError::Config(format!("failed to spawn stderr forwarder: {error}")))
}

fn emit_runner_log(events: &EventHub, plugin_id: &str, runner_id: &str, message: String) {
    let record = redact_log_record(FrontendLogRecord {
        level: "info".into(),
        target: "mutsuki_tauri_host.runner".into(),
        message,
        timestamp_ms: current_timestamp_ms(),
        trace_id: None,
        correlation_id: None,
        fields: BTreeMap::from([
            ("plugin_id".into(), json!(plugin_id)),
            ("runner_id".into(), json!(runner_id)),
        ]),
    });
    let _ = events.emit(MutsukiFrontendEvent::Log { record });
}

fn emit_runner_status(events: &EventHub, runner_id: &str, status: &str) {
    let _ = events.emit(MutsukiFrontendEvent::Runner {
        runner_id: runner_id.into(),
        status: status.into(),
    });
}

fn current_timestamp_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or_default()
}

fn runtime_failure(
    route: &str,
    evidence_key: &str,
    evidence_value: impl Into<String>,
) -> RuntimeFailure {
    let mut error = RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
        "mutsuki_tauri_host.runner",
        route,
    );
    error.evidence.insert(
        evidence_key.into(),
        ScalarValue::String(evidence_value.into()),
    );
    RuntimeFailure::new(error)
}
