use std::collections::BTreeMap;
use std::io::BufReader;
use std::path::PathBuf;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};

use mutsuki_runtime_contracts::{CompletionBatch, RunnerDescriptor, WorkBatch};
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeResult};

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

pub struct SpawnedJsonlRunner {
    inner: JsonlRunner<BufReader<ChildStdout>, ChildStdin>,
    child: Child,
    stderr: Option<ChildStderr>,
}

impl SpawnedJsonlRunner {
    pub fn spawn(descriptor: RunnerDescriptor, spec: &ProcessRunnerSpec) -> RuntimeResult<Self> {
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
        Ok(Self {
            inner: JsonlRunner::new(descriptor, BufReader::new(stdout), stdin),
            child,
            stderr,
        })
    }

    pub fn child_id(&self) -> u32 {
        self.child.id()
    }

    pub fn take_stderr(&mut self) -> Option<ChildStderr> {
        self.stderr.take()
    }

    pub fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    pub fn kill(&mut self) -> std::io::Result<()> {
        self.child.kill()
    }

    pub fn wait(&mut self) -> std::io::Result<ExitStatus> {
        self.child.wait()
    }
}

impl Runner for SpawnedJsonlRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        self.inner.descriptor()
    }

    fn run_batch(
        &mut self,
        ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        self.inner.run_batch(ctx, batch)
    }

    fn cancel(&mut self, invocation_id: &str) -> RuntimeResult<()> {
        self.inner.cancel(invocation_id)
    }

    fn dispose(&mut self) -> RuntimeResult<()> {
        self.inner.dispose()
    }
}

impl Drop for SpawnedJsonlRunner {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
