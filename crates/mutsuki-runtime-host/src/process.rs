use std::collections::BTreeMap;
use std::fmt;
use std::io::{BufReader, sink};
use std::path::PathBuf;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use mutsuki_runtime_contracts::{CompletionBatch, RunnerDescriptor, WorkBatch};
use mutsuki_runtime_core::{
    Runner, RunnerContext, RunnerIsolation, RunnerManagementHandle, RunnerTerminationHandle,
    RuntimeResult,
};

use crate::JsonlRunner;
use crate::error::host_failure;

/// Host-neutral launch input. Callers must resolve policy, secrets and the exact
/// environment before spawning; this helper never inherits the ambient process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessRunnerSpec {
    pub command: PathBuf,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: BTreeMap<String, String>,
}

impl ProcessRunnerSpec {
    pub fn new(command: impl Into<PathBuf>) -> Self {
        Self {
            command: command.into(),
            args: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
        }
    }
}

#[derive(Default)]
struct ProcessControl {
    child: Mutex<Option<Child>>,
}

impl fmt::Debug for ProcessControl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let child_id = self
            .child
            .lock()
            .ok()
            .and_then(|child| child.as_ref().map(Child::id));
        formatter
            .debug_struct("ProcessControl")
            .field("child_id", &child_id)
            .finish()
    }
}

impl RunnerTerminationHandle for ProcessControl {
    fn terminate(&self) -> RuntimeResult<()> {
        let mut child = self
            .child
            .lock()
            .map_err(|_| host_failure("process_runner.terminate", "child lock poisoned"))?;
        let child = child
            .as_mut()
            .ok_or_else(|| host_failure("process_runner.terminate", "child is not running"))?;
        match child.try_wait() {
            Ok(Some(_)) => Ok(()),
            Ok(None) => child
                .kill()
                .map_err(|error| host_failure("process_runner.terminate", error.to_string())),
            Err(error) => Err(host_failure("process_runner.terminate", error.to_string())),
        }
    }
}

type ProcessJsonlRunner = JsonlRunner<BufReader<ChildStdout>, ChildStdin>;

pub struct SpawnedJsonlRunner {
    descriptor: RunnerDescriptor,
    spec: ProcessRunnerSpec,
    inner: Option<ProcessJsonlRunner>,
    control: Arc<ProcessControl>,
    stderr: Option<ChildStderr>,
}

impl SpawnedJsonlRunner {
    pub fn spawn(descriptor: RunnerDescriptor, spec: &ProcessRunnerSpec) -> RuntimeResult<Self> {
        let control = Arc::new(ProcessControl::default());
        let (inner, child, stderr) = spawn_process(&descriptor, spec)?;
        *control
            .child
            .lock()
            .map_err(|_| host_failure("process_runner.spawn", "child lock poisoned"))? =
            Some(child);
        Ok(Self {
            descriptor,
            spec: spec.clone(),
            inner: Some(inner),
            control,
            stderr,
        })
    }

    pub fn child_id(&self) -> u32 {
        self.control
            .child
            .lock()
            .expect("process runner child lock poisoned")
            .as_ref()
            .map(Child::id)
            .unwrap_or_default()
    }

    pub fn take_stderr(&mut self) -> Option<ChildStderr> {
        self.stderr.take()
    }

    pub fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        let mut child = self
            .control
            .child
            .lock()
            .expect("process runner child lock poisoned");
        match child.as_mut() {
            Some(child) => child.try_wait(),
            None => Ok(None),
        }
    }

    pub fn kill(&mut self) -> std::io::Result<()> {
        let mut child = self
            .control
            .child
            .lock()
            .expect("process runner child lock poisoned");
        match child.as_mut() {
            Some(child) => child.kill(),
            None => Ok(()),
        }
    }

    pub fn wait(&mut self) -> std::io::Result<ExitStatus> {
        let mut child = self
            .control
            .child
            .lock()
            .expect("process runner child lock poisoned");
        child
            .as_mut()
            .expect("process runner child unavailable")
            .wait()
    }

    fn recover_process(&mut self) -> RuntimeResult<()> {
        self.inner.take();
        self.stderr.take();
        if let Some(mut child) = self
            .control
            .child
            .lock()
            .map_err(|_| host_failure("process_runner.recover", "child lock poisoned"))?
            .take()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
        let (inner, child, stderr) = spawn_process(&self.descriptor, &self.spec)?;
        // Recovered stderr must always be drained even though the original caller can choose to
        // take and log the initial stream.
        if let Some(stderr) = stderr {
            thread::Builder::new()
                .name(format!(
                    "mutsuki-{}-stderr",
                    self.descriptor.runner_id.replace(['/', ':'], "-")
                ))
                .spawn(move || {
                    let mut stderr = stderr;
                    let _ = std::io::copy(&mut stderr, &mut sink());
                })
                .map_err(|error| host_failure("process_runner.stderr", error.to_string()))?;
        }
        *self
            .control
            .child
            .lock()
            .map_err(|_| host_failure("process_runner.recover", "child lock poisoned"))? =
            Some(child);
        self.inner = Some(inner);
        Ok(())
    }
}

impl Runner for SpawnedJsonlRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        self.inner
            .as_mut()
            .ok_or_else(|| host_failure("process_runner.run_batch", "runner is not connected"))?
            .run_batch(ctx, batch)
    }

    fn cancel(&mut self, invocation_id: &str) -> RuntimeResult<()> {
        self.inner
            .as_mut()
            .ok_or_else(|| host_failure("process_runner.cancel", "runner is not connected"))?
            .cancel(invocation_id)
    }

    fn dispose(&mut self) -> RuntimeResult<()> {
        if let Some(inner) = self.inner.as_mut() {
            inner.dispose()?;
        }
        Ok(())
    }

    fn isolation(&self) -> RunnerIsolation {
        RunnerIsolation::HardProcess(self.control.clone())
    }

    fn management_handle(&self) -> Option<Arc<dyn RunnerManagementHandle>> {
        self.inner
            .as_ref()
            .and_then(|runner| runner.management_handle())
    }

    fn recover_after_hard_termination(&mut self) -> RuntimeResult<()> {
        self.recover_process()
    }
}

impl Drop for SpawnedJsonlRunner {
    fn drop(&mut self) {
        self.inner.take();
        if let Ok(mut child) = self.control.child.lock()
            && let Some(mut child) = child.take()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn spawn_process(
    descriptor: &RunnerDescriptor,
    spec: &ProcessRunnerSpec,
) -> RuntimeResult<(ProcessJsonlRunner, Child, Option<ChildStderr>)> {
    let mut command = Command::new(&spec.command);
    command
        .args(&spec.args)
        .env_clear()
        .envs(&spec.env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = &spec.cwd {
        command.current_dir(cwd);
    }
    let mut child = command.spawn().map_err(|error| {
        host_failure(
            "process_runner.spawn",
            format!("{}: {error}", spec.command.display()),
        )
    })?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| host_failure("process_runner.stdin", "child stdin unavailable"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| host_failure("process_runner.stdout", "child stdout unavailable"))?;
    let stderr = child.stderr.take();
    Ok((
        JsonlRunner::new(descriptor.clone(), BufReader::new(stdout), stdin),
        child,
        stderr,
    ))
}
