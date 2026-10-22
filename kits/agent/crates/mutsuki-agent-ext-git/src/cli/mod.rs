//! Real `git` CLI backend: process execution, cancellation and worktree state tokens.

mod gateway;

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mutsuki_agent_contracts::{
    AgentError, GitHeadIdentity, GitRepositoryRef, GitWorktreeRef, GitWorktreeState,
};
use sha2::{Digest, Sha256};

use crate::gateway::GitGateway;
use crate::support::digest_bytes;

/// Real `git` CLI backend. Credentials stay in the process environment and are
/// never copied into task/trace payloads.
pub struct CliGitBackend {
    git_bin: PathBuf,
    children: Mutex<BTreeMap<String, Child>>,
    observed_states: Mutex<BTreeMap<String, (GitWorktreeState, u64)>>,
}

impl Default for CliGitBackend {
    fn default() -> Self {
        Self {
            git_bin: PathBuf::from("git"),
            children: Mutex::new(BTreeMap::new()),
            observed_states: Mutex::new(BTreeMap::new()),
        }
    }
}

impl CliGitBackend {
    pub fn new(git_bin: impl Into<PathBuf>) -> Self {
        Self {
            git_bin: git_bin.into(),
            children: Mutex::new(BTreeMap::new()),
            observed_states: Mutex::new(BTreeMap::new()),
        }
    }

    fn run(
        &self,
        cwd: &Path,
        args: &[&str],
        cancel: Option<&AtomicBool>,
        handle_id: Option<&str>,
    ) -> Result<String, AgentError> {
        if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            return Err(AgentError::new(
                "agent.git.cancelled",
                "git operation was cancelled",
            ));
        }
        let mut child = Command::new(&self.git_bin)
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_remove("GIT_ASKPASS")
            .env_remove("SSH_ASKPASS")
            .spawn()
            .map_err(|error| AgentError::new("agent.git.spawn_failed", error.to_string()))?;
        if let Some(handle_id) = handle_id {
            // Track pid-bearing child for cancel; wait via try_wait loop.
            let stdout = child.stdout.take();
            let stderr = child.stderr.take();
            self.children
                .lock()
                .expect("git children")
                .insert(handle_id.into(), child);
            let started = Instant::now();
            loop {
                if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
                    let _ = self.cancel_process(handle_id);
                    return Err(AgentError::new(
                        "agent.git.cancelled",
                        "git operation was cancelled",
                    ));
                }
                let mut children = self.children.lock().expect("git children");
                let Some(child) = children.get_mut(handle_id) else {
                    return Err(AgentError::new(
                        "agent.git.process_failed",
                        "git child disappeared",
                    ));
                };
                match child.try_wait() {
                    Ok(Some(status)) => {
                        let mut child = children.remove(handle_id).expect("child");
                        drop(children);
                        let mut stdout_buf = Vec::new();
                        let mut stderr_buf = Vec::new();
                        if let Some(mut out) = stdout {
                            let _ = std::io::Read::read_to_end(&mut out, &mut stdout_buf);
                        }
                        if let Some(mut err) = stderr {
                            let _ = std::io::Read::read_to_end(&mut err, &mut stderr_buf);
                        }
                        let _ = child.wait();
                        if !status.success() {
                            let stderr = String::from_utf8_lossy(&stderr_buf).trim().to_string();
                            return Err(AgentError::new(
                                "agent.git.command_failed",
                                if stderr.is_empty() {
                                    format!("git {} failed", args.join(" "))
                                } else {
                                    stderr
                                },
                            ));
                        }
                        return Ok(String::from_utf8_lossy(&stdout_buf).into_owned());
                    }
                    Ok(None) => {
                        drop(children);
                        if started.elapsed() > Duration::from_mins(2) {
                            let _ = self.cancel_process(handle_id);
                            return Err(AgentError::new(
                                "agent.git.timeout",
                                "git operation exceeded backend timeout",
                            ));
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => {
                        children.remove(handle_id);
                        return Err(AgentError::new(
                            "agent.git.process_failed",
                            error.to_string(),
                        ));
                    }
                }
            }
        }
        let output = child
            .wait_with_output()
            .map_err(|error| AgentError::new("agent.git.process_failed", error.to_string()))?;
        if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            return Err(AgentError::new(
                "agent.git.cancelled",
                "git operation was cancelled",
            ));
        }
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(AgentError::new(
                "agent.git.command_failed",
                if stderr.is_empty() {
                    format!("git {} failed", args.join(" "))
                } else {
                    stderr
                },
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    fn run_bytes(&self, cwd: &Path, args: &[&str]) -> Result<Vec<u8>, AgentError> {
        let output = Command::new(&self.git_bin)
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_remove("GIT_ASKPASS")
            .env_remove("SSH_ASKPASS")
            .output()
            .map_err(|error| AgentError::new("agent.git.spawn_failed", error.to_string()))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(AgentError::new(
                "agent.git.command_failed",
                if stderr.is_empty() {
                    format!("git {} failed", args.join(" "))
                } else {
                    stderr
                },
            ));
        }
        Ok(output.stdout)
    }

    fn state_token(
        &self,
        worktree: &GitWorktreeRef,
        head: &GitHeadIdentity,
    ) -> Result<GitWorktreeState, AgentError> {
        let cwd = Path::new(&worktree.path);
        let index_patch = self.run_bytes(
            cwd,
            &[
                "diff",
                "--cached",
                "--binary",
                "--no-ext-diff",
                "--full-index",
            ],
        )?;
        let tracked_patch =
            self.run_bytes(cwd, &["diff", "--binary", "--no-ext-diff", "--full-index"])?;
        let untracked =
            self.run_bytes(cwd, &["ls-files", "--others", "--exclude-standard", "-z"])?;
        let mut worktree_hasher = Sha256::new();
        worktree_hasher.update((tracked_patch.len() as u64).to_le_bytes());
        worktree_hasher.update(&tracked_patch);
        for path in untracked
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
        {
            let relative = std::str::from_utf8(path).map_err(|_| {
                AgentError::invalid_input("git state cannot represent a non-UTF-8 path")
            })?;
            let relative_path = Path::new(relative);
            if relative_path.is_absolute()
                || relative_path.components().any(|component| {
                    matches!(
                        component,
                        Component::ParentDir | Component::RootDir | Component::Prefix(_)
                    )
                })
            {
                return Err(AgentError::invalid_input(
                    "git returned an unsafe untracked path",
                ));
            }
            worktree_hasher.update((path.len() as u64).to_le_bytes());
            worktree_hasher.update(path);
            let absolute = cwd.join(relative_path);
            let metadata = fs::symlink_metadata(&absolute)
                .map_err(|error| AgentError::new("agent.git.state_failed", error.to_string()))?;
            if metadata.file_type().is_symlink() {
                worktree_hasher.update(b"symlink");
                let target = fs::read_link(&absolute).map_err(|error| {
                    AgentError::new("agent.git.state_failed", error.to_string())
                })?;
                let target = target.as_os_str().as_encoded_bytes();
                worktree_hasher.update((target.len() as u64).to_le_bytes());
                worktree_hasher.update(target);
            } else if metadata.is_file() {
                worktree_hasher.update(b"file");
                worktree_hasher.update(metadata.len().to_le_bytes());
                let mut file = File::open(&absolute).map_err(|error| {
                    AgentError::new("agent.git.state_failed", error.to_string())
                })?;
                let mut buffer = [0_u8; 64 * 1024];
                loop {
                    let read = file.read(&mut buffer).map_err(|error| {
                        AgentError::new("agent.git.state_failed", error.to_string())
                    })?;
                    if read == 0 {
                        break;
                    }
                    worktree_hasher.update(&buffer[..read]);
                }
            } else {
                worktree_hasher.update(b"other");
            }
        }
        Ok(GitWorktreeState {
            head_commit: head.commit.clone(),
            head_ref: head.branch.clone(),
            index_hash: digest_bytes(&index_patch),
            worktree_hash: format!("sha256:{}", hex::encode(worktree_hasher.finalize())),
        })
    }

    fn observe_generation(&self, worktree: &GitWorktreeRef, state: &GitWorktreeState) -> u64 {
        let mut observed = self.observed_states.lock().expect("git observed state");
        let entry = observed
            .entry(worktree.worktree_id.clone())
            .or_insert_with(|| (state.clone(), 1));
        if &entry.0 != state {
            entry.0 = state.clone();
            entry.1 = entry.1.saturating_add(1);
        }
        entry.1
    }

    fn inspect_path(
        &self,
        path: &str,
    ) -> Result<(GitWorktreeRef, GitHeadIdentity, GitWorktreeState), AgentError> {
        for _ in 0..3 {
            let (worktree, first_head) = self.parse_worktree(path)?;
            let first_state = self.state_token(&worktree, &first_head)?;
            let (_, mut second_head) = self.parse_worktree(path)?;
            let second_state = self.state_token(&worktree, &second_head)?;
            if first_head.commit == second_head.commit
                && first_head.branch == second_head.branch
                && first_head.upstream == second_head.upstream
                && first_state == second_state
            {
                second_head.generation = self.observe_generation(&worktree, &second_state);
                return Ok((worktree, second_head, second_state));
            }
        }
        Err(AgentError::new(
            "agent.git.state_unstable",
            "git state changed while the concurrency token was captured",
        ))
    }

    fn parse_worktree(&self, path: &str) -> Result<(GitWorktreeRef, GitHeadIdentity), AgentError> {
        let root = PathBuf::from(path);
        let toplevel = self
            .run(&root, &["rev-parse", "--show-toplevel"], None, None)?
            .trim()
            .to_string();
        let commit = self
            .run(Path::new(&toplevel), &["rev-parse", "HEAD"], None, None)?
            .trim()
            .to_string();
        let branch = self
            .run(
                Path::new(&toplevel),
                &["rev-parse", "--abbrev-ref", "HEAD"],
                None,
                None,
            )?
            .trim()
            .to_string();
        let upstream = self
            .run(
                Path::new(&toplevel),
                &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
                None,
                None,
            )
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let repository = GitRepositoryRef {
            repo_id: format!("repo:{toplevel}"),
            root: toplevel.clone(),
        };
        Ok((
            GitWorktreeRef {
                worktree_id: format!("wt:{toplevel}"),
                path: toplevel,
                repository,
            },
            GitHeadIdentity {
                commit,
                branch: Some(branch),
                upstream,
                generation: 1,
            },
        ))
    }
}
