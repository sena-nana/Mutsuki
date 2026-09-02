// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::needless_continue,
    clippy::similar_names
)]

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Instant;

use mutsuki_service_config::{filtered_environment, generate_runner_session_token};
use mutsuki_service_plugin_loader::ExternalRuntimeSpec;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::time::{Duration, timeout};

#[derive(Debug, thiserror::Error)]
pub enum RunnerSupervisorError {
    #[error("runner {0} is already running")]
    AlreadyRunning(String),
    #[error("runner {0} is not known")]
    UnknownRunner(String),
    #[error("failed to spawn runner {runner_id}: {source}")]
    Spawn {
        runner_id: String,
        source: std::io::Error,
    },
    #[error("failed to stop runner {runner_id}: {source}")]
    Stop {
        runner_id: String,
        source: std::io::Error,
    },
}

pub type RunnerSupervisorResult<T> = Result<T, RunnerSupervisorError>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagedRunnerSpec {
    pub runner_id: String,
    pub plugin_id: String,
    pub runtime: ExternalRuntimeSpec,
    pub env_allowlist: Vec<String>,
    pub service_home: PathBuf,
    pub restart: RestartPolicy,
}

/// Governs how the supervisor reacts to a Runner process that exits on its own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RestartPolicy {
    pub enabled: bool,
    /// Restarts allowed inside any trailing 60s window. Zero disables restarts entirely, which
    /// keeps `enabled = true, max_per_minute = 0` from meaning "restart without limit".
    pub max_per_minute: u32,
}

impl RestartPolicy {
    #[must_use]
    pub fn from_config(section: &mutsuki_service_config::RunnersSection) -> Self {
        Self {
            enabled: section.restart,
            max_per_minute: section.max_restart_per_minute,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunnerSnapshot {
    pub runner_id: String,
    pub plugin_id: String,
    pub state: RunnerProcessState,
    pub pid: Option<u32>,
    pub restarts: u32,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunnerProcessState {
    Running,
    Exited(i32),
    Failed,
    Stopped,
}

#[derive(Clone)]
pub struct RunnerSupervisor {
    inner: Arc<Mutex<SupervisorState>>,
    changed: Arc<dyn Fn() + Send + Sync>,
}

impl Default for RunnerSupervisor {
    fn default() -> Self {
        Self::with_change_listener(Arc::new(|| {}))
    }
}

#[derive(Default)]
struct SupervisorState {
    runners: BTreeMap<String, ManagedRunner>,
}

struct ManagedRunner {
    spec: ManagedRunnerSpec,
    snapshot: Arc<Mutex<RunnerSnapshot>>,
    commands: mpsc::Sender<RunnerCommand>,
    task: tokio::task::JoinHandle<()>,
}

enum RunnerCommand {
    Restart(oneshot::Sender<RunnerSupervisorResult<()>>),
    Stop(oneshot::Sender<RunnerSupervisorResult<()>>),
    Shutdown {
        graceful: Duration,
        reply: oneshot::Sender<RunnerSupervisorResult<()>>,
    },
}

impl RunnerSupervisor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_change_listener(changed: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(SupervisorState::default())),
            changed,
        }
    }

    pub async fn start(&self, spec: ManagedRunnerSpec) -> RunnerSupervisorResult<()> {
        let mut state = self.inner.lock().await;
        if state.runners.contains_key(&spec.runner_id) {
            return Err(RunnerSupervisorError::AlreadyRunning(spec.runner_id));
        }
        let runner_id = spec.runner_id.clone();
        let child = spawn_child(&spec)?;
        let snapshot = Arc::new(Mutex::new(RunnerSnapshot {
            runner_id: runner_id.clone(),
            plugin_id: spec.plugin_id.clone(),
            state: RunnerProcessState::Running,
            pid: child.id(),
            restarts: 0,
            last_error: None,
        }));
        let (commands, receiver) = mpsc::channel(8);
        let task = tokio::spawn(run_runner_actor(
            spec.clone(),
            child,
            snapshot.clone(),
            receiver,
            self.changed.clone(),
        ));
        state.runners.insert(
            runner_id,
            ManagedRunner {
                spec,
                snapshot,
                commands,
                task,
            },
        );
        drop(state);
        (self.changed)();
        Ok(())
    }

    pub async fn list(&self) -> Vec<RunnerSnapshot> {
        let snapshots = self
            .inner
            .lock()
            .await
            .runners
            .values()
            .map(|runner| runner.snapshot.clone())
            .collect::<Vec<_>>();
        let mut result = Vec::with_capacity(snapshots.len());
        for snapshot in snapshots {
            result.push(snapshot.lock().await.clone());
        }
        result
    }

    pub async fn restart(&self, runner_id: &str) -> RunnerSupervisorResult<()> {
        let commands = self
            .inner
            .lock()
            .await
            .runners
            .get(runner_id)
            .map(|runner| runner.commands.clone());
        let Some(commands) = commands else {
            return Err(RunnerSupervisorError::UnknownRunner(runner_id.into()));
        };
        request_runner(&commands, RunnerCommand::Restart).await
    }

    pub async fn stop(&self, runner_id: &str) -> RunnerSupervisorResult<()> {
        let commands = self
            .inner
            .lock()
            .await
            .runners
            .get(runner_id)
            .map(|runner| runner.commands.clone());
        let Some(commands) = commands else {
            return Err(RunnerSupervisorError::UnknownRunner(runner_id.into()));
        };
        request_runner(&commands, RunnerCommand::Stop).await
    }

    pub async fn remove(&self, runner_id: &str, graceful: Duration) -> RunnerSupervisorResult<()> {
        let runner = self.inner.lock().await.runners.remove(runner_id);
        let Some(runner) = runner else {
            return Ok(());
        };
        let (reply, result) = oneshot::channel();
        runner
            .commands
            .send(RunnerCommand::Shutdown { graceful, reply })
            .await
            .map_err(|_| RunnerSupervisorError::UnknownRunner(runner_id.into()))?;
        let outcome = result
            .await
            .unwrap_or_else(|_| Err(RunnerSupervisorError::UnknownRunner(runner_id.into())));
        let _ = runner.task.await;
        (self.changed)();
        outcome
    }

    pub async fn reconcile(
        &self,
        desired: Vec<ManagedRunnerSpec>,
        graceful: Duration,
    ) -> Vec<RunnerSupervisorError> {
        let desired = desired
            .into_iter()
            .map(|spec| (spec.runner_id.clone(), spec))
            .collect::<BTreeMap<_, _>>();
        let mut errors = Vec::new();

        let existing = self
            .inner
            .lock()
            .await
            .runners
            .iter()
            .map(|(id, runner)| (id.clone(), runner.spec.clone(), runner.snapshot.clone()))
            .collect::<Vec<_>>();
        let existing_ids = existing
            .iter()
            .map(|(id, _, _)| id.clone())
            .collect::<Vec<_>>();
        for runner_id in existing_ids {
            let should_remove = !desired.contains_key(&runner_id);
            let existing_spec = existing
                .iter()
                .find(|(id, _, _)| id == &runner_id)
                .map(|(_, spec, _)| spec);
            let should_replace = desired
                .get(&runner_id)
                .is_some_and(|spec| existing_spec != Some(spec));
            if !should_remove && !should_replace {
                continue;
            }
            if let Err(error) = self.remove(&runner_id, graceful).await {
                errors.push(error);
                continue;
            }
        }

        for (runner_id, spec) in desired {
            let snapshot = self
                .inner
                .lock()
                .await
                .runners
                .get(&runner_id)
                .map(|runner| runner.snapshot.clone());
            let needs_start = match snapshot {
                Some(snapshot) => {
                    !matches!(snapshot.lock().await.state, RunnerProcessState::Running)
                }
                None => true,
            };
            if !needs_start {
                continue;
            }
            if self.inner.lock().await.runners.contains_key(&runner_id) {
                if let Err(error) = self.restart(&runner_id).await {
                    errors.push(error);
                }
            } else if let Err(error) = self.start(spec).await {
                errors.push(error);
            }
        }

        errors
    }

    pub async fn shutdown(&self, graceful: Duration) {
        let runner_ids = self
            .inner
            .lock()
            .await
            .runners
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for runner_id in runner_ids {
            let _ = self.remove(&runner_id, graceful).await;
        }
    }
}

async fn request_runner(
    commands: &mpsc::Sender<RunnerCommand>,
    command: impl FnOnce(oneshot::Sender<RunnerSupervisorResult<()>>) -> RunnerCommand,
) -> RunnerSupervisorResult<()> {
    let (reply, result) = oneshot::channel();
    commands
        .send(command(reply))
        .await
        .map_err(|_| RunnerSupervisorError::UnknownRunner("stopped".into()))?;
    result
        .await
        .unwrap_or_else(|_| Err(RunnerSupervisorError::UnknownRunner("stopped".into())))
}

async fn run_runner_actor(
    spec: ManagedRunnerSpec,
    initial_child: Child,
    snapshot: Arc<Mutex<RunnerSnapshot>>,
    mut commands: mpsc::Receiver<RunnerCommand>,
    changed: Arc<dyn Fn() + Send + Sync>,
) {
    let mut child = Some(initial_child);
    let mut restarts = RestartBudget::new(spec.restart);
    let mut spawned_at = Instant::now();
    loop {
        let mut pending_command = None;
        if let Some(active) = child.as_mut() {
            tokio::select! {
                status = active.wait() => {
                    let mut state = snapshot.lock().await;
                    state.pid = None;
                    match status {
                        Ok(status) => {
                            state.state = RunnerProcessState::Exited(status.code().unwrap_or(-1));
                            state.last_error = None;
                        }
                        Err(error) => {
                            state.state = RunnerProcessState::Failed;
                            state.last_error = Some(error.to_string());
                        }
                    }
                    child = None;
                    drop(state);
                    changed();
                    // The process left on its own. An operator-requested stop takes the command
                    // path instead, and must not be undone by the restart policy.
                    if spawned_at.elapsed() >= RestartBudget::MAX_BACKOFF {
                        restarts.reset_backoff();
                    }
                    match supervise_unexpected_exit(
                        &spec,
                        &snapshot,
                        &mut child,
                        &changed,
                        &mut restarts,
                        &mut commands,
                    )
                    .await
                    {
                        ExitOutcome::Restarted => spawned_at = Instant::now(),
                        ExitOutcome::StayDown => {}
                        ExitOutcome::Interrupted(command) => pending_command = Some(command),
                    }
                }
                command = commands.recv() => {
                    pending_command = Some(command);
                }
            }
        } else {
            pending_command = Some(commands.recv().await);
        }

        if let Some(command) = pending_command {
            if handle_runner_command(command, &spec, &snapshot, &mut child, &changed).await {
                break;
            }
            restarts.reset_backoff();
            spawned_at = Instant::now();
        }
    }
}

enum ExitOutcome {
    Restarted,
    StayDown,
    /// An operator command arrived during the restart backoff. The caller owns it, because
    /// dropping it here would strand the requester's reply channel.
    Interrupted(Option<RunnerCommand>),
}

/// Restart accounting for one Runner.
///
/// The budget is a sliding 60s window rather than a total, so a Runner that crashes rarely keeps
/// recovering forever while one that crash-loops is parked in `Failed` for an operator to look
/// at. Backoff is separate from the budget: it stops a fast crash loop from burning the whole
/// window inside a few milliseconds.
struct RestartBudget {
    policy: RestartPolicy,
    recent: VecDeque<Instant>,
    consecutive: u32,
}

impl RestartBudget {
    const WINDOW: Duration = Duration::from_mins(1);
    const BASE_BACKOFF: Duration = Duration::from_millis(100);
    const MAX_BACKOFF: Duration = Duration::from_secs(10);

    fn new(policy: RestartPolicy) -> Self {
        Self {
            policy,
            recent: VecDeque::new(),
            consecutive: 0,
        }
    }

    /// Returns the delay to wait before respawning, or `None` when the policy or the budget says
    /// this Runner should stay down.
    fn next_delay(&mut self) -> Option<Duration> {
        if !self.policy.enabled || self.policy.max_per_minute == 0 {
            return None;
        }
        let now = Instant::now();
        while self
            .recent
            .front()
            .is_some_and(|at| now.duration_since(*at) >= Self::WINDOW)
        {
            self.recent.pop_front();
        }
        if self.recent.len() >= self.policy.max_per_minute as usize {
            return None;
        }
        self.recent.push_back(now);
        let delay = Self::BASE_BACKOFF
            .saturating_mul(1u32 << self.consecutive.min(7))
            .min(Self::MAX_BACKOFF);
        self.consecutive = self.consecutive.saturating_add(1);
        Some(delay)
    }

    /// Drops the backoff streak. Both callers mean the same thing: the next failure is a new
    /// incident, not the continuation of a crash loop — either because an operator replaced the
    /// process, or because it stayed up past the backoff ceiling before dying.
    fn reset_backoff(&mut self) {
        self.consecutive = 0;
    }
}

/// Waits out the backoff and respawns, unless the policy or budget forbids it, or an operator
/// command arrives first.
async fn supervise_unexpected_exit(
    spec: &ManagedRunnerSpec,
    snapshot: &Arc<Mutex<RunnerSnapshot>>,
    child: &mut Option<Child>,
    changed: &Arc<dyn Fn() + Send + Sync>,
    restarts: &mut RestartBudget,
    commands: &mut mpsc::Receiver<RunnerCommand>,
) -> ExitOutcome {
    let Some(delay) = restarts.next_delay() else {
        if restarts.policy.enabled {
            tracing::warn!(
                runner_id = %spec.runner_id,
                max_per_minute = restarts.policy.max_per_minute,
                "runner exceeded its restart budget and will stay down"
            );
            let mut state = snapshot.lock().await;
            state.state = RunnerProcessState::Failed;
            state.last_error = Some(format!(
                "restart budget exhausted: more than {} restarts in 60s",
                restarts.policy.max_per_minute
            ));
            drop(state);
            changed();
        }
        return ExitOutcome::StayDown;
    };

    tracing::info!(
        runner_id = %spec.runner_id,
        delay_ms = delay.as_millis(),
        "restarting runner after unexpected exit"
    );
    // A stop or shutdown that arrives during the backoff wins: the caller asked for this Runner
    // to be down, so respawning it would fight the operator.
    if let Ok(command) = tokio::time::timeout(delay, commands.recv()).await {
        return ExitOutcome::Interrupted(command);
    }

    match spawn_child(spec) {
        Ok(next) => {
            let mut state = snapshot.lock().await;
            state.state = RunnerProcessState::Running;
            state.pid = next.id();
            state.restarts = state.restarts.saturating_add(1);
            state.last_error = None;
            *child = Some(next);
            drop(state);
            changed();
            ExitOutcome::Restarted
        }
        Err(error) => {
            let mut state = snapshot.lock().await;
            state.state = RunnerProcessState::Failed;
            state.pid = None;
            state.last_error = Some(error.to_string());
            drop(state);
            changed();
            ExitOutcome::StayDown
        }
    }
}

async fn handle_runner_command(
    command: Option<RunnerCommand>,
    spec: &ManagedRunnerSpec,
    snapshot: &Arc<Mutex<RunnerSnapshot>>,
    child: &mut Option<Child>,
    changed: &Arc<dyn Fn() + Send + Sync>,
) -> bool {
    match command {
        Some(RunnerCommand::Restart(reply)) => {
            let result = stop_process(child, &spec.runner_id, Duration::from_secs(5))
                .await
                .and_then(|()| spawn_child(spec));
            match result {
                Ok(next) => {
                    let mut state = snapshot.lock().await;
                    state.state = RunnerProcessState::Running;
                    state.pid = next.id();
                    state.restarts = state.restarts.saturating_add(1);
                    state.last_error = None;
                    *child = Some(next);
                    drop(state);
                    changed();
                    let _ = reply.send(Ok(()));
                }
                Err(error) => {
                    let mut state = snapshot.lock().await;
                    state.state = RunnerProcessState::Failed;
                    state.pid = None;
                    state.last_error = Some(error.to_string());
                    drop(state);
                    changed();
                    let _ = reply.send(Err(error));
                }
            }
            false
        }
        Some(RunnerCommand::Stop(reply)) => {
            let result = stop_process(child, &spec.runner_id, Duration::from_secs(5)).await;
            if result.is_ok() {
                let mut state = snapshot.lock().await;
                state.state = RunnerProcessState::Stopped;
                state.pid = None;
            }
            changed();
            let _ = reply.send(result);
            false
        }
        Some(RunnerCommand::Shutdown { graceful, reply }) => {
            let result = stop_process(child, &spec.runner_id, graceful).await;
            if result.is_ok() {
                let mut state = snapshot.lock().await;
                state.state = RunnerProcessState::Stopped;
                state.pid = None;
            }
            changed();
            let _ = reply.send(result);
            true
        }
        None => true,
    }
}

fn spawn_child(spec: &ManagedRunnerSpec) -> RunnerSupervisorResult<Child> {
    let mut extra_env = spec.runtime.env.clone();
    extra_env.insert(
        "MUTSUKI_HOME".into(),
        spec.service_home.to_string_lossy().into_owned(),
    );
    // Minted per spawn rather than carried in the spec: the token identifies one process
    // incarnation, so a restart invalidates the previous one, and it never derives from the
    // control token, which would make every sidecar a control-plane superuser.
    extra_env.insert(
        "MUTSUKI_RUNNER_SESSION_TOKEN".into(),
        generate_runner_session_token(),
    );
    extra_env.insert("MUTSUKI_RUNNER_ID".into(), spec.runner_id.clone());
    extra_env.insert("MUTSUKI_PLUGIN_ID".into(), spec.plugin_id.clone());
    let envs = filtered_environment(&spec.env_allowlist, extra_env);

    let mut command = Command::new(&spec.runtime.command);
    command
        .args(&spec.runtime.args)
        .env_clear()
        .envs(envs)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(cwd) = &spec.runtime.cwd {
        command.current_dir(cwd);
    }
    let mut child = command
        .spawn()
        .map_err(|source| RunnerSupervisorError::Spawn {
            runner_id: spec.runner_id.clone(),
            source,
        })?;
    if let Some(stdout) = child.stdout.take() {
        let runner_id = spec.runner_id.clone();
        tokio::spawn(drain_stream(runner_id, "stdout", stdout));
    }
    if let Some(stderr) = child.stderr.take() {
        let runner_id = spec.runner_id.clone();
        tokio::spawn(drain_stream(runner_id, "stderr", stderr));
    }
    Ok(child)
}

async fn drain_stream<R>(runner_id: String, stream: &'static str, reader: R)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut lines = BufReader::new(reader).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => tracing::info!(runner_id, stream, line),
            Ok(None) => break,
            Err(error) => {
                tracing::warn!(runner_id, stream, error = %error, "runner stream read failed");
                break;
            }
        }
    }
}

async fn stop_process(
    child: &mut Option<Child>,
    runner_id: &str,
    graceful: Duration,
) -> RunnerSupervisorResult<()> {
    let Some(mut child) = child.take() else {
        return Ok(());
    };
    if let Ok(Some(_)) = child.try_wait() {
        return Ok(());
    }
    if timeout(graceful, child.wait()).await.is_err() {
        child
            .kill()
            .await
            .map_err(|source| RunnerSupervisorError::Stop {
                runner_id: runner_id.into(),
                source,
            })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use super::*;

    #[tokio::test]
    async fn process_exit_updates_snapshot_without_a_list_probe() {
        let (changed, mut changes) = tokio::sync::mpsc::unbounded_channel();
        let supervisor = RunnerSupervisor::with_change_listener(Arc::new(move || {
            let _ = changed.send(());
        }));
        supervisor.start(exit_spec("short-lived")).await.unwrap();
        changes.recv().await.expect("start change");
        tokio::time::timeout(Duration::from_secs(2), changes.recv())
            .await
            .expect("exit change")
            .expect("change channel");
        let snapshot = supervisor.list().await;
        assert!(
            matches!(snapshot[0].state, RunnerProcessState::Exited(0)),
            "runner state was {:?} (last_error: {:?})",
            snapshot[0].state,
            snapshot[0].last_error,
        );
        supervisor
            .remove("short-lived", Duration::from_millis(50))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn reconcile_keeps_running_specs_and_removes_missing_specs() {
        let supervisor = RunnerSupervisor::new();
        let spec = sleeping_spec("sidecar-a");

        let errors = supervisor
            .reconcile(vec![spec.clone()], Duration::from_millis(500))
            .await;
        assert!(errors.is_empty());
        let first = supervisor.list().await;
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].runner_id, "sidecar-a");
        assert!(matches!(first[0].state, RunnerProcessState::Running));

        let errors = supervisor
            .reconcile(vec![spec], Duration::from_millis(500))
            .await;
        assert!(errors.is_empty());
        let kept = supervisor.list().await;
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].runner_id, "sidecar-a");
        assert_eq!(kept[0].restarts, 0);

        supervisor
            .remove("sidecar-a", Duration::from_millis(500))
            .await
            .unwrap();
        assert!(supervisor.list().await.is_empty());
        supervisor
            .remove("sidecar-a", Duration::from_millis(500))
            .await
            .unwrap();
    }

    /// `restart = true` has to actually restart. Without this the setting is inert configuration
    /// and a crashed sidecar stays down until an operator notices.
    #[tokio::test]
    async fn crashed_runner_is_restarted_under_the_configured_budget() {
        let supervisor = RunnerSupervisor::new();
        let mut spec = exit_spec("crash-loop");
        spec.restart = RestartPolicy {
            enabled: true,
            max_per_minute: 2,
        };
        supervisor.start(spec).await.expect("start");

        let snapshot = wait_for_snapshot("crash-loop", &supervisor, |snapshot| {
            matches!(snapshot.state, RunnerProcessState::Failed)
        })
        .await;

        // Two restarts, then the budget is spent and the runner is parked for an operator.
        assert_eq!(snapshot.restarts, 2);
        assert!(
            snapshot
                .last_error
                .as_deref()
                .is_some_and(|error| error.contains("restart budget exhausted"))
        );

        supervisor
            .remove("crash-loop", Duration::from_millis(200))
            .await
            .expect("remove");
    }

    /// An operator stop must stick. Restarting a Runner the operator just stopped would make the
    /// stop command silently ineffective.
    #[tokio::test]
    async fn restart_policy_does_not_resurrect_an_operator_stopped_runner() {
        let supervisor = RunnerSupervisor::new();
        let mut spec = sleeping_spec("stoppable");
        spec.restart = RestartPolicy {
            enabled: true,
            max_per_minute: 5,
        };
        supervisor.start(spec).await.expect("start");
        supervisor.stop("stoppable").await.expect("stop");

        tokio::time::sleep(Duration::from_millis(400)).await;

        let snapshot = supervisor.list().await;
        assert!(matches!(snapshot[0].state, RunnerProcessState::Stopped));
        assert_eq!(snapshot[0].restarts, 0);

        supervisor
            .remove("stoppable", Duration::from_millis(200))
            .await
            .expect("remove");
    }

    /// The default policy leaves a crashed Runner down, so an embedder that never opts in does
    /// not silently inherit a restart loop.
    #[tokio::test]
    async fn disabled_restart_policy_leaves_a_crashed_runner_down() {
        let supervisor = RunnerSupervisor::new();
        supervisor
            .start(exit_spec("no-restart"))
            .await
            .expect("start");

        let snapshot = wait_for_snapshot("no-restart", &supervisor, |snapshot| {
            matches!(snapshot.state, RunnerProcessState::Exited(_))
        })
        .await;
        assert_eq!(snapshot.restarts, 0);

        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(supervisor.list().await[0].restarts, 0);

        supervisor
            .remove("no-restart", Duration::from_millis(200))
            .await
            .expect("remove");
    }

    async fn wait_for_snapshot(
        runner_id: &str,
        supervisor: &RunnerSupervisor,
        predicate: impl Fn(&RunnerSnapshot) -> bool,
    ) -> RunnerSnapshot {
        for _ in 0..400 {
            if let Some(snapshot) = supervisor
                .list()
                .await
                .into_iter()
                .find(|snapshot| snapshot.runner_id == runner_id)
                && predicate(&snapshot)
            {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("runner {runner_id} never reached the expected state");
    }

    /// The session token is a per-incarnation credential, not a service-wide one: a restart must
    /// invalidate the value the previous process held.
    #[tokio::test]
    async fn each_spawn_hands_the_child_a_freshly_minted_session_token() {
        let dir = tempfile::tempdir().expect("temp dir");
        let log = dir.path().join("tokens.txt");
        let supervisor = RunnerSupervisor::new();
        supervisor
            .start(token_echo_spec("token-echo", &log))
            .await
            .expect("start");
        wait_for_lines(&log, 1).await;
        supervisor.restart("token-echo").await.expect("restart");
        let tokens = wait_for_lines(&log, 2).await;
        supervisor
            .remove("token-echo", Duration::from_millis(500))
            .await
            .expect("remove");

        assert_eq!(tokens.len(), 2);
        assert_ne!(tokens[0], tokens[1]);
        assert!(tokens.iter().all(|token| token.starts_with("runner-")));
    }

    async fn wait_for_lines(path: &std::path::Path, expected: usize) -> Vec<String> {
        for _ in 0..200 {
            if let Ok(contents) = std::fs::read_to_string(path) {
                let lines = contents
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>();
                if lines.len() >= expected {
                    return lines;
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!(
            "child never wrote {expected} session token line(s) to {}",
            path.display()
        );
    }

    fn token_echo_spec(runner_id: &str, log: &std::path::Path) -> ManagedRunnerSpec {
        let log = log.display().to_string();
        #[cfg(windows)]
        let (command, args) = (
            "powershell".into(),
            vec![
                "-NoProfile".into(),
                "-Command".into(),
                format!(
                    "Add-Content -Path '{log}' -Value $env:MUTSUKI_RUNNER_SESSION_TOKEN; Start-Sleep -Seconds 30"
                ),
            ],
        );
        #[cfg(unix)]
        let (command, args) = (
            "/bin/sh".into(),
            vec![
                "-c".into(),
                format!("echo \"$MUTSUKI_RUNNER_SESSION_TOKEN\" >> '{log}'; sleep 30"),
            ],
        );
        ManagedRunnerSpec {
            runner_id: runner_id.into(),
            plugin_id: "plugin-a".into(),
            runtime: mutsuki_service_plugin_loader::ExternalRuntimeSpec {
                command,
                args,
                env: BTreeMap::new(),
                cwd: Option::<PathBuf>::None,
                runner_link: "sidecar".into(),
            },
            env_allowlist: Vec::new(),
            service_home: PathBuf::from("."),
            restart: RestartPolicy::default(),
        }
    }

    fn sleeping_spec(runner_id: &str) -> ManagedRunnerSpec {
        #[cfg(windows)]
        let (command, args) = (
            "powershell".into(),
            vec![
                "-NoProfile".into(),
                "-Command".into(),
                "Start-Sleep -Seconds 30".into(),
            ],
        );
        #[cfg(unix)]
        let (command, args) = ("/bin/sh".into(), vec!["-c".into(), "sleep 30".into()]);
        ManagedRunnerSpec {
            runner_id: runner_id.into(),
            plugin_id: "plugin-a".into(),
            runtime: mutsuki_service_plugin_loader::ExternalRuntimeSpec {
                command,
                args,
                env: BTreeMap::new(),
                cwd: Option::<PathBuf>::None,
                runner_link: "sidecar".into(),
            },
            env_allowlist: Vec::new(),
            service_home: PathBuf::from("."),
            restart: RestartPolicy::default(),
        }
    }

    fn exit_spec(runner_id: &str) -> ManagedRunnerSpec {
        #[cfg(windows)]
        let (command, args) = (
            "powershell".into(),
            vec!["-NoProfile".into(), "-Command".into(), "exit 0".into()],
        );
        #[cfg(unix)]
        let (command, args) = ("/bin/sh".into(), vec!["-c".into(), "exit 0".into()]);
        ManagedRunnerSpec {
            runner_id: runner_id.into(),
            plugin_id: "plugin-a".into(),
            runtime: mutsuki_service_plugin_loader::ExternalRuntimeSpec {
                command,
                args,
                env: BTreeMap::new(),
                cwd: Option::<PathBuf>::None,
                runner_link: "sidecar".into(),
            },
            env_allowlist: Vec::new(),
            service_home: PathBuf::from("."),
            restart: RestartPolicy::default(),
        }
    }
}
