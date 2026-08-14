use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

use mutsuki_service_config::filtered_environment;
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
    pub session_token: String,
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
    loop {
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
                }
                command = commands.recv() => {
                    if handle_runner_command(command, &spec, &snapshot, &mut child, &changed).await {
                        break;
                    }
                }
            }
        } else {
            let command = commands.recv().await;
            if handle_runner_command(command, &spec, &snapshot, &mut child, &changed).await {
                break;
            }
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
    extra_env.insert(
        "MUTSUKI_RUNNER_SESSION_TOKEN".into(),
        spec.session_token.clone(),
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
        assert!(matches!(snapshot[0].state, RunnerProcessState::Exited(0)));
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
            session_token: "token".into(),
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
            session_token: "token".into(),
        }
    }
}
