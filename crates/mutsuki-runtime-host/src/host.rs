use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use mutsuki_runtime_contracts::{
    ArtifactType, ContractSurface, ContractSurfaceKind, ExecutionClass, LifecyclePolicy,
    PermissionGrant, PluginArtifact, PluginManifest, PluginProvides, RunnerDescriptor,
    RunnerResult, RuntimeError, RuntimeLoadPlan, RuntimeProfile, Task, TaskStatus,
};
use mutsuki_runtime_core::{
    CoreKernelRunner, CoreRuntime, Runner, RunnerCompletion, RunnerContext, RunnerDispatch,
    RunnerLoad, RunnerLoopReport, RuntimeFailure, RuntimeResult,
};

pub type NativeStepHandler =
    Box<dyn FnMut(RunnerContext, Vec<Task>) -> RuntimeResult<Vec<RunnerResult>> + Send>;

pub struct NativeRunner {
    descriptor: RunnerDescriptor,
    handler: NativeStepHandler,
    cancelled: Vec<String>,
    disposed: bool,
}

impl NativeRunner {
    pub fn new(
        descriptor: RunnerDescriptor,
        handler: impl FnMut(RunnerContext, Vec<Task>) -> RuntimeResult<Vec<RunnerResult>>
        + Send
        + 'static,
    ) -> Self {
        Self {
            descriptor,
            handler: Box::new(handler),
            cancelled: Vec::new(),
            disposed: false,
        }
    }
}

impl Runner for NativeRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn step(&mut self, ctx: RunnerContext, tasks: Vec<Task>) -> RuntimeResult<Vec<RunnerResult>> {
        (self.handler)(ctx, tasks)
    }

    fn cancel(&mut self, invocation_id: &str) -> RuntimeResult<()> {
        self.cancelled.push(invocation_id.to_string());
        Ok(())
    }

    fn dispose(&mut self) -> RuntimeResult<()> {
        self.disposed = true;
        Ok(())
    }
}

#[derive(Default)]
pub struct NativePluginHost {
    manifests: Vec<PluginManifest>,
    runners: Vec<Box<dyn Runner>>,
}

impl NativePluginHost {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_manifest(&mut self, manifest: PluginManifest) {
        self.manifests.push(manifest);
    }

    pub fn register_runner(&mut self, runner: Box<dyn Runner>) {
        self.runners.push(runner);
    }

    pub fn into_runtime(self, profile: RuntimeProfile) -> RuntimeResult<CoreRuntime> {
        self.boot_core_runtime(profile)
    }

    pub fn into_host_runtime(self, profile: RuntimeProfile) -> RuntimeResult<HostRuntime> {
        self.into_host_runtime_with_config(profile, HostRuntimeConfig::default())
    }

    pub fn into_host_runtime_with_config(
        self,
        profile: RuntimeProfile,
        config: HostRuntimeConfig,
    ) -> RuntimeResult<HostRuntime> {
        HostRuntime::start(self.boot_core_runtime(profile)?, config)
    }

    fn boot_core_runtime(mut self, profile: RuntimeProfile) -> RuntimeResult<CoreRuntime> {
        let mut plan = resolve_load_plan(&self.manifests, &profile);
        let core_runner = CoreKernelRunner::new(plan.registry_generation);
        plan.plugins
            .push(core_manifest(core_runner.descriptor().clone()));
        plan.contract_surfaces.push(ContractSurface {
            surface_id: "runner:core.kernel".into(),
            kind: ContractSurfaceKind::Runner,
            owner_plugin_id: "core".into(),
            fingerprint: "sha256:core.kernel".into(),
            deprecated: false,
        });
        self.runners.push(Box::new(core_runner));
        CoreRuntime::boot(plan, self.runners)
    }
}

#[derive(Clone, Debug)]
pub struct HostRuntimeConfig {
    pub worker_threads: usize,
    pub blocking_threads: usize,
    pub pool_queue_limit: usize,
    pub default_runner_limits: RunnerLimits,
    pub runner_limits: BTreeMap<String, RunnerLimits>,
}

impl Default for HostRuntimeConfig {
    fn default() -> Self {
        Self {
            worker_threads: std::thread::available_parallelism()
                .map(usize::from)
                .unwrap_or(2)
                .max(1),
            blocking_threads: 2,
            pool_queue_limit: 1024,
            default_runner_limits: RunnerLimits::default(),
            runner_limits: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct RunnerLimits {
    pub max_running: usize,
    pub max_waiting: usize,
    pub max_inflight: usize,
    pub queue_limit: usize,
}

impl Default for RunnerLimits {
    fn default() -> Self {
        Self {
            max_running: 1,
            max_waiting: 64,
            max_inflight: 64,
            queue_limit: 1024,
        }
    }
}

pub struct HostRuntime {
    tx: mpsc::Sender<CoreActorMsg>,
    actor: Option<thread::JoinHandle<()>>,
}

impl HostRuntime {
    fn start(core: CoreRuntime, config: HostRuntimeConfig) -> RuntimeResult<Self> {
        let (tx, rx) = mpsc::channel();
        let actor_tx = tx.clone();
        let actor = thread::Builder::new()
            .name("mutsuki-core-actor".into())
            .spawn(move || core_actor_loop(core, config, rx, actor_tx))
            .map_err(|error| host_failure("host.actor.spawn", error.to_string()))?;
        Ok(Self {
            tx,
            actor: Some(actor),
        })
    }

    pub fn dispatch(&mut self, command: HostRuntimeCommand) -> RuntimeResult<HostRuntimeReply> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx
            .send(CoreActorMsg::Command(command, reply_tx))
            .map_err(|error| host_failure("host.actor.command", error.to_string()))?;
        reply_rx
            .recv()
            .map_err(|error| host_failure("host.actor.reply", error.to_string()))?
    }

    pub fn task_status(&self, task_id: &str) -> Option<TaskStatus> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx
            .send(CoreActorMsg::TaskStatus(task_id.to_string(), reply_tx))
            .ok()?;
        reply_rx.recv().ok().flatten()
    }
}

impl Drop for HostRuntime {
    fn drop(&mut self) {
        let _ = self.tx.send(CoreActorMsg::Shutdown);
        if let Some(actor) = self.actor.take() {
            let _ = actor.join();
        }
    }
}

pub enum HostRuntimeCommand {
    SubmitTask(Box<Task>),
    TickOnce,
    RunUntilIdle { max_ticks: usize },
    CancelTask(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostRuntimeReply {
    TaskSubmitted(String),
    Tick(RunnerLoopReport),
    Idle(RunnerLoopReport),
    TaskCancelled(String),
}

enum CoreActorMsg {
    Command(
        HostRuntimeCommand,
        mpsc::Sender<RuntimeResult<HostRuntimeReply>>,
    ),
    TaskStatus(String, mpsc::Sender<Option<TaskStatus>>),
    WorkerCompleted(RunnerCompletion),
    Shutdown,
}

struct CoreActorCommandOutcome {
    reply: RuntimeResult<HostRuntimeReply>,
    shutdown: bool,
}

struct WorkerPool {
    sender: mpsc::Sender<RunnerDispatch>,
    queued: Arc<AtomicUsize>,
    queue_limit: usize,
    _handles: Vec<thread::JoinHandle<()>>,
}

impl WorkerPool {
    fn new(
        name: &str,
        threads: usize,
        queue_limit: usize,
        actor_tx: mpsc::Sender<CoreActorMsg>,
    ) -> RuntimeResult<Self> {
        let (sender, receiver) = mpsc::channel::<RunnerDispatch>();
        let receiver = Arc::new(Mutex::new(receiver));
        let queued = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for index in 0..threads.max(1) {
            let receiver = receiver.clone();
            let queued = queued.clone();
            let actor_tx = actor_tx.clone();
            let thread_name = format!("mutsuki-{name}-worker-{index}");
            let handle = thread::Builder::new()
                .name(thread_name)
                .spawn(move || {
                    loop {
                        let dispatch = {
                            let receiver = receiver.lock().expect("worker receiver mutex poisoned");
                            receiver.recv()
                        };
                        let Ok(dispatch) = dispatch else {
                            break;
                        };
                        queued.fetch_sub(1, Ordering::Relaxed);
                        let completion = execute_dispatch(dispatch);
                        if actor_tx
                            .send(CoreActorMsg::WorkerCompleted(completion))
                            .is_err()
                        {
                            break;
                        }
                    }
                })
                .map_err(|error| host_failure("host.worker.spawn", error.to_string()))?;
            handles.push(handle);
        }
        Ok(Self {
            sender,
            queued,
            queue_limit,
            _handles: handles,
        })
    }

    fn available_slots(&self) -> usize {
        self.queue_limit
            .saturating_sub(self.queued.load(Ordering::Relaxed))
    }

    fn send(&self, dispatch: RunnerDispatch) -> RuntimeResult<()> {
        self.queued.fetch_add(1, Ordering::Relaxed);
        let result = self.sender.send(dispatch);
        if let Err(error) = result {
            self.queued.fetch_sub(1, Ordering::Relaxed);
            return Err(host_failure("host.worker.dispatch", error.to_string()));
        }
        Ok(())
    }
}

fn execute_dispatch(dispatch: RunnerDispatch) -> RunnerCompletion {
    let RunnerDispatch {
        mut runner,
        ctx,
        task_leases,
        tasks,
    } = dispatch;
    let results = runner.step(ctx, tasks);
    RunnerCompletion {
        runner,
        task_leases,
        results,
    }
}

fn core_actor_loop(
    mut core: CoreRuntime,
    config: HostRuntimeConfig,
    rx: mpsc::Receiver<CoreActorMsg>,
    actor_tx: mpsc::Sender<CoreActorMsg>,
) {
    let mut pools = match worker_pools(&config, actor_tx) {
        Ok(pools) => pools,
        Err(_) => return,
    };
    let mut pending_cancels: BTreeMap<String, Vec<String>> = BTreeMap::new();
    while let Ok(msg) = rx.recv() {
        match msg {
            CoreActorMsg::Command(command, reply_tx) => {
                let outcome = handle_command(
                    command,
                    &mut core,
                    &config,
                    &mut pools,
                    &rx,
                    &mut pending_cancels,
                );
                let _ = reply_tx.send(outcome.reply);
                if outcome.shutdown {
                    break;
                }
            }
            CoreActorMsg::TaskStatus(task_id, reply_tx) => {
                let _ = reply_tx.send(core.task_status(&task_id));
            }
            CoreActorMsg::WorkerCompleted(mut completion) => {
                apply_pending_cancels(&mut completion, &mut pending_cancels);
                let _ = core.complete_runner_dispatch(completion);
                let _ = schedule_ready(&mut core, &config, &mut pools);
            }
            CoreActorMsg::Shutdown => break,
        }
    }
}

fn handle_command(
    command: HostRuntimeCommand,
    core: &mut CoreRuntime,
    config: &HostRuntimeConfig,
    pools: &mut HashMap<ExecutionClass, WorkerPool>,
    rx: &mpsc::Receiver<CoreActorMsg>,
    pending_cancels: &mut BTreeMap<String, Vec<String>>,
) -> CoreActorCommandOutcome {
    let mut shutdown = false;
    let reply = (|| -> RuntimeResult<HostRuntimeReply> {
        match command {
            HostRuntimeCommand::SubmitTask(task) => {
                let task_id = core.submit_task(*task);
                Ok(HostRuntimeReply::TaskSubmitted(task_id))
            }
            HostRuntimeCommand::TickOnce => {
                let mut report = schedule_ready(core, config, pools)?;
                shutdown = drain_worker_completions(
                    core,
                    config,
                    pools,
                    rx,
                    pending_cancels,
                    &mut report,
                    1,
                );
                Ok(HostRuntimeReply::Tick(report))
            }
            HostRuntimeCommand::RunUntilIdle { max_ticks } => {
                let mut aggregate = RunnerLoopReport {
                    claimed_tasks: 0,
                    completed_tasks: 0,
                };
                for _ in 0..max_ticks {
                    let report = schedule_ready(core, config, pools)?;
                    aggregate.claimed_tasks += report.claimed_tasks;
                    aggregate.completed_tasks += report.completed_tasks;
                    shutdown = drain_worker_completions(
                        core,
                        config,
                        pools,
                        rx,
                        pending_cancels,
                        &mut aggregate,
                        8,
                    );
                    if core.tasks().ready_count() == 0 && core.tasks().running_count() == 0 {
                        break;
                    }
                    if shutdown {
                        break;
                    }
                }
                Ok(HostRuntimeReply::Idle(aggregate))
            }
            HostRuntimeCommand::CancelTask(task_id) => {
                let running_runner = running_runner_for_task(core, &task_id);
                core.cancel_task(&task_id)?;
                if let Some(runner_id) = running_runner {
                    pending_cancels
                        .entry(runner_id)
                        .or_default()
                        .push(task_id.clone());
                }
                Ok(HostRuntimeReply::TaskCancelled(task_id))
            }
        }
    })();
    CoreActorCommandOutcome { reply, shutdown }
}

fn drain_worker_completions(
    core: &mut CoreRuntime,
    config: &HostRuntimeConfig,
    pools: &mut HashMap<ExecutionClass, WorkerPool>,
    rx: &mpsc::Receiver<CoreActorMsg>,
    pending_cancels: &mut BTreeMap<String, Vec<String>>,
    aggregate: &mut RunnerLoopReport,
    max_messages: usize,
) -> bool {
    for _ in 0..max_messages {
        match rx.recv_timeout(Duration::from_millis(10)) {
            Ok(CoreActorMsg::WorkerCompleted(mut completion)) => {
                apply_pending_cancels(&mut completion, pending_cancels);
                if let Ok(report) = core.complete_runner_dispatch(completion) {
                    aggregate.completed_tasks += report.completed_tasks;
                }
                if let Ok(report) = schedule_ready(core, config, pools) {
                    aggregate.claimed_tasks += report.claimed_tasks;
                    aggregate.completed_tasks += report.completed_tasks;
                }
            }
            Ok(CoreActorMsg::TaskStatus(task_id, reply_tx)) => {
                let _ = reply_tx.send(core.task_status(&task_id));
            }
            Ok(CoreActorMsg::Command(command, reply_tx)) => {
                let outcome = handle_command(command, core, config, pools, rx, pending_cancels);
                let shutdown = outcome.shutdown;
                let _ = reply_tx.send(outcome.reply);
                if shutdown {
                    return true;
                }
            }
            Ok(CoreActorMsg::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => return true,
            Err(mpsc::RecvTimeoutError::Timeout) => return false,
        }
    }
    false
}

fn running_runner_for_task(core: &CoreRuntime, task_id: &str) -> Option<String> {
    let record = core.tasks().get(task_id)?;
    if record.status != TaskStatus::Running {
        return None;
    }
    record.claimed_by.clone()
}

fn apply_pending_cancels(
    completion: &mut RunnerCompletion,
    pending_cancels: &mut BTreeMap<String, Vec<String>>,
) {
    let runner_id = completion.runner.descriptor().runner_id.clone();
    let Some(invocation_ids) = pending_cancels.remove(&runner_id) else {
        return;
    };
    for invocation_id in invocation_ids {
        let _ = completion.runner.cancel(&invocation_id);
    }
}

fn schedule_ready(
    core: &mut CoreRuntime,
    config: &HostRuntimeConfig,
    pools: &mut HashMap<ExecutionClass, WorkerPool>,
) -> RuntimeResult<RunnerLoopReport> {
    let (report, dispatches) = core.claim_ready_dispatches(
        |descriptor, load| dispatch_limit(descriptor, load, config, pools),
        None,
    )?;
    for dispatch in dispatches {
        let execution_class = dispatch.runner.descriptor().execution_class.clone();
        let Some(pool) = pools.get(&execution_class) else {
            return Err(host_failure(
                "host.worker.pool_missing",
                format!("execution_class.{execution_class:?}"),
            ));
        };
        pool.send(dispatch)?;
    }
    Ok(RunnerLoopReport {
        claimed_tasks: report.claimed_tasks,
        completed_tasks: report.completed_tasks,
    })
}

fn dispatch_limit(
    descriptor: &RunnerDescriptor,
    load: &RunnerLoad,
    config: &HostRuntimeConfig,
    pools: &HashMap<ExecutionClass, WorkerPool>,
) -> usize {
    if descriptor.execution_class == ExecutionClass::Control {
        return if descriptor.runner_id == "core.kernel" {
            1
        } else {
            0
        };
    }
    let limits = config
        .runner_limits
        .get(&descriptor.runner_id)
        .unwrap_or(&config.default_runner_limits);
    if load.running_count >= limits.max_running
        || load.waiting_count >= limits.max_waiting
        || load.pending_weight >= limits.max_inflight
        || load.queued_count >= limits.queue_limit
    {
        return 0;
    }
    let pool_slots = pools
        .get(&descriptor.execution_class)
        .map(WorkerPool::available_slots)
        .unwrap_or(0);
    limits
        .max_running
        .saturating_sub(load.running_count)
        .min(limits.max_inflight.saturating_sub(load.pending_weight))
        .min(limits.queue_limit.saturating_sub(load.queued_count))
        .min(pool_slots)
}

fn worker_pools(
    config: &HostRuntimeConfig,
    actor_tx: mpsc::Sender<CoreActorMsg>,
) -> RuntimeResult<HashMap<ExecutionClass, WorkerPool>> {
    let mut pools = HashMap::new();
    for execution_class in [
        ExecutionClass::Orchestration,
        ExecutionClass::Io,
        ExecutionClass::Cpu,
    ] {
        pools.insert(
            execution_class.clone(),
            WorkerPool::new(
                execution_class_name(&execution_class),
                config.worker_threads,
                config.pool_queue_limit,
                actor_tx.clone(),
            )?,
        );
    }
    for execution_class in [ExecutionClass::Blocking, ExecutionClass::Script] {
        pools.insert(
            execution_class.clone(),
            WorkerPool::new(
                execution_class_name(&execution_class),
                config.blocking_threads,
                config.pool_queue_limit,
                actor_tx.clone(),
            )?,
        );
    }
    Ok(pools)
}

fn execution_class_name(execution_class: &ExecutionClass) -> &'static str {
    match execution_class {
        ExecutionClass::Control => "control",
        ExecutionClass::Orchestration => "orchestration",
        ExecutionClass::Io => "io",
        ExecutionClass::Cpu => "cpu",
        ExecutionClass::Blocking => "blocking",
        ExecutionClass::Script => "script",
    }
}

fn host_failure(route: &str, detail: impl Into<String>) -> RuntimeFailure {
    let mut error = RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
        "runtime.host",
        route,
    );
    error.evidence.insert(
        "detail".into(),
        mutsuki_runtime_contracts::ScalarValue::String(detail.into()),
    );
    RuntimeFailure::new(error)
}

pub fn resolve_load_plan(
    manifests: &[PluginManifest],
    profile: &RuntimeProfile,
) -> RuntimeLoadPlan {
    let enabled: Vec<PluginManifest> = manifests
        .iter()
        .filter(|manifest| profile.enabled_plugins.contains(&manifest.plugin_id))
        .cloned()
        .collect();
    let mut runner_bindings = profile.bindings.clone();
    for manifest in &enabled {
        for runner in &manifest.provides.runners {
            for protocol_id in &runner.accepted_protocol_ids {
                runner_bindings
                    .entry(protocol_id.clone())
                    .or_insert_with(|| runner.runner_id.clone());
            }
        }
    }
    RuntimeLoadPlan {
        lock_version: 1,
        core_api_version: "mutsuki-core-v1".into(),
        profile_id: profile.profile_id.clone(),
        profile_hash: format!(
            "profile:{}:{}",
            profile.profile_id,
            profile.enabled_plugins.len()
        ),
        registry_generation: 1,
        plugins: enabled.clone(),
        load_order: profile.enabled_plugins.clone(),
        runner_bindings,
        contract_surfaces: surfaces_for(&enabled),
    }
}

fn surfaces_for(manifests: &[PluginManifest]) -> Vec<ContractSurface> {
    let mut surfaces = Vec::new();
    for manifest in manifests {
        for runner in &manifest.provides.runners {
            surfaces.push(ContractSurface {
                surface_id: format!("runner:{}", runner.runner_id),
                kind: ContractSurfaceKind::Runner,
                owner_plugin_id: manifest.plugin_id.clone(),
                fingerprint: format!("runner:{}:{}", runner.runner_id, runner.plugin_generation),
                deprecated: false,
            });
            for protocol_id in &runner.accepted_protocol_ids {
                surfaces.push(ContractSurface {
                    surface_id: format!("task_protocol:{protocol_id}"),
                    kind: ContractSurfaceKind::TaskProtocol,
                    owner_plugin_id: manifest.plugin_id.clone(),
                    fingerprint: format!("task_protocol:{protocol_id}"),
                    deprecated: false,
                });
            }
        }
        for protocol in &manifest.provides.protocols {
            surfaces.push(ContractSurface {
                surface_id: format!("protocol:{}", protocol.protocol_id),
                kind: ContractSurfaceKind::Protocol,
                owner_plugin_id: manifest.plugin_id.clone(),
                fingerprint: format!("protocol:{}:{}", protocol.protocol_id, protocol.version),
                deprecated: false,
            });
        }
        for binding in &manifest.provides.handler_bindings {
            surfaces.push(ContractSurface {
                surface_id: format!("handler_binding:{}", binding.binding_id),
                kind: ContractSurfaceKind::HandlerBinding,
                owner_plugin_id: manifest.plugin_id.clone(),
                fingerprint: format!(
                    "handler_binding:{}:{}:{}",
                    binding.binding_id, binding.protocol_id, binding.target_protocol_id
                ),
                deprecated: false,
            });
        }
        for (kind, prefix, names) in [
            (
                ContractSurfaceKind::ResourceSchema,
                "resource_schema",
                &manifest.provides.resource_schemas,
            ),
            (
                ContractSurfaceKind::ResourceProvider,
                "resource_provider",
                &manifest.provides.resource_providers,
            ),
            (
                ContractSurfaceKind::Effect,
                "effect",
                &manifest.provides.effects,
            ),
            (
                ContractSurfaceKind::Stream,
                "stream",
                &manifest.provides.streams,
            ),
            (
                ContractSurfaceKind::Subscription,
                "subscription",
                &manifest.provides.subscriptions,
            ),
            (
                ContractSurfaceKind::Timer,
                "timer",
                &manifest.provides.timers,
            ),
            (
                ContractSurfaceKind::StateSchema,
                "state_schema",
                &manifest.provides.state_schemas,
            ),
        ] {
            push_named_surfaces(&mut surfaces, &manifest.plugin_id, kind, prefix, names);
        }
    }
    surfaces
}

fn push_named_surfaces(
    surfaces: &mut Vec<ContractSurface>,
    plugin_id: &str,
    kind: ContractSurfaceKind,
    prefix: &str,
    names: &[String],
) {
    for name in names {
        surfaces.push(ContractSurface {
            surface_id: format!("{prefix}:{name}"),
            kind: kind.clone(),
            owner_plugin_id: plugin_id.into(),
            fingerprint: format!("{prefix}:{name}"),
            deprecated: false,
        });
    }
}

fn core_manifest(runner: RunnerDescriptor) -> PluginManifest {
    PluginManifest {
        plugin_id: "core".into(),
        version: "0.1.0".into(),
        api_version: "mutsuki-plugin-v1".into(),
        artifact: PluginArtifact {
            artifact_type: ArtifactType::Native,
            path: "core".into(),
            sha256: "sha256:core".into(),
        },
        provides: PluginProvides {
            runners: vec![runner],
            ..PluginProvides::default()
        },
        requires: Vec::new(),
        permissions: PermissionGrant {
            effects: Vec::new(),
            resources: Vec::new(),
        },
        lifecycle: LifecyclePolicy {
            reload_policy: "core".into(),
            unload_timeout_ms: 0,
            supports_cancel: true,
            supports_dispose: true,
            supports_snapshot: true,
        },
        metadata: BTreeMap::new(),
    }
}

pub fn runner_manifest(plugin_id: &str, runners: Vec<RunnerDescriptor>) -> PluginManifest {
    PluginManifest {
        plugin_id: plugin_id.into(),
        version: "0.1.0".into(),
        api_version: "mutsuki-plugin-v1".into(),
        artifact: PluginArtifact {
            artifact_type: ArtifactType::Native,
            path: "native".into(),
            sha256: "sha256:native".into(),
        },
        provides: PluginProvides {
            runners,
            ..PluginProvides::default()
        },
        requires: Vec::new(),
        permissions: PermissionGrant {
            effects: Vec::new(),
            resources: Vec::new(),
        },
        lifecycle: LifecyclePolicy {
            reload_policy: "drain_and_swap".into(),
            unload_timeout_ms: 5000,
            supports_cancel: true,
            supports_dispose: true,
            supports_snapshot: false,
        },
        metadata: BTreeMap::new(),
    }
}
