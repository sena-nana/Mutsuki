use std::collections::BTreeMap;
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::FutureExt;
use mutsuki_runtime_contracts::{TaskBatch, TaskHandle, TaskOutcome};
use mutsuki_runtime_core::RuntimeResult;
use mutsuki_runtime_sdk::TaskSubmitter;
use mutsuki_service_config::{SecretStore, ServiceConfig};
use mutsuki_service_control::EventSourceStatus;
use tokio::sync::{mpsc, watch};

pub type HostEventSourceError = Box<dyn std::error::Error + Send + Sync + 'static>;
pub type HostEventSourceFuture =
    Pin<Box<dyn Future<Output = Result<(), HostEventSourceError>> + Send + 'static>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostEventSourceDescriptor {
    pub source_id: String,
    pub plugin_id: String,
    pub instance_id: String,
    /// Secret keys that must resolve through the Host secret backend before
    /// any runtime component is started. Values are never stored here.
    pub required_secrets: Vec<String>,
}

impl HostEventSourceDescriptor {
    pub fn new(source_id: impl Into<String>, plugin_id: impl Into<String>) -> Self {
        let source_id = source_id.into();
        Self {
            instance_id: source_id.clone(),
            source_id,
            plugin_id: plugin_id.into(),
            required_secrets: Vec::new(),
        }
    }

    pub fn with_instance_id(mut self, instance_id: impl Into<String>) -> Self {
        self.instance_id = instance_id.into();
        self
    }

    pub fn require_secret(mut self, key: impl Into<String>) -> Self {
        self.required_secrets.push(key.into());
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostEventSourceHealth {
    Healthy,
    Degraded(String),
    Unhealthy(String),
}

impl HostEventSourceHealth {
    fn label(&self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Degraded(_) => "degraded",
            Self::Unhealthy(_) => "unhealthy",
        }
    }
}

#[derive(Clone)]
pub struct HostEventSourceConfig {
    instance_id: String,
    profile: String,
    home_dir: String,
    data_dir: String,
    secret_env_prefix: String,
    secret_store: SecretStore,
}

impl HostEventSourceConfig {
    pub(crate) fn from_service(config: &ServiceConfig) -> Self {
        Self {
            instance_id: config.service.instance_id.clone(),
            profile: config.service.profile.clone(),
            home_dir: config.service.home_dir.to_string_lossy().into_owned(),
            data_dir: config.service.data_dir.to_string_lossy().into_owned(),
            secret_env_prefix: config.security.secret_env_prefix.clone(),
            secret_store: config.secret_store(),
        }
    }

    pub fn get(&self, scope: &str, key: &str) -> Option<&str> {
        match (scope, key) {
            ("service", "instance_id") => Some(&self.instance_id),
            ("service", "profile") => Some(&self.profile),
            ("service", "home_dir") => Some(&self.home_dir),
            ("service", "data_dir") => Some(&self.data_dir),
            _ => None,
        }
    }

    pub fn secret(&self, key: &str) -> Option<String> {
        let key = key
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() {
                    character.to_ascii_uppercase()
                } else {
                    '_'
                }
            })
            .collect::<String>();
        std::env::var(format!("{}{key}", self.secret_env_prefix))
            .ok()
            .or_else(|| self.secret_store.resolve(&key))
    }

    pub(crate) fn contains_secret(&self, key: &str) -> bool {
        self.secret(key).is_some_and(|value| !value.is_empty())
    }
}

#[derive(Clone)]
pub struct HostEventSourceLogger {
    source_id: String,
    plugin_id: String,
}

impl HostEventSourceLogger {
    pub fn log(&self, level: &str, message: &str, correlation_id: Option<&str>) {
        tracing::event!(
            tracing::Level::INFO,
            source_id = %self.source_id,
            plugin_id = %self.plugin_id,
            correlation_id = correlation_id.unwrap_or(""),
            event_level = level,
            message
        );
    }
}

#[derive(Clone)]
pub struct HostShutdownToken {
    rx: watch::Receiver<bool>,
}

impl HostShutdownToken {
    pub fn is_cancelled(&self) -> bool {
        *self.rx.borrow()
    }

    pub async fn cancelled(&mut self) {
        if !*self.rx.borrow() {
            let _ = self.rx.changed().await;
        }
    }
}

#[derive(Clone)]
pub struct HostEventSourceContext {
    pub task_submitter: Arc<dyn TaskSubmitter>,
    pub shutdown: HostShutdownToken,
    pub config: HostEventSourceConfig,
    pub events: HostEventSourceLogger,
    pub source_instance_id: String,
}

pub trait HostEventSource: Send + 'static {
    fn descriptor(&self) -> &HostEventSourceDescriptor;
    fn start(&mut self, ctx: HostEventSourceContext) -> HostEventSourceFuture;
    fn shutdown(&mut self) -> HostEventSourceFuture;
    fn health(&self) -> HostEventSourceHealth;
}

#[derive(Clone)]
pub(crate) struct EventSourceSupervisor {
    sources: Arc<Mutex<BTreeMap<String, ManagedSource>>>,
    changed: Arc<dyn Fn() + Send + Sync>,
}

impl Default for EventSourceSupervisor {
    fn default() -> Self {
        Self::new(Arc::new(|| {}))
    }
}

impl EventSourceSupervisor {
    pub(crate) fn new(changed: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self {
            sources: Arc::new(Mutex::new(BTreeMap::new())),
            changed,
        }
    }
}

struct ManagedSource {
    status: SourceStatus,
    commands: mpsc::Sender<SourceCommand>,
    task: tokio::task::JoinHandle<SourceExit>,
}

struct SourceExit {
    _source: Box<dyn HostEventSource>,
    cleanup_error: Option<String>,
}

#[derive(Clone)]
struct SourceStatus {
    value: Arc<Mutex<EventSourceStatus>>,
    changed: Arc<dyn Fn() + Send + Sync>,
}

impl SourceStatus {
    fn new(descriptor: &HostEventSourceDescriptor, changed: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self {
            value: Arc::new(Mutex::new(EventSourceStatus {
                source_id: descriptor.source_id.clone(),
                plugin_id: descriptor.plugin_id.clone(),
                instance_id: descriptor.instance_id.clone(),
                state: "starting".into(),
                health: "unknown".into(),
                last_error: None,
                reconnects: 0,
                last_event_unix_ms: None,
                started_at_unix_ms: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .ok()
                    .map(|duration| duration.as_millis()),
            })),
            changed,
        }
    }

    fn snapshot(&self) -> EventSourceStatus {
        self.value
            .lock()
            .expect("event source status mutex")
            .clone()
    }

    fn set(&self, state: &str) {
        let mut status = self.value.lock().expect("event source status mutex");
        if status.state != state {
            status.state = state.into();
            drop(status);
            (self.changed)();
        }
    }

    fn fail(&self, error: String) {
        let mut status = self.value.lock().expect("event source status mutex");
        status.state = "failed".into();
        status.last_error = Some(error);
        drop(status);
        (self.changed)();
    }

    fn update_health(&self, health: HostEventSourceHealth) {
        let mut status = self.value.lock().expect("event source status mutex");
        let before = (status.health.clone(), status.last_error.clone());
        status.health = health.label().into();
        status.last_error = match health {
            HostEventSourceHealth::Degraded(error) | HostEventSourceHealth::Unhealthy(error) => {
                Some(error)
            }
            HostEventSourceHealth::Healthy => None,
        };
        if before != (status.health.clone(), status.last_error.clone()) {
            drop(status);
            (self.changed)();
        }
    }

    fn reconnect(&self) {
        let mut status = self.value.lock().expect("event source status mutex");
        status.reconnects = status.reconnects.saturating_add(1);
        status.state = "restarting".into();
        drop(status);
        (self.changed)();
    }

    fn submitted(&self, correlation_ids: &[Option<String>]) {
        let mut status = self.value.lock().expect("event source status mutex");
        status.last_event_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_millis());
        for correlation_id in correlation_ids {
            tracing::info!(
                source_id = %status.source_id,
                plugin_id = %status.plugin_id,
                correlation_id = correlation_id.as_deref().unwrap_or(""),
                "event source submitted task"
            );
        }
        drop(status);
        (self.changed)();
    }
}

enum SourceCommand {
    Restart,
    Shutdown,
}

impl EventSourceSupervisor {
    pub(crate) fn start(
        &self,
        source: Box<dyn HostEventSource>,
        task_submitter: Arc<dyn TaskSubmitter>,
        config: &ServiceConfig,
        graceful: Duration,
    ) {
        let descriptor = source.descriptor().clone();
        let mut sources = self.sources.lock().expect("event source supervisor mutex");
        let status = SourceStatus::new(&descriptor, self.changed.clone());
        let (tx, rx) = mpsc::channel(4);
        let source_config = HostEventSourceConfig::from_service(config);
        let logger = HostEventSourceLogger {
            source_id: descriptor.source_id.clone(),
            plugin_id: descriptor.plugin_id.clone(),
        };
        let task = tokio::spawn(run_source_actor(
            source,
            task_submitter,
            source_config,
            logger,
            status.clone(),
            rx,
            graceful,
        ));
        sources.insert(
            descriptor.source_id,
            ManagedSource {
                status,
                commands: tx,
                task,
            },
        );
    }

    pub(crate) fn list(&self) -> Vec<EventSourceStatus> {
        self.sources
            .lock()
            .expect("event source supervisor mutex")
            .values()
            .map(|source| source.status.snapshot())
            .collect()
    }

    pub(crate) async fn restart(&self, source_id: &str) -> Result<(), String> {
        let sender = self
            .sources
            .lock()
            .expect("event source supervisor mutex")
            .get(source_id)
            .map(|source| source.commands.clone())
            .ok_or_else(|| format!("unknown event source {source_id}"))?;
        sender
            .send(SourceCommand::Restart)
            .await
            .map_err(|_| format!("event source {source_id} lifecycle task has stopped"))
    }

    pub(crate) async fn shutdown(&self, graceful: Duration) {
        let managed = self.take_sources();
        for source in &managed {
            let _ = source.commands.send(SourceCommand::Shutdown).await;
        }
        for source in managed {
            let mut task = source.task;
            if tokio::time::timeout(graceful + Duration::from_millis(100), &mut task)
                .await
                .is_err()
            {
                task.abort();
            }
        }
    }

    pub(crate) async fn shutdown_source(
        &self,
        source_id: &str,
        graceful: Duration,
    ) -> Result<(), String> {
        let managed = self
            .sources
            .lock()
            .expect("event source supervisor mutex")
            .remove(source_id);
        let Some(source) = managed else {
            return Ok(());
        };
        let _ = source.commands.send(SourceCommand::Shutdown).await;
        let mut task = source.task;
        match tokio::time::timeout(graceful + Duration::from_millis(100), &mut task).await {
            Ok(Ok(SourceExit {
                cleanup_error: None,
                ..
            })) => Ok(()),
            Ok(Ok(SourceExit {
                cleanup_error: Some(error),
                ..
            })) => Err(error),
            Ok(Err(error)) => Err(format!("event source lifecycle task failed: {error}")),
            Err(_) => {
                task.abort();
                Err(format!("event source {source_id} did not stop in time"))
            }
        }
    }

    pub(crate) fn abort(&self) {
        for source in self.take_sources() {
            let _ = source.commands.try_send(SourceCommand::Shutdown);
            source.task.abort();
        }
    }

    fn take_sources(&self) -> Vec<ManagedSource> {
        let mut sources = self.sources.lock().expect("event source supervisor mutex");
        std::mem::take(&mut *sources).into_values().collect()
    }
}

async fn run_source_actor(
    mut source: Box<dyn HostEventSource>,
    task_submitter: Arc<dyn TaskSubmitter>,
    config: HostEventSourceConfig,
    events: HostEventSourceLogger,
    status: SourceStatus,
    mut commands: mpsc::Receiver<SourceCommand>,
    graceful: Duration,
) -> SourceExit {
    loop {
        status.set("starting");
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let tracked_submitter: Arc<dyn TaskSubmitter> = Arc::new(SourceTaskSubmitter {
            inner: task_submitter.clone(),
            status: status.clone(),
        });
        let ctx = HostEventSourceContext {
            task_submitter: tracked_submitter,
            shutdown: HostShutdownToken { rx: shutdown_rx },
            config: config.clone(),
            events: events.clone(),
            source_instance_id: status.snapshot().instance_id,
        };
        let start = catch_unwind(AssertUnwindSafe(|| source.start(ctx)));
        let mut running = match start {
            Ok(future) => future,
            Err(payload) => {
                status.fail(panic_message(payload));
                let (restart, cleanup_error) =
                    wait_command(&mut commands, &mut source, &status, &shutdown_tx, graceful).await;
                if !restart {
                    return SourceExit {
                        _source: source,
                        cleanup_error,
                    };
                }
                continue;
            }
        };
        status.update_health(safe_health(source.as_ref()));
        status.set("running");
        let mut health_tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            tokio::select! {
                outcome = AssertUnwindSafe(&mut running).catch_unwind() => {
                    let message = match outcome {
                        Ok(Ok(())) => "event source exited unexpectedly".to_string(),
                        Ok(Err(error)) => error.to_string(),
                        Err(payload) => panic_message(payload),
                    };
                    status.fail(message);
                    break;
                }
                command = commands.recv() => {
                    let (restart, cleanup_error) = handle_running_command(
                        command,
                        &mut source,
                        &mut running,
                        &status,
                        &shutdown_tx,
                        graceful,
                    ).await;
                    if restart {
                        break;
                    } else {
                        return SourceExit {
                            _source: source,
                            cleanup_error,
                        };
                    }
                }
                _ = health_tick.tick() => status.update_health(safe_health(source.as_ref())),
            }
        }
        if status.snapshot().state == "failed" {
            let (restart, cleanup_error) =
                wait_command(&mut commands, &mut source, &status, &shutdown_tx, graceful).await;
            if !restart {
                return SourceExit {
                    _source: source,
                    cleanup_error,
                };
            }
        }
    }
}

async fn wait_command(
    commands: &mut mpsc::Receiver<SourceCommand>,
    source: &mut Box<dyn HostEventSource>,
    status: &SourceStatus,
    shutdown: &watch::Sender<bool>,
    graceful: Duration,
) -> (bool, Option<String>) {
    handle_command(commands.recv().await, source, status, shutdown, graceful).await
}

async fn handle_command(
    command: Option<SourceCommand>,
    source: &mut Box<dyn HostEventSource>,
    status: &SourceStatus,
    shutdown: &watch::Sender<bool>,
    graceful: Duration,
) -> (bool, Option<String>) {
    match command {
        Some(SourceCommand::Restart) => {
            status.reconnect();
            let _ = shutdown.send(true);
            let error = stop_source(source, status, graceful, false).await;
            (true, error)
        }
        Some(SourceCommand::Shutdown) | None => {
            status.set("stopping");
            let _ = shutdown.send(true);
            let error = stop_source(source, status, graceful, true).await;
            (false, error)
        }
    }
}

async fn handle_running_command(
    command: Option<SourceCommand>,
    source: &mut Box<dyn HostEventSource>,
    running: &mut HostEventSourceFuture,
    status: &SourceStatus,
    shutdown: &watch::Sender<bool>,
    graceful: Duration,
) -> (bool, Option<String>) {
    let (restart, terminal) = match command {
        Some(SourceCommand::Restart) => {
            status.reconnect();
            (true, false)
        }
        Some(SourceCommand::Shutdown) | None => {
            status.set("stopping");
            (false, true)
        }
    };
    let _ = shutdown.send(true);
    let error = stop_running_source(source, running, status, graceful, terminal).await;
    (restart, error)
}

async fn stop_source(
    source: &mut Box<dyn HostEventSource>,
    status: &SourceStatus,
    graceful: Duration,
    terminal: bool,
) -> Option<String> {
    let future = match catch_unwind(AssertUnwindSafe(|| source.shutdown())) {
        Ok(future) => future,
        Err(payload) => {
            let error = panic_message(payload);
            status.fail(error.clone());
            return Some(error);
        }
    };
    match tokio::time::timeout(graceful, AssertUnwindSafe(future).catch_unwind()).await {
        Ok(Ok(Ok(()))) => {
            if terminal {
                status.set("stopped");
            }
            None
        }
        Ok(Ok(Err(error))) => {
            let error = error.to_string();
            status.fail(error.clone());
            Some(error)
        }
        Ok(Err(payload)) => {
            let error = panic_message(payload);
            status.fail(error.clone());
            Some(error)
        }
        Err(_) => {
            let error = format!("shutdown timed out after {} ms", graceful.as_millis());
            status.fail(error.clone());
            Some(error)
        }
    }
}

async fn stop_running_source(
    source: &mut Box<dyn HostEventSource>,
    running: &mut HostEventSourceFuture,
    status: &SourceStatus,
    graceful: Duration,
    terminal: bool,
) -> Option<String> {
    let shutdown = match catch_unwind(AssertUnwindSafe(|| source.shutdown())) {
        Ok(future) => future,
        Err(payload) => {
            let error = panic_message(payload);
            status.fail(error.clone());
            return Some(error);
        }
    };
    let completion = async {
        tokio::join!(
            AssertUnwindSafe(running).catch_unwind(),
            AssertUnwindSafe(shutdown).catch_unwind(),
        )
    };
    let error = match tokio::time::timeout(graceful, completion).await {
        Ok((Ok(Ok(())), Ok(Ok(())))) => None,
        Ok((Ok(Err(error)), _)) => Some(error.to_string()),
        Ok((Err(payload), _)) => Some(panic_message(payload)),
        Ok((_, Ok(Err(error)))) => Some(error.to_string()),
        Ok((_, Err(payload))) => Some(panic_message(payload)),
        Err(_) => Some(format!(
            "shutdown timed out after {} ms",
            graceful.as_millis()
        )),
    };
    if let Some(error) = &error {
        status.fail(error.clone());
    } else if terminal {
        status.set("stopped");
    }
    error
}

fn safe_health(source: &dyn HostEventSource) -> HostEventSourceHealth {
    catch_unwind(AssertUnwindSafe(|| source.health()))
        .unwrap_or_else(|payload| HostEventSourceHealth::Unhealthy(panic_message(payload)))
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        format!("event source panicked: {message}")
    } else if let Some(message) = payload.downcast_ref::<String>() {
        format!("event source panicked: {message}")
    } else {
        "event source panicked with a non-string payload".into()
    }
}

struct SourceTaskSubmitter {
    inner: Arc<dyn TaskSubmitter>,
    status: SourceStatus,
}

impl TaskSubmitter for SourceTaskSubmitter {
    fn submit_batch(&self, batch: TaskBatch) -> RuntimeResult<Vec<TaskHandle>> {
        let correlation_ids = batch
            .tasks
            .iter()
            .map(|task| task.correlation_id.clone())
            .collect::<Vec<_>>();
        let result = self.inner.submit_batch(batch);
        if result.is_ok() {
            self.status.submitted(&correlation_ids);
        }
        result
    }

    fn cancel_task(&self, handle: &TaskHandle) -> RuntimeResult<()> {
        self.inner.cancel_task(handle)
    }

    fn task_outcome(&self, handle: &TaskHandle) -> RuntimeResult<Option<TaskOutcome>> {
        self.inner.task_outcome(handle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn source_panic_is_isolated_and_explicit_restart_recovers() {
        let starts = Arc::new(AtomicUsize::new(0));
        let supervisor = EventSourceSupervisor::default();
        supervisor.start(
            Box::new(PanicOnceSource {
                descriptor: HostEventSourceDescriptor::new("panic-once", "test.plugin"),
                starts: starts.clone(),
            }),
            Arc::new(NoopSubmitter),
            &test_config(),
            Duration::from_millis(50),
        );

        wait_for_state(&supervisor, "failed").await;
        assert!(
            supervisor.list()[0]
                .last_error
                .as_deref()
                .expect("panic error")
                .contains("panicked")
        );
        supervisor.restart("panic-once").await.expect("restart");
        wait_for_state(&supervisor, "running").await;
        assert_eq!(supervisor.list()[0].reconnects, 1);
        assert_eq!(starts.load(Ordering::SeqCst), 2);

        supervisor.shutdown(Duration::from_millis(50)).await;
    }

    #[tokio::test]
    async fn shutdown_timeout_becomes_structured_failure() {
        let supervisor = EventSourceSupervisor::default();
        supervisor.start(
            Box::new(HangingShutdownSource {
                descriptor: HostEventSourceDescriptor::new("hang", "test.plugin"),
            }),
            Arc::new(NoopSubmitter),
            &test_config(),
            Duration::from_millis(20),
        );
        wait_for_state(&supervisor, "running").await;
        let sender = supervisor
            .sources
            .lock()
            .expect("sources")
            .get("hang")
            .expect("source")
            .commands
            .clone();
        sender
            .send(SourceCommand::Shutdown)
            .await
            .expect("shutdown");
        wait_for_state(&supervisor, "failed").await;
        assert!(
            supervisor.list()[0]
                .last_error
                .as_deref()
                .expect("timeout error")
                .contains("timed out")
        );
        supervisor.shutdown(Duration::from_millis(20)).await;
    }

    #[tokio::test]
    async fn explicit_restart_exposes_restarting_until_graceful_stop_finishes() {
        let supervisor = EventSourceSupervisor::default();
        supervisor.start(
            Box::new(HangingShutdownSource {
                descriptor: HostEventSourceDescriptor::new("restart-visible", "test.plugin"),
            }),
            Arc::new(NoopSubmitter),
            &test_config(),
            Duration::from_millis(100),
        );
        wait_for_state(&supervisor, "running").await;

        supervisor
            .restart("restart-visible")
            .await
            .expect("restart command");
        wait_for_state(&supervisor, "restarting").await;
        assert_eq!(supervisor.list()[0].state, "restarting");
        assert_eq!(supervisor.list()[0].reconnects, 1);

        wait_for_state(&supervisor, "failed").await;
        supervisor.shutdown(Duration::from_millis(100)).await;
    }

    #[test]
    fn recovered_health_clears_the_previous_degraded_error() {
        let status = SourceStatus::new(
            &HostEventSourceDescriptor::new("recover", "test.plugin"),
            Arc::new(|| {}),
        );
        status.update_health(HostEventSourceHealth::Degraded(
            "handshake incomplete".into(),
        ));
        assert_eq!(
            status.snapshot().last_error.as_deref(),
            Some("handshake incomplete")
        );

        status.update_health(HostEventSourceHealth::Healthy);
        let snapshot = status.snapshot();
        assert_eq!(snapshot.health, "healthy");
        assert_eq!(snapshot.last_error, None);
    }

    #[tokio::test]
    async fn shutdown_concurrently_drives_the_running_source_and_its_cleanup() {
        let supervisor = EventSourceSupervisor::default();
        supervisor.start(
            Box::new(CoordinatedShutdownSource::new()),
            Arc::new(NoopSubmitter),
            &test_config(),
            Duration::from_millis(100),
        );
        wait_for_state(&supervisor, "running").await;

        supervisor
            .shutdown_source("coordinated", Duration::from_millis(100))
            .await
            .expect("running source and shutdown future complete together");
    }

    async fn wait_for_state(supervisor: &EventSourceSupervisor, expected: &str) {
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if supervisor
                    .list()
                    .first()
                    .is_some_and(|source| source.state == expected)
                {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("source reached expected state");
    }

    fn test_config() -> ServiceConfig {
        let mut config = ServiceConfig::default();
        config.ipc.token = Some("test".into());
        config
    }

    struct NoopSubmitter;

    impl TaskSubmitter for NoopSubmitter {
        fn submit_batch(&self, _batch: TaskBatch) -> RuntimeResult<Vec<TaskHandle>> {
            Ok(Vec::new())
        }

        fn cancel_task(&self, _handle: &TaskHandle) -> RuntimeResult<()> {
            Ok(())
        }

        fn task_outcome(&self, _handle: &TaskHandle) -> RuntimeResult<Option<TaskOutcome>> {
            Ok(None)
        }
    }

    struct PanicOnceSource {
        descriptor: HostEventSourceDescriptor,
        starts: Arc<AtomicUsize>,
    }

    impl HostEventSource for PanicOnceSource {
        fn descriptor(&self) -> &HostEventSourceDescriptor {
            &self.descriptor
        }

        fn start(&mut self, mut ctx: HostEventSourceContext) -> HostEventSourceFuture {
            let start = self.starts.fetch_add(1, Ordering::SeqCst);
            if start == 0 {
                Box::pin(async {
                    panic!("event source panic");
                    #[allow(unreachable_code)]
                    Ok(())
                })
            } else {
                Box::pin(async move {
                    ctx.shutdown.cancelled().await;
                    Ok(())
                })
            }
        }

        fn shutdown(&mut self) -> HostEventSourceFuture {
            Box::pin(async { Ok(()) })
        }

        fn health(&self) -> HostEventSourceHealth {
            HostEventSourceHealth::Healthy
        }
    }

    struct HangingShutdownSource {
        descriptor: HostEventSourceDescriptor,
    }

    impl HostEventSource for HangingShutdownSource {
        fn descriptor(&self) -> &HostEventSourceDescriptor {
            &self.descriptor
        }

        fn start(&mut self, mut ctx: HostEventSourceContext) -> HostEventSourceFuture {
            Box::pin(async move {
                ctx.shutdown.cancelled().await;
                Ok(())
            })
        }

        fn shutdown(&mut self) -> HostEventSourceFuture {
            Box::pin(std::future::pending())
        }

        fn health(&self) -> HostEventSourceHealth {
            HostEventSourceHealth::Healthy
        }
    }

    struct CoordinatedShutdownSource {
        descriptor: HostEventSourceDescriptor,
        stop: Arc<Mutex<Option<watch::Sender<bool>>>>,
        stopped: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
    }

    impl CoordinatedShutdownSource {
        fn new() -> Self {
            Self {
                descriptor: HostEventSourceDescriptor::new("coordinated", "test.plugin"),
                stop: Arc::new(Mutex::new(None)),
                stopped: Arc::new(Mutex::new(None)),
            }
        }
    }

    impl HostEventSource for CoordinatedShutdownSource {
        fn descriptor(&self) -> &HostEventSourceDescriptor {
            &self.descriptor
        }

        fn start(&mut self, _ctx: HostEventSourceContext) -> HostEventSourceFuture {
            let (stop_tx, mut stop_rx) = watch::channel(false);
            *self.stop.lock().expect("coordinated stop mutex") = Some(stop_tx);
            let (stopped_tx, stopped_rx) = oneshot::channel();
            *self.stopped.lock().expect("coordinated stopped mutex") = Some(stopped_rx);
            Box::pin(async move {
                if !*stop_rx.borrow() {
                    let _ = stop_rx.changed().await;
                }
                let _ = stopped_tx.send(());
                Ok(())
            })
        }

        fn shutdown(&mut self) -> HostEventSourceFuture {
            let stop = self.stop.lock().expect("coordinated stop mutex").take();
            let stopped = self
                .stopped
                .lock()
                .expect("coordinated stopped mutex")
                .take();
            Box::pin(async move {
                if let Some(stop) = stop {
                    let _ = stop.send(true);
                }
                if let Some(stopped) = stopped {
                    let _ = stopped.await;
                }
                Ok(())
            })
        }

        fn health(&self) -> HostEventSourceHealth {
            HostEventSourceHealth::Healthy
        }
    }
}
