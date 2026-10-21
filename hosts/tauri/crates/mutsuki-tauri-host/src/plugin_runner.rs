use crate::config::MutsukiTauriConfig;
use crate::error::{HostError, HostResult};
use crate::health::HostHealthState;
use crate::plugin_abi::connect_packaged_plugin;
use crate::plugin_package::{PluginPackageRecord, scan_momoplug_packages};
use mutsuki_runtime_contracts::{
    ArtifactType, CompletionBatch, PluginDeploymentKind, PluginId, PluginManifest, PluginProvides,
    ProtocolClass, ProtocolId, RunnerDescriptor, RuntimeError, ScalarValue, WorkBatch,
};
use mutsuki_runtime_host::DeferredHostClient;
use mutsuki_runtime_host::{
    ProcessRunnerSpec, RuntimeBootstrapper, SpawnedBinaryRunner, runner_manifest,
};
use mutsuki_runtime_plugin_loader::{
    ExternalRuntimeSpec, PluginToml, read_plugin_toml, validate_plugin_toml,
};
use mutsuki_runtime_sdk::{
    AsyncBatchHandler, Runner, RunnerContext, RuntimeFailure, RuntimeResult,
};
use mutsuki_runtime_sdk::{LoadedPlugin, PluginBuilder, RuntimeClientRef};
use mutsuki_tauri_bridge::{
    EventHub, FrontendLogRecord, MutsukiFrontendEvent, PluginSummary, RunnerSummary,
    redact_log_record,
};
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
    pub(crate) loaded_plugins: Vec<LoadedPlugin>,
    pub(crate) process_runners: Vec<Box<dyn Runner>>,
    pub(crate) plugins: Vec<PluginSummary>,
    pub(crate) runners: Vec<RunnerSummary>,
    pub(crate) packages: Vec<PluginPackageRecord>,
}

pub(crate) struct DiscoveredPluginState {
    pub(crate) enabled_plugins: BTreeSet<PluginId>,
    pub(crate) plugin_deployments: BTreeMap<PluginId, PluginDeploymentKind>,
    pub(crate) active_protocols: BTreeSet<ProtocolId>,
}

pub(crate) type BuiltinRunnerFactory = Arc<dyn Fn() -> Box<dyn Runner> + Send + Sync + 'static>;
pub(crate) type RuntimeClientRunnerFactory =
    Arc<dyn Fn(RuntimeClientRef) -> Box<dyn Runner> + Send + Sync + 'static>;
pub(crate) type BuiltinAsyncHandlerFactory =
    Arc<dyn Fn() -> Arc<dyn AsyncBatchHandler> + Send + Sync + 'static>;

pub(crate) fn scan_plugin_runners(
    config: &MutsukiTauriConfig,
    events: Arc<EventHub>,
    health: Arc<HostHealthState>,
    abi_host: Arc<DeferredHostClient>,
) -> HostResult<PluginRunnerLoad> {
    let plugin_files = find_named_files(&config.paths.plugins_dir, "plugin.toml")?;

    let mut plugins = Vec::new();
    let mut runners = Vec::new();
    let mut manifests_by_id = BTreeMap::<PluginId, (PluginToml, PathBuf)>::new();
    let mut seen_plugin_ids = BTreeSet::<PluginId>::new();
    let packages = scan_momoplug_packages(config)?;
    let mut loaded_plugins = Vec::new();
    for package in &packages {
        if let Some(error) = package.error.as_deref() {
            plugins.push(failed_plugin_with_deployment(
                package.plugin_id.clone(),
                package.version.clone(),
                "package",
                error,
            ));
            continue;
        }
        if !package.executable {
            continue;
        }
        if !seen_plugin_ids.insert(PluginId::from(package.plugin_id.as_str())) {
            plugins.push(failed_plugin_with_deployment(
                package.plugin_id.clone(),
                package.version.clone(),
                "package",
                "duplicate plugin id",
            ));
            continue;
        }
        if !package.selected {
            plugins.push(disabled_plugin(package));
            continue;
        }
        match connect_packaged_plugin(package, abi_host.clone()) {
            Ok(plugin) => {
                let deployment = PluginDeploymentKind::default_for_artifact(
                    &plugin.manifest.artifact.artifact_type,
                );
                plugins.push(loaded_plugin_summary(&plugin.manifest, deployment.clone()));
                runners.extend(plugin.manifest.provides.runners.iter().map(|descriptor| {
                    loaded_runner_summary(descriptor, deployment_label(&deployment))
                }));
                loaded_plugins.push(plugin);
            }
            Err(error) => {
                let deployment = package
                    .runtime_manifest
                    .as_ref()
                    .map(|manifest| {
                        PluginDeploymentKind::default_for_artifact(&manifest.artifact.artifact_type)
                    })
                    .map(|deployment| deployment_label(&deployment))
                    .unwrap_or("package");
                plugins.push(failed_plugin_with_deployment(
                    package.plugin_id.clone(),
                    package.version.clone(),
                    deployment,
                    error,
                ));
            }
        }
    }
    for path in plugin_files {
        let plugin = read_plugin_toml(&path)
            .and_then(|plugin| validate_plugin_toml(&plugin).map(|()| plugin))
            .map_err(|error| error.to_string());
        match plugin {
            Ok(plugin) if !seen_plugin_ids.insert(plugin.manifest.plugin_id.clone()) => {
                plugins.push(failed_plugin_with_deployment(
                    plugin.manifest.plugin_id,
                    plugin.manifest.version,
                    deployment_label(&PluginDeploymentKind::default_for_artifact(
                        &plugin.manifest.artifact.artifact_type,
                    )),
                    "duplicate plugin id",
                ));
            }
            Ok(plugin) => {
                manifests_by_id.insert(plugin.manifest.plugin_id.clone(), (plugin, path));
            }
            Err(error) => plugins.push(failed_plugin(
                invalid_id(&config.paths.plugins_dir, &path),
                "unknown",
                error,
            )),
        }
    }

    let mut load = PluginRunnerLoad {
        manifests: Vec::new(),
        loaded_plugins,
        process_runners: Vec::new(),
        plugins,
        runners,
        packages,
    };

    for (plugin_id, (plugin, manifest_path)) in manifests_by_id {
        let PluginToml { manifest, runtime } = plugin;
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

        let runtime = match process_runtime(runtime.as_ref(), &manifest_path) {
            Ok(runtime) => runtime,
            Err(error) => {
                load.runners
                    .extend(manifest.provides.runners.iter().map(|descriptor| {
                        failed_runner(
                            descriptor.runner_id.clone(),
                            plugin_id.clone(),
                            "process",
                            error.clone(),
                        )
                    }));
                load.plugins
                    .push(failed_plugin(plugin_id, manifest.version, error));
                continue;
            }
        };

        let mut plugin_runners = Vec::new();
        let mut plugin_runner_summaries = Vec::new();
        let mut plugin_error = None;
        for descriptor in &manifest.provides.runners {
            match ExternalProcessRunner::spawn(
                descriptor.clone(),
                &runtime,
                &manifest_path,
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

    load.plugins
        .sort_by(|left, right| left.plugin_id.cmp(&right.plugin_id));
    load.runners
        .sort_by(|left, right| left.runner_id.cmp(&right.runner_id));
    Ok(load)
}

/// The `[runtime]` table a process plugin needs, with paths resolved against its `plugin.toml`.
fn process_runtime(
    runtime: Option<&ExternalRuntimeSpec>,
    manifest_path: &Path,
) -> Result<ExternalRuntimeSpec, String> {
    let runtime = runtime.ok_or_else(|| {
        format!(
            "process plugin {} has no [runtime] table",
            manifest_path.display()
        )
    })?;
    if runtime.runner_link != "binary-stdio" {
        return Err(format!(
            "runner_link {} is not supported by the desktop Host; use binary-stdio",
            runtime.runner_link
        ));
    }
    let plugin_dir = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    Ok(runtime.resolved_against(plugin_dir))
}

pub(crate) fn loaded_builtin_plugin_summary(manifest: &PluginManifest) -> PluginSummary {
    loaded_plugin_summary(manifest, PluginDeploymentKind::Builtin)
}

pub(crate) fn register_discovered_plugins(
    load: &mut PluginRunnerLoad,
    bootstrapper: &mut RuntimeBootstrapper,
) -> DiscoveredPluginState {
    let mut enabled_plugins = BTreeSet::new();
    let mut plugin_deployments = BTreeMap::new();
    let mut active_protocols = BTreeSet::new();
    for plugin in std::mem::take(&mut load.loaded_plugins) {
        let deployment =
            PluginDeploymentKind::default_for_artifact(&plugin.manifest.artifact.artifact_type);
        active_protocols.extend(
            plugin
                .manifest
                .provides
                .runners
                .iter()
                .flat_map(|runner| runner.accepted_protocol_ids.iter().cloned()),
        );
        enabled_plugins.insert(plugin.manifest.plugin_id.clone());
        plugin_deployments.insert(plugin.manifest.plugin_id.clone(), deployment);
        bootstrapper.register_loaded_plugin(plugin);
    }
    for manifest in std::mem::take(&mut load.manifests) {
        active_protocols.extend(
            manifest
                .provides
                .runners
                .iter()
                .flat_map(|runner| runner.accepted_protocol_ids.iter().cloned()),
        );
        enabled_plugins.insert(manifest.plugin_id.clone());
        plugin_deployments.insert(manifest.plugin_id.clone(), PluginDeploymentKind::Process);
        bootstrapper.register_manifest(manifest);
    }
    for runner in std::mem::take(&mut load.process_runners) {
        bootstrapper.register_external_runner(PluginDeploymentKind::Process, runner);
    }
    DiscoveredPluginState {
        enabled_plugins,
        plugin_deployments,
        active_protocols,
    }
}

/// 将已启用且完成包校验的插件声明权限注册为本 generation 的桌面授权能力。
pub(crate) fn declared_permission_grant_manifest(
    load: &PluginRunnerLoad,
) -> Option<PluginManifest> {
    let effects = load
        .loaded_plugins
        .iter()
        .map(|plugin| &plugin.manifest)
        .chain(load.manifests.iter())
        .flat_map(|manifest| manifest.permissions.effects.iter().cloned())
        .collect::<BTreeSet<_>>();
    if effects.is_empty() {
        return None;
    }
    let provides = PluginProvides {
        protocol_classes: effects
            .iter()
            .map(|effect| (ProtocolId::from(effect.as_str()), ProtocolClass::Effect))
            .collect(),
        effects: effects.into_iter().collect(),
        ..PluginProvides::default()
    };
    Some(
        PluginBuilder::new("mutsuki.tauri.approved-permissions")
            .provides(provides)
            .build()
            .manifest,
    )
}

/// 把桌面授权能力加入 load plan；它不作为用户插件显示或执行。
pub(crate) fn register_permission_grants(
    manifest: Option<PluginManifest>,
    bootstrapper: &mut RuntimeBootstrapper,
    state: &mut DiscoveredPluginState,
) {
    let Some(manifest) = manifest else {
        return;
    };
    state.enabled_plugins.insert(manifest.plugin_id.clone());
    state
        .plugin_deployments
        .insert(manifest.plugin_id.clone(), PluginDeploymentKind::Builtin);
    bootstrapper.register_manifest(manifest);
}

pub(crate) fn register_builtin_runtime(
    load: &mut PluginRunnerLoad,
    bootstrapper: &mut RuntimeBootstrapper,
    runners: Vec<Box<dyn Runner>>,
    async_handlers: Vec<Arc<dyn AsyncBatchHandler>>,
    state: &mut DiscoveredPluginState,
) -> HostResult<()> {
    let mut plugin_runners: BTreeMap<PluginId, Vec<_>> = BTreeMap::new();
    for descriptor in runners
        .iter()
        .map(|runner| runner.descriptor().clone())
        .chain(
            async_handlers
                .iter()
                .map(|handler| handler.descriptor().clone()),
        )
    {
        plugin_runners
            .entry(descriptor.plugin_id.clone())
            .or_default()
            .push(descriptor);
    }
    for (plugin_id, descriptors) in plugin_runners {
        if state.enabled_plugins.contains(&plugin_id) {
            return Err(HostError::Config(format!(
                "builtin runner plugin id conflicts with discovered plugin: {plugin_id}"
            )));
        }
        let manifest = runner_manifest(plugin_id.as_str(), descriptors.clone());
        load.plugins.push(loaded_builtin_plugin_summary(&manifest));
        for descriptor in &descriptors {
            state
                .active_protocols
                .extend(descriptor.accepted_protocol_ids.iter().cloned());
            load.runners.push(loaded_builtin_runner_summary(descriptor));
        }
        state.enabled_plugins.insert(plugin_id.clone());
        state
            .plugin_deployments
            .insert(plugin_id, PluginDeploymentKind::Builtin);
        bootstrapper.register_manifest(manifest);
    }
    for runner in runners {
        bootstrapper.register_builtin_runner(runner);
    }
    for handler in async_handlers {
        bootstrapper.register_async_handler(handler);
    }
    load.plugins
        .sort_by(|left, right| left.plugin_id.cmp(&right.plugin_id));
    load.runners
        .sort_by(|left, right| left.runner_id.cmp(&right.runner_id));
    Ok(())
}

pub(crate) fn loaded_builtin_runner_summary(descriptor: &RunnerDescriptor) -> RunnerSummary {
    loaded_runner_summary(descriptor, "builtin")
}

fn ensure_process_plugin_backend(mut manifest: PluginManifest) -> PluginManifest {
    // A manifest that already declares any process backend keeps it as is.
    if manifest
        .provides
        .plugin_backends
        .iter()
        .any(|backend| backend.deployment_kind == PluginDeploymentKind::Process)
    {
        return manifest;
    }
    let plugin_id = manifest.plugin_id.to_string();
    mutsuki_runtime_sdk::ensure_plugin_backend_descriptors(
        &plugin_id,
        &mut manifest.provides,
        &PluginDeploymentKind::Process,
    );
    manifest
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
        plugin_id: manifest.plugin_id.to_string(),
        version: manifest.version.clone(),
        enabled: true,
        deployment: deployment_label(&deployment).into(),
        status: "loaded".into(),
        error: None,
    }
}

fn loaded_runner_summary(descriptor: &RunnerDescriptor, deployment: &str) -> RunnerSummary {
    RunnerSummary {
        runner_id: descriptor.runner_id.to_string(),
        plugin_id: descriptor.plugin_id.to_string(),
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

fn disabled_plugin(package: &PluginPackageRecord) -> PluginSummary {
    let deployment = package
        .runtime_manifest
        .as_ref()
        .map(|manifest| {
            PluginDeploymentKind::default_for_artifact(&manifest.artifact.artifact_type)
        })
        .map(|deployment| deployment_label(&deployment))
        .unwrap_or("package");
    PluginSummary {
        plugin_id: package.plugin_id.clone(),
        version: package.version.clone(),
        enabled: false,
        deployment: deployment.into(),
        status: "disabled".into(),
        error: None,
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
    inner: SpawnedBinaryRunner,
    stderr_thread: Option<thread::JoinHandle<()>>,
    events: Arc<EventHub>,
    health: Arc<HostHealthState>,
}

impl ExternalProcessRunner {
    fn spawn(
        descriptor: RunnerDescriptor,
        runtime: &ExternalRuntimeSpec,
        manifest_path: &Path,
        profile_id: &str,
        events: Arc<EventHub>,
        health: Arc<HostHealthState>,
    ) -> HostResult<Self> {
        let session_token = Uuid::new_v4().to_string();

        let mut env = BTreeMap::new();
        inherit_required_env(&mut env);
        for (key, value) in &runtime.env {
            env.insert(key.clone(), value.clone());
        }
        env.insert("MUTSUKI_RUNNER_SESSION_TOKEN".into(), session_token);
        env.insert("MUTSUKI_PLUGIN_ID".into(), descriptor.plugin_id.to_string());
        env.insert("MUTSUKI_RUNNER_ID".into(), descriptor.runner_id.to_string());
        env.insert("MUTSUKI_PROFILE_ID".into(), profile_id.into());
        let spec = ProcessRunnerSpec {
            command: PathBuf::from(&runtime.command),
            args: runtime.args.clone(),
            cwd: runtime.cwd.clone(),
            env,
        };
        let mut inner = SpawnedBinaryRunner::spawn(descriptor.clone(), &spec).map_err(|error| {
            HostError::Config(format!(
                "failed to spawn runner {} from {}: {error}",
                descriptor.runner_id,
                manifest_path.display()
            ))
        })?;
        let stderr = inner.take_stderr().ok_or_else(|| {
            HostError::Config(format!(
                "runner {} stderr was not piped",
                descriptor.runner_id
            ))
        })?;
        let stderr_thread = spawn_stderr_forwarder(
            descriptor.plugin_id.to_string(),
            descriptor.runner_id.to_string(),
            stderr,
            events.clone(),
        )?;
        emit_runner_status(&events, descriptor.runner_id.as_str(), "started");
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
        emit_runner_status(&self.events, self.descriptor.runner_id.as_str(), "stopped");
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
                self.descriptor.runner_id.as_str(),
                self.descriptor.plugin_id.as_str(),
                error.error(),
            );
            emit_runner_status(&self.events, self.descriptor.runner_id.as_str(), "failed");
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

/// The same base environment ServiceHost's default `env_allowlist` passes to runners; a plugin
/// cannot ask for further host variables, only set its own in `[runtime].env`.
fn inherit_required_env(env: &mut BTreeMap<String, String>) {
    for key in ["PATH", "SystemRoot", "WINDIR", "ComSpec", "PATHEXT"] {
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
