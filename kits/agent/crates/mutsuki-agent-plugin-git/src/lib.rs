use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mutsuki_agent_contracts::{
    AgentError, AgentPluginStateKind, AgentServiceDescriptor, AgentToolDescriptor, GitActionPlan,
    GitBlameLine, GitBlameResult, GitBranchInfo, GitCommitInfo, GitCommitResult, GitDiffRequest,
    GitDiffResult, GitDiffScope, GitFileChange, GitFileStatus, GitHeadIdentity, GitLogResult,
    GitOperationHandle, GitOperationKind, GitOperationState, GitRepositoryRef, GitRevisionConflict,
    GitRisk, GitServiceRequest, GitServiceResponse, GitShowResult, GitStatusSnapshot,
    GitWorktreeInfo, GitWorktreeRef, GitWorktreeState, GitWriteContext, PermissionRequest,
    ResourceRef,
};
use mutsuki_agent_plugin_api::{AgentPluginRegistrar, AgentService, ContextProvider, ToolProvider};
use mutsuki_agent_runtime::AgentResourceStore;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub const PLUGIN_ID: &str = "mutsuki.plugin.agent.git";
pub const SERVICE_ID: &str = "mutsuki.agent.service.git";
pub const CONTEXT_PROVIDER_ID: &str = "mutsuki.agent.context.git";
pub const INLINE_LIMIT: usize = 2_048;
pub const SUMMARY_LIMIT: usize = 512;

fn digest_bytes(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

fn digest_json(value: &impl serde::Serialize) -> String {
    digest_bytes(&serde_json::to_vec(value).expect("git state must serialize"))
}

/// Process / VCS execution backend. Host injects CLI or test doubles; AgentKit
/// owns Git domain semantics only.
pub trait GitGateway: Send + Sync {
    fn discover(&self, path: &str) -> Result<(GitWorktreeRef, GitHeadIdentity), AgentError>;
    fn status(&self, worktree: &GitWorktreeRef) -> Result<GitStatusSnapshot, AgentError>;
    fn diff(&self, request: &GitDiffRequest) -> Result<(GitDiffResult, String), AgentError>;
    fn stage(&self, worktree: &GitWorktreeRef, paths: &[String]) -> Result<(), AgentError>;
    fn unstage(&self, worktree: &GitWorktreeRef, paths: &[String]) -> Result<(), AgentError>;
    fn commit(
        &self,
        worktree: &GitWorktreeRef,
        message: &str,
        allow_empty: bool,
    ) -> Result<GitCommitResult, AgentError>;
    fn branch_list(&self, worktree: &GitWorktreeRef) -> Result<Vec<GitBranchInfo>, AgentError>;
    fn branch_create(
        &self,
        worktree: &GitWorktreeRef,
        name: &str,
        checkout: bool,
    ) -> Result<(GitBranchInfo, GitHeadIdentity), AgentError>;
    fn branch_switch(
        &self,
        worktree: &GitWorktreeRef,
        name: &str,
    ) -> Result<(GitBranchInfo, GitHeadIdentity), AgentError>;
    fn branch_delete(
        &self,
        worktree: &GitWorktreeRef,
        name: &str,
        force: bool,
    ) -> Result<(), AgentError>;
    fn worktree_list(
        &self,
        repository: &GitRepositoryRef,
    ) -> Result<Vec<GitWorktreeInfo>, AgentError>;
    fn worktree_create(
        &self,
        repository: &GitRepositoryRef,
        path: &str,
        branch: Option<&str>,
    ) -> Result<GitWorktreeInfo, AgentError>;
    fn worktree_remove(&self, worktree: &GitWorktreeRef, force: bool) -> Result<(), AgentError>;
    fn log(
        &self,
        worktree: &GitWorktreeRef,
        limit: u32,
        path: Option<&str>,
    ) -> Result<(Vec<GitCommitInfo>, String), AgentError>;
    fn show(
        &self,
        worktree: &GitWorktreeRef,
        revision: &str,
    ) -> Result<(GitShowResult, String), AgentError>;
    fn blame(
        &self,
        worktree: &GitWorktreeRef,
        path: &str,
        max_lines: u32,
    ) -> Result<(GitBlameResult, String), AgentError>;
    fn push(
        &self,
        worktree: &GitWorktreeRef,
        remote: &str,
        refspec: Option<&str>,
        force: bool,
        cancel: &AtomicBool,
    ) -> Result<(String, GitHeadIdentity), AgentError>;
    fn pull(
        &self,
        worktree: &GitWorktreeRef,
        remote: &str,
        rebase: bool,
        cancel: &AtomicBool,
    ) -> Result<(String, GitHeadIdentity), AgentError>;
    fn fetch(
        &self,
        worktree: &GitWorktreeRef,
        remote: &str,
        cancel: &AtomicBool,
    ) -> Result<(String, GitHeadIdentity), AgentError>;
    fn merge(
        &self,
        worktree: &GitWorktreeRef,
        revision: &str,
        cancel: &AtomicBool,
    ) -> Result<(String, GitHeadIdentity), AgentError>;
    fn rebase(
        &self,
        worktree: &GitWorktreeRef,
        onto: &str,
        cancel: &AtomicBool,
    ) -> Result<(String, GitHeadIdentity), AgentError>;
    fn reset(
        &self,
        worktree: &GitWorktreeRef,
        revision: &str,
        mode: &str,
        cancel: &AtomicBool,
    ) -> Result<(String, GitHeadIdentity), AgentError>;
    fn cherry_pick(
        &self,
        worktree: &GitWorktreeRef,
        revision: &str,
        cancel: &AtomicBool,
    ) -> Result<(String, GitHeadIdentity), AgentError>;
    fn head(&self, worktree: &GitWorktreeRef) -> Result<GitHeadIdentity, AgentError>;
    fn cancel_process(&self, handle_id: &str) -> Result<(), AgentError>;
}

#[derive(Clone)]
struct FileBlob {
    content: String,
}

#[derive(Clone)]
struct CommitRecord {
    info: GitCommitInfo,
    tree: BTreeMap<String, String>,
}

struct RepoState {
    repository: GitRepositoryRef,
    generation: u64,
    branch: String,
    upstream: Option<String>,
    head: String,
    branches: BTreeMap<String, String>,
    commits: BTreeMap<String, CommitRecord>,
    worktree_files: BTreeMap<String, FileBlob>,
    staged: BTreeMap<String, Option<String>>,
    worktrees: BTreeMap<String, GitWorktreeRef>,
    remote_heads: BTreeMap<String, String>,
    next_hash: u64,
}

/// Deterministic in-memory Git for conformance / shared Agent+UI tests.
#[derive(Clone, Default)]
pub struct InMemoryGitBackend {
    repos: Arc<Mutex<BTreeMap<String, RepoState>>>,
}

impl InMemoryGitBackend {
    pub fn seed_repo(self, root: impl Into<String>, files: BTreeMap<String, String>) -> Self {
        let root = root.into();
        let repo_id = format!("repo:{}", root);
        let worktree_id = format!("wt:{}", root);
        let mut tree = BTreeMap::new();
        let mut worktree_files = BTreeMap::new();
        for (path, content) in files {
            tree.insert(path.clone(), content.clone());
            worktree_files.insert(path, FileBlob { content });
        }
        let hash = "c0000001".to_string();
        let commit = CommitRecord {
            info: GitCommitInfo {
                hash: hash.clone(),
                summary: "initial".into(),
                author: "mutsuki".into(),
                timestamp_unix_ms: 1,
                parents: Vec::new(),
            },
            tree,
        };
        let repository = GitRepositoryRef {
            repo_id: repo_id.clone(),
            root: root.clone(),
        };
        let mut worktrees = BTreeMap::new();
        worktrees.insert(
            worktree_id.clone(),
            GitWorktreeRef {
                worktree_id,
                path: root.clone(),
                repository: repository.clone(),
            },
        );
        let mut branches = BTreeMap::new();
        branches.insert("main".into(), hash.clone());
        let mut remote_heads = BTreeMap::new();
        remote_heads.insert("origin/main".into(), hash.clone());
        let mut commits = BTreeMap::new();
        commits.insert(hash.clone(), commit);
        self.repos.lock().expect("git mutex").insert(
            repo_id,
            RepoState {
                repository,
                generation: 1,
                branch: "main".into(),
                upstream: Some("origin/main".into()),
                head: hash,
                branches,
                commits,
                worktree_files,
                staged: BTreeMap::new(),
                worktrees,
                remote_heads,
                next_hash: 2,
            },
        );
        self
    }

    fn with_repo_mut<T>(
        &self,
        worktree: &GitWorktreeRef,
        f: impl FnOnce(&mut RepoState) -> Result<T, AgentError>,
    ) -> Result<T, AgentError> {
        let mut repos = self.repos.lock().expect("git mutex");
        let state = repos
            .get_mut(&worktree.repository.repo_id)
            .ok_or_else(|| AgentError::not_found("git repository was not found"))?;
        if !state.worktrees.contains_key(&worktree.worktree_id) {
            return Err(AgentError::not_found("git worktree was not found"));
        }
        f(state)
    }

    fn head_of(state: &RepoState) -> GitHeadIdentity {
        GitHeadIdentity {
            commit: state.head.clone(),
            branch: Some(state.branch.clone()),
            upstream: state.upstream.clone(),
            generation: state.generation,
        }
    }

    fn state_of(state: &RepoState) -> GitWorktreeState {
        let mut index = state
            .commits
            .get(&state.head)
            .map(|commit| commit.tree.clone())
            .unwrap_or_default();
        for (path, content) in &state.staged {
            if let Some(content) = content {
                index.insert(path.clone(), content.clone());
            } else {
                index.remove(path);
            }
        }
        let worktree = state
            .worktree_files
            .iter()
            .map(|(path, blob)| (path.clone(), blob.content.clone()))
            .collect::<BTreeMap<_, _>>();
        GitWorktreeState {
            head_commit: state.head.clone(),
            head_ref: Some(state.branch.clone()),
            index_hash: digest_json(&index),
            worktree_hash: digest_json(&worktree),
        }
    }

    fn bump(state: &mut RepoState) {
        state.generation = state.generation.saturating_add(1);
    }

    fn next_commit_hash(state: &mut RepoState) -> String {
        let hash = format!("c{:07}", state.next_hash);
        state.next_hash = state.next_hash.saturating_add(1);
        hash
    }

    fn collect_changes(state: &RepoState) -> Vec<GitFileChange> {
        let head_tree = state
            .commits
            .get(&state.head)
            .map(|commit| &commit.tree)
            .cloned()
            .unwrap_or_default();
        let mut changes = Vec::new();
        let mut paths = state.worktree_files.keys().cloned().collect::<Vec<_>>();
        for path in head_tree.keys() {
            if !paths.contains(path) {
                paths.push(path.clone());
            }
        }
        for path in state.staged.keys() {
            if !paths.contains(path) {
                paths.push(path.clone());
            }
        }
        paths.sort();
        for path in paths {
            let work = state
                .worktree_files
                .get(&path)
                .map(|blob| blob.content.clone());
            let head = head_tree.get(&path).cloned();
            let staged_entry = state.staged.get(&path).cloned();
            if let Some(staged_content) = &staged_entry {
                let status = match (head.as_ref(), staged_content) {
                    (_, None) => GitFileStatus::Deleted,
                    (None, Some(_)) => GitFileStatus::Added,
                    (Some(prev), Some(next)) if prev != next => GitFileStatus::Modified,
                    _ => GitFileStatus::Modified,
                };
                changes.push(GitFileChange {
                    path: path.clone(),
                    old_path: None,
                    status,
                    staged: true,
                    hunks_summary: Some("staged".into()),
                    additions: 1,
                    deletions: 0,
                });
            }
            let index_text = match &staged_entry {
                Some(content) => content.clone(),
                None => head.clone(),
            };
            match (index_text.as_ref(), work.as_ref()) {
                (Some(index), Some(work_text)) if index != work_text => {
                    if staged_entry.is_none() {
                        changes.push(GitFileChange {
                            path: path.clone(),
                            old_path: None,
                            status: GitFileStatus::Modified,
                            staged: false,
                            hunks_summary: Some("unstaged".into()),
                            additions: 1,
                            deletions: 1,
                        });
                    }
                }
                (None, Some(_)) => {
                    if staged_entry.is_none() {
                        changes.push(GitFileChange {
                            path,
                            old_path: None,
                            status: GitFileStatus::Untracked,
                            staged: false,
                            hunks_summary: None,
                            additions: 1,
                            deletions: 0,
                        });
                    }
                }
                (Some(_), None) if staged_entry.is_none() => {
                    changes.push(GitFileChange {
                        path,
                        old_path: None,
                        status: GitFileStatus::Deleted,
                        staged: false,
                        hunks_summary: Some("deleted".into()),
                        additions: 0,
                        deletions: 1,
                    });
                }
                _ => {}
            }
        }
        changes
    }

    fn build_patch(state: &RepoState, request: &GitDiffRequest) -> (Vec<GitFileChange>, String) {
        let changes = match request.scope {
            GitDiffScope::WorkingTree => Self::collect_changes(state)
                .into_iter()
                .filter(|change| !change.staged)
                .collect::<Vec<_>>(),
            GitDiffScope::Staged => Self::collect_changes(state)
                .into_iter()
                .filter(|change| change.staged)
                .collect::<Vec<_>>(),
            GitDiffScope::Range => {
                let base = request.base.clone().unwrap_or_else(|| state.head.clone());
                let head = request.head.clone().unwrap_or_else(|| state.head.clone());
                let base_tree = state
                    .commits
                    .get(&base)
                    .map(|commit| commit.tree.clone())
                    .unwrap_or_default();
                let head_tree = state
                    .commits
                    .get(&head)
                    .map(|commit| commit.tree.clone())
                    .unwrap_or_default();
                let mut paths = base_tree.keys().cloned().collect::<Vec<_>>();
                for path in head_tree.keys() {
                    if !paths.contains(path) {
                        paths.push(path.clone());
                    }
                }
                paths.sort();
                paths
                    .into_iter()
                    .filter_map(|path| {
                        let left = base_tree.get(&path);
                        let right = head_tree.get(&path);
                        if left == right {
                            return None;
                        }
                        let status = match (left, right) {
                            (None, Some(_)) => GitFileStatus::Added,
                            (Some(_), None) => GitFileStatus::Deleted,
                            _ => GitFileStatus::Modified,
                        };
                        Some(GitFileChange {
                            path,
                            old_path: None,
                            status,
                            staged: false,
                            hunks_summary: Some("range".into()),
                            additions: 1,
                            deletions: 1,
                        })
                    })
                    .collect()
            }
        };
        let filtered = if request.paths.is_empty() {
            changes
        } else {
            changes
                .into_iter()
                .filter(|change| request.paths.iter().any(|path| path == &change.path))
                .collect()
        };
        let mut patch = String::new();
        for change in &filtered {
            patch.push_str(&format!(
                "diff --git a/{0} b/{0}\n--- a/{0}\n+++ b/{0}\n@@ status: {1:?} staged={2} @@\n",
                change.path, change.status, change.staged
            ));
            if let Some(content) = state.worktree_files.get(&change.path) {
                for line in content.content.lines() {
                    patch.push('+');
                    patch.push_str(line);
                    patch.push('\n');
                }
            }
        }
        (filtered, patch)
    }

    fn check_cancel(cancel: &AtomicBool) -> Result<(), AgentError> {
        if cancel.load(Ordering::Relaxed) {
            Err(AgentError::new(
                "agent.git.cancelled",
                "git operation was cancelled",
            ))
        } else {
            Ok(())
        }
    }
}

impl GitGateway for InMemoryGitBackend {
    fn discover(&self, path: &str) -> Result<(GitWorktreeRef, GitHeadIdentity), AgentError> {
        let repos = self.repos.lock().expect("git mutex");
        for state in repos.values() {
            if let Some(worktree) = state.worktrees.values().find(|wt| wt.path == path) {
                return Ok((worktree.clone(), Self::head_of(state)));
            }
            if state.repository.root == path {
                let worktree = state
                    .worktrees
                    .values()
                    .next()
                    .cloned()
                    .ok_or_else(|| AgentError::not_found("git worktree was not found"))?;
                return Ok((worktree, Self::head_of(state)));
            }
        }
        Err(AgentError::not_found(format!(
            "no git repository discovered at `{path}`"
        )))
    }

    fn status(&self, worktree: &GitWorktreeRef) -> Result<GitStatusSnapshot, AgentError> {
        self.with_repo_mut(worktree, |state| {
            let changes = Self::collect_changes(state);
            Ok(GitStatusSnapshot {
                worktree: worktree.clone(),
                head: Self::head_of(state),
                state: Self::state_of(state),
                clean: changes.is_empty(),
                changes,
            })
        })
    }

    fn diff(&self, request: &GitDiffRequest) -> Result<(GitDiffResult, String), AgentError> {
        self.with_repo_mut(&request.worktree, |state| {
            let (files, patch) = Self::build_patch(state, request);
            let head = Self::head_of(state);
            Ok((
                GitDiffResult {
                    worktree: request.worktree.clone(),
                    base: head.clone(),
                    head,
                    summary: format!("{} file(s) changed", files.len()),
                    files,
                    patch_ref: None,
                    inline_patch: None,
                    truncated: false,
                },
                patch,
            ))
        })
    }

    fn stage(&self, worktree: &GitWorktreeRef, paths: &[String]) -> Result<(), AgentError> {
        self.with_repo_mut(worktree, |state| {
            for path in paths {
                let content = state
                    .worktree_files
                    .get(path)
                    .map(|blob| blob.content.clone());
                state.staged.insert(path.clone(), content);
            }
            Self::bump(state);
            Ok(())
        })
    }

    fn unstage(&self, worktree: &GitWorktreeRef, paths: &[String]) -> Result<(), AgentError> {
        self.with_repo_mut(worktree, |state| {
            for path in paths {
                state.staged.remove(path);
            }
            Self::bump(state);
            Ok(())
        })
    }

    fn commit(
        &self,
        worktree: &GitWorktreeRef,
        message: &str,
        allow_empty: bool,
    ) -> Result<GitCommitResult, AgentError> {
        self.with_repo_mut(worktree, |state| {
            if state.staged.is_empty() && !allow_empty {
                return Err(AgentError::invalid_input("nothing staged to commit"));
            }
            let mut tree = state
                .commits
                .get(&state.head)
                .map(|commit| commit.tree.clone())
                .unwrap_or_default();
            let mut changed = Vec::new();
            let staged = std::mem::take(&mut state.staged);
            for (path, content) in staged {
                match content {
                    Some(text) => {
                        tree.insert(path.clone(), text.clone());
                        state
                            .worktree_files
                            .insert(path.clone(), FileBlob { content: text });
                        changed.push(GitFileChange {
                            path,
                            old_path: None,
                            status: GitFileStatus::Modified,
                            staged: true,
                            hunks_summary: Some("committed".into()),
                            additions: 1,
                            deletions: 0,
                        });
                    }
                    None => {
                        tree.remove(&path);
                        state.worktree_files.remove(&path);
                        changed.push(GitFileChange {
                            path,
                            old_path: None,
                            status: GitFileStatus::Deleted,
                            staged: true,
                            hunks_summary: Some("committed".into()),
                            additions: 0,
                            deletions: 1,
                        });
                    }
                }
            }
            let hash = Self::next_commit_hash(state);
            let info = GitCommitInfo {
                hash: hash.clone(),
                summary: message.into(),
                author: "mutsuki".into(),
                timestamp_unix_ms: state.generation.saturating_add(1_000),
                parents: vec![state.head.clone()],
            };
            state.commits.insert(
                hash.clone(),
                CommitRecord {
                    info: info.clone(),
                    tree,
                },
            );
            state.head = hash;
            *state
                .branches
                .entry(state.branch.clone())
                .or_insert_with(|| state.head.clone()) = state.head.clone();
            Self::bump(state);
            Ok(GitCommitResult {
                commit: info,
                head: Self::head_of(state),
                changed_files: changed,
            })
        })
    }

    fn branch_list(&self, worktree: &GitWorktreeRef) -> Result<Vec<GitBranchInfo>, AgentError> {
        self.with_repo_mut(worktree, |state| {
            Ok(state
                .branches
                .iter()
                .map(|(name, tip)| GitBranchInfo {
                    name: name.clone(),
                    tip: tip.clone(),
                    current: name == &state.branch,
                    upstream: if name == &state.branch {
                        state.upstream.clone()
                    } else {
                        None
                    },
                })
                .collect())
        })
    }

    fn branch_create(
        &self,
        worktree: &GitWorktreeRef,
        name: &str,
        checkout: bool,
    ) -> Result<(GitBranchInfo, GitHeadIdentity), AgentError> {
        self.with_repo_mut(worktree, |state| {
            if state.branches.contains_key(name) {
                return Err(AgentError::invalid_input(format!(
                    "branch `{name}` already exists"
                )));
            }
            state.branches.insert(name.into(), state.head.clone());
            if checkout {
                state.branch = name.into();
            }
            Self::bump(state);
            Ok((
                GitBranchInfo {
                    name: name.into(),
                    tip: state.head.clone(),
                    current: checkout,
                    upstream: None,
                },
                Self::head_of(state),
            ))
        })
    }

    fn branch_switch(
        &self,
        worktree: &GitWorktreeRef,
        name: &str,
    ) -> Result<(GitBranchInfo, GitHeadIdentity), AgentError> {
        self.with_repo_mut(worktree, |state| {
            let tip =
                state.branches.get(name).cloned().ok_or_else(|| {
                    AgentError::not_found(format!("branch `{name}` was not found"))
                })?;
            state.branch = name.into();
            state.head = tip.clone();
            if let Some(tree) = state.commits.get(&tip).map(|commit| commit.tree.clone()) {
                state.worktree_files = tree
                    .into_iter()
                    .map(|(path, content)| (path, FileBlob { content }))
                    .collect();
            }
            state.staged.clear();
            Self::bump(state);
            Ok((
                GitBranchInfo {
                    name: name.into(),
                    tip,
                    current: true,
                    upstream: state.upstream.clone(),
                },
                Self::head_of(state),
            ))
        })
    }

    fn branch_delete(
        &self,
        worktree: &GitWorktreeRef,
        name: &str,
        force: bool,
    ) -> Result<(), AgentError> {
        self.with_repo_mut(worktree, |state| {
            if name == state.branch && !force {
                return Err(AgentError::invalid_input(
                    "cannot delete the current branch without force",
                ));
            }
            if state.branches.remove(name).is_none() {
                return Err(AgentError::not_found(format!(
                    "branch `{name}` was not found"
                )));
            }
            Self::bump(state);
            Ok(())
        })
    }

    fn worktree_list(
        &self,
        repository: &GitRepositoryRef,
    ) -> Result<Vec<GitWorktreeInfo>, AgentError> {
        let repos = self.repos.lock().expect("git mutex");
        let state = repos
            .get(&repository.repo_id)
            .ok_or_else(|| AgentError::not_found("git repository was not found"))?;
        Ok(state
            .worktrees
            .values()
            .map(|worktree| GitWorktreeInfo {
                worktree: worktree.clone(),
                head: Self::head_of(state),
                locked: false,
                prunable: false,
            })
            .collect())
    }

    fn worktree_create(
        &self,
        repository: &GitRepositoryRef,
        path: &str,
        branch: Option<&str>,
    ) -> Result<GitWorktreeInfo, AgentError> {
        let mut repos = self.repos.lock().expect("git mutex");
        let state = repos
            .get_mut(&repository.repo_id)
            .ok_or_else(|| AgentError::not_found("git repository was not found"))?;
        let worktree_id = format!("wt:{path}");
        if state.worktrees.contains_key(&worktree_id) {
            return Err(AgentError::invalid_input("worktree already exists"));
        }
        if let Some(branch) = branch {
            state
                .branches
                .entry(branch.into())
                .or_insert_with(|| state.head.clone());
        }
        let worktree = GitWorktreeRef {
            worktree_id: worktree_id.clone(),
            path: path.into(),
            repository: repository.clone(),
        };
        state.worktrees.insert(worktree_id, worktree.clone());
        Self::bump(state);
        Ok(GitWorktreeInfo {
            worktree,
            head: Self::head_of(state),
            locked: false,
            prunable: false,
        })
    }

    fn worktree_remove(&self, worktree: &GitWorktreeRef, force: bool) -> Result<(), AgentError> {
        self.with_repo_mut(worktree, |state| {
            if state.worktrees.len() == 1 && !force {
                return Err(AgentError::invalid_input(
                    "cannot remove the last worktree without force",
                ));
            }
            state.worktrees.remove(&worktree.worktree_id);
            Self::bump(state);
            Ok(())
        })
    }

    fn log(
        &self,
        worktree: &GitWorktreeRef,
        limit: u32,
        _path: Option<&str>,
    ) -> Result<(Vec<GitCommitInfo>, String), AgentError> {
        self.with_repo_mut(worktree, |state| {
            let mut commits = Vec::new();
            let mut cursor = Some(state.head.clone());
            while let Some(hash) = cursor {
                if commits.len() as u32 >= limit {
                    break;
                }
                let Some(record) = state.commits.get(&hash) else {
                    break;
                };
                commits.push(record.info.clone());
                cursor = record.info.parents.first().cloned();
            }
            let text = commits
                .iter()
                .map(|commit| format!("{} {}", commit.hash, commit.summary))
                .collect::<Vec<_>>()
                .join("\n");
            Ok((commits, text))
        })
    }

    fn show(
        &self,
        worktree: &GitWorktreeRef,
        revision: &str,
    ) -> Result<(GitShowResult, String), AgentError> {
        self.with_repo_mut(worktree, |state| {
            let record = state
                .commits
                .get(revision)
                .ok_or_else(|| AgentError::not_found(format!("revision `{revision}` not found")))?;
            let mut patch = format!("commit {}\n{}\n", record.info.hash, record.info.summary);
            for (path, content) in &record.tree {
                patch.push_str(&format!("file:{path}\n{content}\n"));
            }
            Ok((
                GitShowResult {
                    commit: record.info.clone(),
                    summary: record.info.summary.clone(),
                    patch_ref: None,
                    inline_patch: None,
                    truncated: false,
                },
                patch,
            ))
        })
    }

    fn blame(
        &self,
        worktree: &GitWorktreeRef,
        path: &str,
        max_lines: u32,
    ) -> Result<(GitBlameResult, String), AgentError> {
        self.with_repo_mut(worktree, |state| {
            let content = state
                .worktree_files
                .get(path)
                .map(|blob| blob.content.clone())
                .ok_or_else(|| AgentError::not_found(format!("path `{path}` was not found")))?;
            let lines = content
                .lines()
                .take(max_lines as usize)
                .enumerate()
                .map(|(index, text)| GitBlameLine {
                    line: (index as u32) + 1,
                    commit: state.head.clone(),
                    author: "mutsuki".into(),
                    text: text.into(),
                })
                .collect::<Vec<_>>();
            let text = lines
                .iter()
                .map(|line| format!("{} {} {}", line.commit, line.author, line.text))
                .collect::<Vec<_>>()
                .join("\n");
            Ok((
                GitBlameResult {
                    path: path.into(),
                    lines,
                    details_ref: None,
                    truncated: false,
                },
                text,
            ))
        })
    }

    fn push(
        &self,
        worktree: &GitWorktreeRef,
        remote: &str,
        _refspec: Option<&str>,
        force: bool,
        cancel: &AtomicBool,
    ) -> Result<(String, GitHeadIdentity), AgentError> {
        Self::check_cancel(cancel)?;
        self.with_repo_mut(worktree, |state| {
            let key = format!("{remote}/{}", state.branch);
            if let Some(remote_tip) = state.remote_heads.get(&key)
                && remote_tip != &state.head
                && !force
            {
                return Err(AgentError::new(
                    "agent.git.non_fast_forward",
                    "remote rejected non-fast-forward push",
                ));
            }
            state.remote_heads.insert(key, state.head.clone());
            Self::bump(state);
            Ok((format!("pushed to {remote}"), Self::head_of(state)))
        })
    }

    fn pull(
        &self,
        worktree: &GitWorktreeRef,
        remote: &str,
        _rebase: bool,
        cancel: &AtomicBool,
    ) -> Result<(String, GitHeadIdentity), AgentError> {
        Self::check_cancel(cancel)?;
        self.with_repo_mut(worktree, |state| {
            let key = format!("{remote}/{}", state.branch);
            if let Some(remote_tip) = state.remote_heads.get(&key).cloned() {
                state.head = remote_tip.clone();
                *state.branches.entry(state.branch.clone()).or_default() = remote_tip;
            }
            Self::bump(state);
            Ok((format!("pulled from {remote}"), Self::head_of(state)))
        })
    }

    fn fetch(
        &self,
        worktree: &GitWorktreeRef,
        remote: &str,
        cancel: &AtomicBool,
    ) -> Result<(String, GitHeadIdentity), AgentError> {
        Self::check_cancel(cancel)?;
        self.with_repo_mut(worktree, |state| {
            Self::bump(state);
            Ok((format!("fetched {remote}"), Self::head_of(state)))
        })
    }

    fn merge(
        &self,
        worktree: &GitWorktreeRef,
        revision: &str,
        cancel: &AtomicBool,
    ) -> Result<(String, GitHeadIdentity), AgentError> {
        Self::check_cancel(cancel)?;
        self.with_repo_mut(worktree, |state| {
            if !state.commits.contains_key(revision) && !state.branches.contains_key(revision) {
                return Err(AgentError::not_found(format!(
                    "merge revision `{revision}` not found"
                )));
            }
            let tip = state
                .branches
                .get(revision)
                .cloned()
                .unwrap_or_else(|| revision.into());
            let hash = Self::next_commit_hash(state);
            let tree = state
                .commits
                .get(&tip)
                .map(|commit| commit.tree.clone())
                .unwrap_or_default();
            let info = GitCommitInfo {
                hash: hash.clone(),
                summary: format!("Merge {revision}"),
                author: "mutsuki".into(),
                timestamp_unix_ms: state.generation.saturating_add(2_000),
                parents: vec![state.head.clone(), tip],
            };
            state
                .commits
                .insert(hash.clone(), CommitRecord { info, tree });
            state.head = hash.clone();
            *state.branches.entry(state.branch.clone()).or_default() = hash;
            Self::bump(state);
            Ok((format!("merged {revision}"), Self::head_of(state)))
        })
    }

    fn rebase(
        &self,
        worktree: &GitWorktreeRef,
        onto: &str,
        cancel: &AtomicBool,
    ) -> Result<(String, GitHeadIdentity), AgentError> {
        Self::check_cancel(cancel)?;
        self.with_repo_mut(worktree, |state| {
            let tip = state
                .branches
                .get(onto)
                .cloned()
                .or_else(|| state.commits.contains_key(onto).then(|| onto.into()))
                .ok_or_else(|| AgentError::not_found(format!("rebase onto `{onto}` not found")))?;
            state.head = tip.clone();
            *state.branches.entry(state.branch.clone()).or_default() = tip;
            Self::bump(state);
            Ok((format!("rebased onto {onto}"), Self::head_of(state)))
        })
    }

    fn reset(
        &self,
        worktree: &GitWorktreeRef,
        revision: &str,
        mode: &str,
        cancel: &AtomicBool,
    ) -> Result<(String, GitHeadIdentity), AgentError> {
        Self::check_cancel(cancel)?;
        self.with_repo_mut(worktree, |state| {
            let tip = state
                .branches
                .get(revision)
                .cloned()
                .or_else(|| {
                    state
                        .commits
                        .contains_key(revision)
                        .then(|| revision.into())
                })
                .ok_or_else(|| {
                    AgentError::not_found(format!("reset revision `{revision}` not found"))
                })?;
            state.head = tip.clone();
            *state.branches.entry(state.branch.clone()).or_default() = tip.clone();
            if mode == "hard" {
                if let Some(tree) = state.commits.get(&tip).map(|commit| commit.tree.clone()) {
                    state.worktree_files = tree
                        .into_iter()
                        .map(|(path, content)| (path, FileBlob { content }))
                        .collect();
                }
                state.staged.clear();
            }
            Self::bump(state);
            Ok((format!("reset --{mode} {revision}"), Self::head_of(state)))
        })
    }

    fn cherry_pick(
        &self,
        worktree: &GitWorktreeRef,
        revision: &str,
        cancel: &AtomicBool,
    ) -> Result<(String, GitHeadIdentity), AgentError> {
        Self::check_cancel(cancel)?;
        self.with_repo_mut(worktree, |state| {
            let source = state
                .commits
                .get(revision)
                .ok_or_else(|| {
                    AgentError::not_found(format!("cherry-pick revision `{revision}` not found"))
                })?
                .clone();
            let hash = Self::next_commit_hash(state);
            let mut info = source.info.clone();
            info.hash = hash.clone();
            info.parents = vec![state.head.clone()];
            info.summary = format!("cherry-pick: {}", source.info.summary);
            state.commits.insert(
                hash.clone(),
                CommitRecord {
                    info,
                    tree: source.tree,
                },
            );
            state.head = hash.clone();
            *state.branches.entry(state.branch.clone()).or_default() = hash;
            Self::bump(state);
            Ok((format!("cherry-picked {revision}"), Self::head_of(state)))
        })
    }

    fn head(&self, worktree: &GitWorktreeRef) -> Result<GitHeadIdentity, AgentError> {
        self.with_repo_mut(worktree, |state| Ok(Self::head_of(state)))
    }

    fn cancel_process(&self, _handle_id: &str) -> Result<(), AgentError> {
        Ok(())
    }
}

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

impl GitGateway for CliGitBackend {
    fn discover(&self, path: &str) -> Result<(GitWorktreeRef, GitHeadIdentity), AgentError> {
        let (worktree, head, _) = self.inspect_path(path)?;
        Ok((worktree, head))
    }

    fn status(&self, worktree: &GitWorktreeRef) -> Result<GitStatusSnapshot, AgentError> {
        let cwd = Path::new(&worktree.path);
        let porcelain = self.run(cwd, &["status", "--porcelain=v1"], None, None)?;
        let (_, head, state) = self.inspect_path(&worktree.path)?;
        let mut changes = Vec::new();
        for line in porcelain.lines() {
            if line.len() < 4 {
                continue;
            }
            let x = line.as_bytes()[0] as char;
            let y = line.as_bytes()[1] as char;
            let path = line[3..].to_string();
            if x != ' ' && x != '?' {
                changes.push(GitFileChange {
                    path: path.clone(),
                    old_path: None,
                    status: match x {
                        'A' => GitFileStatus::Added,
                        'D' => GitFileStatus::Deleted,
                        'R' => GitFileStatus::Renamed,
                        'C' => GitFileStatus::Copied,
                        'U' => GitFileStatus::Conflicted,
                        _ => GitFileStatus::Modified,
                    },
                    staged: true,
                    hunks_summary: None,
                    additions: 0,
                    deletions: 0,
                });
            }
            if y != ' ' {
                changes.push(GitFileChange {
                    path,
                    old_path: None,
                    status: if y == '?' {
                        GitFileStatus::Untracked
                    } else if y == 'D' {
                        GitFileStatus::Deleted
                    } else {
                        GitFileStatus::Modified
                    },
                    staged: false,
                    hunks_summary: None,
                    additions: 0,
                    deletions: 0,
                });
            }
        }
        Ok(GitStatusSnapshot {
            worktree: worktree.clone(),
            head,
            state,
            clean: changes.is_empty(),
            changes,
        })
    }

    fn diff(&self, request: &GitDiffRequest) -> Result<(GitDiffResult, String), AgentError> {
        let cwd = Path::new(&request.worktree.path);
        let mut args = vec!["diff".to_string(), "--no-ext-diff".into()];
        match request.scope {
            GitDiffScope::WorkingTree => {}
            GitDiffScope::Staged => args.push("--cached".into()),
            GitDiffScope::Range => {
                if let Some(base) = &request.base {
                    args.push(base.clone());
                }
                if let Some(head) = &request.head {
                    args.push(head.clone());
                }
            }
        }
        if !request.paths.is_empty() {
            args.push("--".into());
            args.extend(request.paths.iter().cloned());
        }
        let arg_refs = args.iter().map(String::as_str).collect::<Vec<_>>();
        let patch = self.run(cwd, &arg_refs, None, None)?;
        let head = self.head(&request.worktree)?;
        Ok((
            GitDiffResult {
                worktree: request.worktree.clone(),
                base: head.clone(),
                head,
                summary: truncate(&patch, SUMMARY_LIMIT),
                files: Vec::new(),
                patch_ref: None,
                inline_patch: None,
                truncated: false,
            },
            patch,
        ))
    }

    fn stage(&self, worktree: &GitWorktreeRef, paths: &[String]) -> Result<(), AgentError> {
        let mut args = vec!["add".to_string(), "--".into()];
        args.extend(paths.iter().cloned());
        let arg_refs = args.iter().map(String::as_str).collect::<Vec<_>>();
        self.run(Path::new(&worktree.path), &arg_refs, None, None)?;
        Ok(())
    }

    fn unstage(&self, worktree: &GitWorktreeRef, paths: &[String]) -> Result<(), AgentError> {
        let mut args = vec!["restore".to_string(), "--staged".into(), "--".into()];
        args.extend(paths.iter().cloned());
        let arg_refs = args.iter().map(String::as_str).collect::<Vec<_>>();
        self.run(Path::new(&worktree.path), &arg_refs, None, None)?;
        Ok(())
    }

    fn commit(
        &self,
        worktree: &GitWorktreeRef,
        message: &str,
        allow_empty: bool,
    ) -> Result<GitCommitResult, AgentError> {
        let mut args = vec!["commit", "-m", message];
        if allow_empty {
            args.push("--allow-empty");
        }
        self.run(Path::new(&worktree.path), &args, None, None)?;
        let head = self.head(worktree)?;
        Ok(GitCommitResult {
            commit: GitCommitInfo {
                hash: head.commit.clone(),
                summary: message.into(),
                author: String::new(),
                timestamp_unix_ms: 0,
                parents: Vec::new(),
            },
            head,
            changed_files: Vec::new(),
        })
    }

    fn branch_list(&self, worktree: &GitWorktreeRef) -> Result<Vec<GitBranchInfo>, AgentError> {
        let output = self.run(
            Path::new(&worktree.path),
            &[
                "for-each-ref",
                "--format=%(refname:short)%09%(objectname)%09%(HEAD)",
                "refs/heads",
            ],
            None,
            None,
        )?;
        Ok(output
            .lines()
            .filter_map(|line| {
                let mut parts = line.split('\t');
                let name = parts.next()?.to_string();
                let tip = parts.next()?.to_string();
                let current = parts.next() == Some("*");
                Some(GitBranchInfo {
                    name,
                    tip,
                    current,
                    upstream: None,
                })
            })
            .collect())
    }

    fn branch_create(
        &self,
        worktree: &GitWorktreeRef,
        name: &str,
        checkout: bool,
    ) -> Result<(GitBranchInfo, GitHeadIdentity), AgentError> {
        if checkout {
            self.run(
                Path::new(&worktree.path),
                &["switch", "-c", name],
                None,
                None,
            )?;
        } else {
            self.run(Path::new(&worktree.path), &["branch", name], None, None)?;
        }
        let head = self.head(worktree)?;
        Ok((
            GitBranchInfo {
                name: name.into(),
                tip: head.commit.clone(),
                current: checkout,
                upstream: None,
            },
            head,
        ))
    }

    fn branch_switch(
        &self,
        worktree: &GitWorktreeRef,
        name: &str,
    ) -> Result<(GitBranchInfo, GitHeadIdentity), AgentError> {
        self.run(Path::new(&worktree.path), &["switch", name], None, None)?;
        let head = self.head(worktree)?;
        Ok((
            GitBranchInfo {
                name: name.into(),
                tip: head.commit.clone(),
                current: true,
                upstream: head.upstream.clone(),
            },
            head,
        ))
    }

    fn branch_delete(
        &self,
        worktree: &GitWorktreeRef,
        name: &str,
        force: bool,
    ) -> Result<(), AgentError> {
        let flag = if force { "-D" } else { "-d" };
        self.run(
            Path::new(&worktree.path),
            &["branch", flag, name],
            None,
            None,
        )?;
        Ok(())
    }

    fn worktree_list(
        &self,
        repository: &GitRepositoryRef,
    ) -> Result<Vec<GitWorktreeInfo>, AgentError> {
        let output = self.run(
            Path::new(&repository.root),
            &["worktree", "list", "--porcelain"],
            None,
            None,
        )?;
        let mut worktrees = Vec::new();
        let mut path = None;
        let mut head = None;
        for line in output.lines() {
            if let Some(value) = line.strip_prefix("worktree ") {
                path = Some(value.to_string());
            } else if let Some(value) = line.strip_prefix("HEAD ") {
                head = Some(value.to_string());
            } else if line.is_empty()
                && let (Some(path), Some(commit)) = (path.take(), head.take())
            {
                worktrees.push(GitWorktreeInfo {
                    worktree: GitWorktreeRef {
                        worktree_id: format!("wt:{path}"),
                        path: path.clone(),
                        repository: repository.clone(),
                    },
                    head: GitHeadIdentity {
                        commit,
                        branch: None,
                        upstream: None,
                        generation: 1,
                    },
                    locked: false,
                    prunable: false,
                });
            }
        }
        Ok(worktrees)
    }

    fn worktree_create(
        &self,
        repository: &GitRepositoryRef,
        path: &str,
        branch: Option<&str>,
    ) -> Result<GitWorktreeInfo, AgentError> {
        let mut args = vec!["worktree".into(), "add".into(), path.into()];
        if let Some(branch) = branch {
            args.push("-b".into());
            args.push(branch.into());
        }
        let arg_refs = args.iter().map(String::as_str).collect::<Vec<_>>();
        self.run(Path::new(&repository.root), &arg_refs, None, None)?;
        let (worktree, head) = self.discover(path)?;
        Ok(GitWorktreeInfo {
            worktree,
            head,
            locked: false,
            prunable: false,
        })
    }

    fn worktree_remove(&self, worktree: &GitWorktreeRef, force: bool) -> Result<(), AgentError> {
        let mut args = vec!["worktree", "remove"];
        if force {
            args.push("--force");
        }
        args.push(&worktree.path);
        self.run(Path::new(&worktree.repository.root), &args, None, None)?;
        Ok(())
    }

    fn log(
        &self,
        worktree: &GitWorktreeRef,
        limit: u32,
        path: Option<&str>,
    ) -> Result<(Vec<GitCommitInfo>, String), AgentError> {
        let limit = limit.to_string();
        let mut args = vec![
            "log".into(),
            format!("-{limit}"),
            "--format=%H%x09%s%x09%an%x09%at".into(),
        ];
        if let Some(path) = path {
            args.push("--".into());
            args.push(path.into());
        }
        let arg_refs = args.iter().map(String::as_str).collect::<Vec<_>>();
        let text = self.run(Path::new(&worktree.path), &arg_refs, None, None)?;
        let commits = text
            .lines()
            .filter_map(|line| {
                let mut parts = line.splitn(4, '\t');
                Some(GitCommitInfo {
                    hash: parts.next()?.into(),
                    summary: parts.next()?.into(),
                    author: parts.next()?.into(),
                    timestamp_unix_ms: parts.next()?.parse::<u64>().ok()?.saturating_mul(1000),
                    parents: Vec::new(),
                })
            })
            .collect();
        Ok((commits, text))
    }

    fn show(
        &self,
        worktree: &GitWorktreeRef,
        revision: &str,
    ) -> Result<(GitShowResult, String), AgentError> {
        let patch = self.run(
            Path::new(&worktree.path),
            &["show", "--no-ext-diff", revision],
            None,
            None,
        )?;
        let summary = truncate(&patch, SUMMARY_LIMIT);
        Ok((
            GitShowResult {
                commit: GitCommitInfo {
                    hash: revision.into(),
                    summary: summary.clone(),
                    author: String::new(),
                    timestamp_unix_ms: 0,
                    parents: Vec::new(),
                },
                summary,
                patch_ref: None,
                inline_patch: None,
                truncated: false,
            },
            patch,
        ))
    }

    fn blame(
        &self,
        worktree: &GitWorktreeRef,
        path: &str,
        max_lines: u32,
    ) -> Result<(GitBlameResult, String), AgentError> {
        let text = self.run(
            Path::new(&worktree.path),
            &["blame", "--line-porcelain", path],
            None,
            None,
        )?;
        let mut lines = Vec::new();
        let mut commit = String::new();
        let mut author = String::new();
        for line in text.lines() {
            if line.len() >= 40 && line.as_bytes().iter().take(40).all(u8::is_ascii_hexdigit) {
                commit = line[..40].to_string();
            } else if let Some(value) = line.strip_prefix("author ") {
                author = value.into();
            } else if let Some(value) = line.strip_prefix("\t") {
                lines.push(GitBlameLine {
                    line: (lines.len() as u32) + 1,
                    commit: commit.clone(),
                    author: author.clone(),
                    text: value.into(),
                });
                if lines.len() as u32 >= max_lines {
                    break;
                }
            }
        }
        Ok((
            GitBlameResult {
                path: path.into(),
                lines,
                details_ref: None,
                truncated: false,
            },
            text,
        ))
    }

    fn push(
        &self,
        worktree: &GitWorktreeRef,
        remote: &str,
        refspec: Option<&str>,
        force: bool,
        cancel: &AtomicBool,
    ) -> Result<(String, GitHeadIdentity), AgentError> {
        let mut args = vec!["push".to_string(), remote.into()];
        if force {
            args.push("--force".into());
        }
        if let Some(refspec) = refspec {
            args.push(refspec.into());
        }
        let arg_refs = args.iter().map(String::as_str).collect::<Vec<_>>();
        let summary = self.run(
            Path::new(&worktree.path),
            &arg_refs,
            Some(cancel),
            Some("push"),
        )?;
        Ok((summary, self.head(worktree)?))
    }

    fn pull(
        &self,
        worktree: &GitWorktreeRef,
        remote: &str,
        rebase: bool,
        cancel: &AtomicBool,
    ) -> Result<(String, GitHeadIdentity), AgentError> {
        let mut args = vec!["pull", remote];
        if rebase {
            args.push("--rebase");
        }
        let summary = self.run(Path::new(&worktree.path), &args, Some(cancel), Some("pull"))?;
        Ok((summary, self.head(worktree)?))
    }

    fn fetch(
        &self,
        worktree: &GitWorktreeRef,
        remote: &str,
        cancel: &AtomicBool,
    ) -> Result<(String, GitHeadIdentity), AgentError> {
        let summary = self.run(
            Path::new(&worktree.path),
            &["fetch", remote],
            Some(cancel),
            Some("fetch"),
        )?;
        Ok((summary, self.head(worktree)?))
    }

    fn merge(
        &self,
        worktree: &GitWorktreeRef,
        revision: &str,
        cancel: &AtomicBool,
    ) -> Result<(String, GitHeadIdentity), AgentError> {
        let summary = self.run(
            Path::new(&worktree.path),
            &["merge", revision],
            Some(cancel),
            Some("merge"),
        )?;
        Ok((summary, self.head(worktree)?))
    }

    fn rebase(
        &self,
        worktree: &GitWorktreeRef,
        onto: &str,
        cancel: &AtomicBool,
    ) -> Result<(String, GitHeadIdentity), AgentError> {
        let summary = self.run(
            Path::new(&worktree.path),
            &["rebase", onto],
            Some(cancel),
            Some("rebase"),
        )?;
        Ok((summary, self.head(worktree)?))
    }

    fn reset(
        &self,
        worktree: &GitWorktreeRef,
        revision: &str,
        mode: &str,
        cancel: &AtomicBool,
    ) -> Result<(String, GitHeadIdentity), AgentError> {
        let mode_flag = format!("--{mode}");
        let summary = self.run(
            Path::new(&worktree.path),
            &["reset", &mode_flag, revision],
            Some(cancel),
            Some("reset"),
        )?;
        Ok((summary, self.head(worktree)?))
    }

    fn cherry_pick(
        &self,
        worktree: &GitWorktreeRef,
        revision: &str,
        cancel: &AtomicBool,
    ) -> Result<(String, GitHeadIdentity), AgentError> {
        let summary = self.run(
            Path::new(&worktree.path),
            &["cherry-pick", revision],
            Some(cancel),
            Some("cherry-pick"),
        )?;
        Ok((summary, self.head(worktree)?))
    }

    fn head(&self, worktree: &GitWorktreeRef) -> Result<GitHeadIdentity, AgentError> {
        let (_, head, _) = self.inspect_path(&worktree.path)?;
        Ok(head)
    }

    fn cancel_process(&self, handle_id: &str) -> Result<(), AgentError> {
        if let Some(mut child) = self
            .children
            .lock()
            .expect("git children")
            .remove(handle_id)
        {
            let _ = child.kill();
            let _ = child.wait();
        }
        Ok(())
    }
}

struct ActiveOp {
    handle: GitOperationHandle,
    cancel: Arc<AtomicBool>,
}

/// Shared GitService consumed by Agent tools and product Git UI.
pub struct SharedGitService {
    descriptor: AgentServiceDescriptor,
    backend: Arc<dyn GitGateway>,
    resources: AgentResourceStore,
    next_action: AtomicU64,
    active: Mutex<BTreeMap<String, ActiveOp>>,
    write_fences: Mutex<BTreeMap<String, Arc<Mutex<()>>>>,
}

impl SharedGitService {
    pub fn new(backend: Arc<dyn GitGateway>, resources: AgentResourceStore) -> Self {
        Self {
            descriptor: AgentServiceDescriptor {
                service_id: SERVICE_ID.into(),
                version: "1".into(),
                request_schema: "mutsuki.agent.git.request@1".into(),
                response_schema: "mutsuki.agent.git.response@1".into(),
                state: AgentPluginStateKind::Stateful,
                affinity: Some("worktree".into()),
            },
            backend,
            resources,
            next_action: AtomicU64::new(1),
            active: Mutex::new(BTreeMap::new()),
            write_fences: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn in_memory(backend: InMemoryGitBackend) -> Self {
        Self::new(Arc::new(backend), AgentResourceStore::default())
    }

    pub fn plugin_descriptor(
        generation: u64,
    ) -> Result<mutsuki_agent_contracts::AgentKitPluginDescriptor, AgentError> {
        let mut registrar = AgentPluginRegistrar::new(PLUGIN_ID, generation)
            .service(AgentServiceDescriptor {
                service_id: SERVICE_ID.into(),
                version: "1".into(),
                request_schema: "mutsuki.agent.git.request@1".into(),
                response_schema: "mutsuki.agent.git.response@1".into(),
                state: AgentPluginStateKind::Stateful,
                affinity: Some("worktree".into()),
            })
            .context_provider(CONTEXT_PROVIDER_ID)
            .require_capability("workspace.process")
            .require_service(SERVICE_ID);
        for (name, risk) in [
            ("git.status", GitRisk::ReadOnly),
            ("git.diff", GitRisk::ReadOnly),
            ("git.log", GitRisk::ReadOnly),
            ("git.show", GitRisk::ReadOnly),
            ("git.blame", GitRisk::ReadOnly),
            ("git.branch.list", GitRisk::ReadOnly),
            ("git.worktree.list", GitRisk::ReadOnly),
            ("git.stage", GitRisk::ReversibleWrite),
            ("git.unstage", GitRisk::ReversibleWrite),
            ("git.commit", GitRisk::ReversibleWrite),
            ("git.branch.create", GitRisk::ReversibleWrite),
            ("git.branch.switch", GitRisk::ReversibleWrite),
            ("git.push", GitRisk::HighRisk),
            ("git.pull", GitRisk::HighRisk),
            ("git.fetch", GitRisk::HighRisk),
            ("git.merge", GitRisk::HighRisk),
            ("git.rebase", GitRisk::HighRisk),
            ("git.reset", GitRisk::HighRisk),
            ("git.cherry_pick", GitRisk::HighRisk),
            ("git.branch.delete", GitRisk::HighRisk),
            ("git.worktree.create", GitRisk::HighRisk),
            ("git.worktree.remove", GitRisk::HighRisk),
        ] {
            let mut tool = AgentToolDescriptor::new(
                name,
                format!("mutsuki.agent.tool.{name}@1"),
                format!("Run {name}"),
            );
            tool.side_effect = risk.side_effect();
            tool.requires_approval = risk.requires_approval();
            tool.permissions = match risk {
                GitRisk::ReadOnly => vec!["git.read".into()],
                GitRisk::ReversibleWrite => vec!["git.write".into()],
                GitRisk::HighRisk => vec!["git.high_risk".into()],
            };
            registrar = registrar.tool(tool);
        }
        registrar.build()
    }

    pub fn active_handle_count(&self) -> usize {
        self.active.lock().expect("git active").len()
    }

    pub fn call_value(&self, request: Value) -> Result<Value, AgentError> {
        AgentService::call(self, request)
    }

    fn plan(
        &self,
        operation: &str,
        risk: GitRisk,
        summary: impl Into<String>,
        snapshot: &GitStatusSnapshot,
        write: &GitWriteContext,
        preview: Option<Value>,
    ) -> GitActionPlan {
        let action_id = format!("git-{}", self.next_action.fetch_add(1, Ordering::Relaxed));
        GitActionPlan {
            action_id: action_id.clone(),
            operation: operation.into(),
            risk,
            summary: summary.into(),
            approval: PermissionRequest {
                session_id: write.session_id.clone(),
                turn_id: write.turn_id.clone(),
                action_id,
                tool: operation.into(),
                side_effect: risk.side_effect(),
                summary: format!("{operation} requires approval"),
                version: write.approval_version.unwrap_or(1),
            },
            worktree: snapshot.worktree.clone(),
            head: snapshot.head.clone(),
            state: snapshot.state.clone(),
            preview,
        }
    }

    fn ensure_approved(
        &self,
        risk: GitRisk,
        operation: &str,
        worktree: &GitWorktreeRef,
        write: &GitWriteContext,
        preview: Option<Value>,
    ) -> Result<Option<GitServiceResponse>, AgentError> {
        let snapshot = self.backend.status(worktree)?;
        let head = &snapshot.head;
        let state = &snapshot.state;
        if let Some(expected) = &write.expected_state
            && expected != state
        {
            return Ok(Some(GitServiceResponse::Conflict(
                GitRevisionConflict::stale_state(
                    worktree.clone(),
                    expected.clone(),
                    state.clone(),
                    head.clone(),
                ),
            )));
        }
        if write.expected_state.is_none()
            && let Some(expected) = &write.expected_head
            && (expected.commit != head.commit || expected.generation != head.generation)
        {
            return Ok(Some(GitServiceResponse::Conflict(
                GitRevisionConflict::stale(worktree.clone(), expected.clone(), head.clone()),
            )));
        }
        if risk.requires_approval() && !write.approved {
            return Ok(Some(GitServiceResponse::Plan(self.plan(
                operation,
                risk,
                format!("{operation} is waiting for approval"),
                &snapshot,
                write,
                preview,
            ))));
        }
        Ok(None)
    }

    fn write_fence(&self, key: String) -> Arc<Mutex<()>> {
        self.write_fences
            .lock()
            .expect("git write fences")
            .entry(key)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    fn begin_op(
        &self,
        kind: GitOperationKind,
        risk: GitRisk,
        worktree: Option<GitWorktreeRef>,
        summary: impl Into<String>,
    ) -> (String, Arc<AtomicBool>) {
        let handle_id = format!(
            "git-op-{}",
            self.next_action.fetch_add(1, Ordering::Relaxed)
        );
        let cancel = Arc::new(AtomicBool::new(false));
        let handle = GitOperationHandle {
            handle_id: handle_id.clone(),
            kind,
            state: GitOperationState::Running,
            risk,
            worktree,
            summary: summary.into(),
        };
        self.active.lock().expect("git active").insert(
            handle_id.clone(),
            ActiveOp {
                handle,
                cancel: cancel.clone(),
            },
        );
        (handle_id, cancel)
    }

    fn finish_op(&self, handle_id: &str, state: GitOperationState) {
        if let Some(mut op) = self.active.lock().expect("git active").remove(handle_id) {
            op.handle.state = state;
        }
    }

    fn store_text(
        &self,
        kind: &str,
        schema: &str,
        text: &str,
    ) -> Result<(Option<ResourceRef>, Option<String>, bool), AgentError> {
        if text.len() <= INLINE_LIMIT {
            return Ok((None, Some(text.to_string()), false));
        }
        let reference =
            self.resources
                .put_json(SERVICE_ID, kind, schema, 1, &json!({ "text": text }))?;
        Ok((Some(reference), None, true))
    }

    fn with_timeout<T>(
        &self,
        write: &GitWriteContext,
        cancel: &AtomicBool,
        op: impl FnOnce() -> Result<T, AgentError>,
    ) -> Result<T, AgentError> {
        let started = Instant::now();
        let result = op();
        if cancel.load(Ordering::Relaxed) {
            return Err(AgentError::new(
                "agent.git.cancelled",
                "git operation was cancelled",
            ));
        }
        if started.elapsed() > Duration::from_millis(write.timeout_ms.max(1)) {
            return Err(AgentError::new(
                "agent.git.timeout",
                "git operation exceeded timeout",
            ));
        }
        result
    }

    fn dispatch(&self, request: GitServiceRequest) -> Result<GitServiceResponse, AgentError> {
        let Some(key) = write_affinity(&request) else {
            return self.dispatch_inner(request);
        };
        let fence = self.write_fence(key);
        let _guard = fence.lock().map_err(|_| {
            AgentError::new("agent.git.state_poisoned", "git write fence lock poisoned")
        })?;
        self.dispatch_inner(request)
    }

    fn dispatch_inner(&self, request: GitServiceRequest) -> Result<GitServiceResponse, AgentError> {
        match request {
            GitServiceRequest::Discover { path } => {
                let (worktree, head) = self.backend.discover(&path)?;
                Ok(GitServiceResponse::Discovered { worktree, head })
            }
            GitServiceRequest::Status { worktree } => {
                Ok(GitServiceResponse::Status(self.backend.status(&worktree)?))
            }
            GitServiceRequest::Diff { request } => {
                let (mut result, patch) = self.backend.diff(&request)?;
                let (patch_ref, inline_patch, truncated) =
                    self.store_text("mutsuki.agent.git.diff", "mutsuki.agent.git.diff@1", &patch)?;
                result.patch_ref = patch_ref;
                result.inline_patch = inline_patch;
                result.truncated = truncated;
                if truncated {
                    result.summary = truncate(&patch, SUMMARY_LIMIT);
                }
                Ok(GitServiceResponse::Diff(result))
            }
            GitServiceRequest::Stage {
                worktree,
                paths,
                write,
            } => {
                if let Some(denied) = self.ensure_approved(
                    GitRisk::ReversibleWrite,
                    "git.stage",
                    &worktree,
                    &write,
                    Some(json!({ "paths": paths })),
                )? {
                    return Ok(denied);
                }
                let (handle_id, cancel) = self.begin_op(
                    GitOperationKind::Stage,
                    GitRisk::ReversibleWrite,
                    Some(worktree.clone()),
                    "stage",
                );
                let result =
                    self.with_timeout(&write, &cancel, || self.backend.stage(&worktree, &paths));
                self.finish_op(
                    &handle_id,
                    if result.is_ok() {
                        GitOperationState::Succeeded
                    } else {
                        GitOperationState::Failed
                    },
                );
                result?;
                Ok(GitServiceResponse::Staged {
                    paths,
                    head: self.backend.head(&worktree)?,
                })
            }
            GitServiceRequest::Unstage {
                worktree,
                paths,
                write,
            } => {
                if let Some(denied) = self.ensure_approved(
                    GitRisk::ReversibleWrite,
                    "git.unstage",
                    &worktree,
                    &write,
                    Some(json!({ "paths": paths })),
                )? {
                    return Ok(denied);
                }
                self.backend.unstage(&worktree, &paths)?;
                Ok(GitServiceResponse::Unstaged {
                    paths,
                    head: self.backend.head(&worktree)?,
                })
            }
            GitServiceRequest::Commit {
                worktree,
                message,
                allow_empty,
                write,
            } => {
                if let Some(denied) = self.ensure_approved(
                    GitRisk::ReversibleWrite,
                    "git.commit",
                    &worktree,
                    &write,
                    Some(json!({ "message": message })),
                )? {
                    return Ok(denied);
                }
                Ok(GitServiceResponse::Committed(self.backend.commit(
                    &worktree,
                    &message,
                    allow_empty,
                )?))
            }
            GitServiceRequest::BranchList { worktree } => Ok(GitServiceResponse::Branches {
                branches: self.backend.branch_list(&worktree)?,
            }),
            GitServiceRequest::BranchCreate {
                worktree,
                name,
                checkout,
                write,
            } => {
                if let Some(denied) = self.ensure_approved(
                    GitRisk::ReversibleWrite,
                    "git.branch.create",
                    &worktree,
                    &write,
                    Some(json!({ "name": name, "checkout": checkout })),
                )? {
                    return Ok(denied);
                }
                let (branch, head) = self.backend.branch_create(&worktree, &name, checkout)?;
                Ok(GitServiceResponse::Branch { branch, head })
            }
            GitServiceRequest::BranchSwitch {
                worktree,
                name,
                write,
            } => {
                if let Some(denied) = self.ensure_approved(
                    GitRisk::ReversibleWrite,
                    "git.branch.switch",
                    &worktree,
                    &write,
                    Some(json!({ "name": name })),
                )? {
                    return Ok(denied);
                }
                let (branch, head) = self.backend.branch_switch(&worktree, &name)?;
                Ok(GitServiceResponse::Branch { branch, head })
            }
            GitServiceRequest::BranchDelete {
                worktree,
                name,
                force,
                write,
            } => {
                if let Some(denied) = self.ensure_approved(
                    GitRisk::HighRisk,
                    "git.branch.delete",
                    &worktree,
                    &write,
                    Some(json!({ "name": name, "force": force })),
                )? {
                    return Ok(denied);
                }
                self.backend.branch_delete(&worktree, &name, force)?;
                Ok(GitServiceResponse::Ack)
            }
            GitServiceRequest::WorktreeList { repository } => Ok(GitServiceResponse::Worktrees {
                worktrees: self.backend.worktree_list(&repository)?,
            }),
            GitServiceRequest::WorktreeCreate {
                repository,
                path,
                branch,
                write,
            } => {
                let anchor = GitWorktreeRef {
                    worktree_id: format!("wt:{}", repository.root),
                    path: repository.root.clone(),
                    repository: repository.clone(),
                };
                if let Some(denied) = self.ensure_approved(
                    GitRisk::HighRisk,
                    "git.worktree.create",
                    &anchor,
                    &write,
                    Some(json!({ "path": path, "branch": branch })),
                )? {
                    return Ok(denied);
                }
                Ok(GitServiceResponse::Worktree {
                    worktree: self.backend.worktree_create(
                        &repository,
                        &path,
                        branch.as_deref(),
                    )?,
                })
            }
            GitServiceRequest::WorktreeRemove {
                worktree,
                force,
                write,
            } => {
                if let Some(denied) = self.ensure_approved(
                    GitRisk::HighRisk,
                    "git.worktree.remove",
                    &worktree,
                    &write,
                    Some(json!({ "force": force })),
                )? {
                    return Ok(denied);
                }
                self.backend.worktree_remove(&worktree, force)?;
                Ok(GitServiceResponse::Ack)
            }
            GitServiceRequest::Log {
                worktree,
                limit,
                path,
            } => {
                let (commits, text) = self.backend.log(&worktree, limit, path.as_deref())?;
                let (details_ref, _, truncated) =
                    self.store_text("mutsuki.agent.git.log", "mutsuki.agent.git.log@1", &text)?;
                Ok(GitServiceResponse::Log(GitLogResult {
                    commits,
                    details_ref,
                    truncated,
                }))
            }
            GitServiceRequest::Show { worktree, revision } => {
                let (mut result, patch) = self.backend.show(&worktree, &revision)?;
                let (patch_ref, inline_patch, truncated) =
                    self.store_text("mutsuki.agent.git.show", "mutsuki.agent.git.show@1", &patch)?;
                result.patch_ref = patch_ref;
                result.inline_patch = inline_patch;
                result.truncated = truncated;
                Ok(GitServiceResponse::Show(result))
            }
            GitServiceRequest::Blame {
                worktree,
                path,
                max_lines,
            } => {
                let (mut result, text) = self.backend.blame(&worktree, &path, max_lines)?;
                let (details_ref, _, truncated) = self.store_text(
                    "mutsuki.agent.git.blame",
                    "mutsuki.agent.git.blame@1",
                    &text,
                )?;
                result.details_ref = details_ref;
                result.truncated = truncated;
                Ok(GitServiceResponse::Blame(result))
            }
            GitServiceRequest::Push {
                worktree,
                remote,
                refspec,
                force,
                write,
            } => {
                let remote = if remote.is_empty() {
                    "origin".into()
                } else {
                    remote
                };
                if let Some(denied) = self.ensure_approved(
                    GitRisk::HighRisk,
                    "git.push",
                    &worktree,
                    &write,
                    Some(json!({ "remote": remote, "force": force, "refspec": refspec })),
                )? {
                    return Ok(denied);
                }
                let (handle_id, cancel) = self.begin_op(
                    GitOperationKind::Push,
                    GitRisk::HighRisk,
                    Some(worktree.clone()),
                    "push",
                );
                let result = self.with_timeout(&write, &cancel, || {
                    self.backend
                        .push(&worktree, &remote, refspec.as_deref(), force, &cancel)
                });
                self.finish_op(
                    &handle_id,
                    if result.is_ok() {
                        GitOperationState::Succeeded
                    } else {
                        GitOperationState::Failed
                    },
                );
                let (summary, head) = result?;
                Ok(GitServiceResponse::RemoteOk { summary, head })
            }
            GitServiceRequest::Pull {
                worktree,
                remote,
                rebase,
                write,
            } => {
                let remote = if remote.is_empty() {
                    "origin".into()
                } else {
                    remote
                };
                if let Some(denied) = self.ensure_approved(
                    GitRisk::HighRisk,
                    "git.pull",
                    &worktree,
                    &write,
                    Some(json!({ "remote": remote, "rebase": rebase })),
                )? {
                    return Ok(denied);
                }
                let (handle_id, cancel) = self.begin_op(
                    GitOperationKind::Pull,
                    GitRisk::HighRisk,
                    Some(worktree.clone()),
                    "pull",
                );
                let result = self.with_timeout(&write, &cancel, || {
                    self.backend.pull(&worktree, &remote, rebase, &cancel)
                });
                self.finish_op(
                    &handle_id,
                    if result.is_ok() {
                        GitOperationState::Succeeded
                    } else {
                        GitOperationState::Failed
                    },
                );
                let (summary, head) = result?;
                Ok(GitServiceResponse::RemoteOk { summary, head })
            }
            GitServiceRequest::Fetch {
                worktree,
                remote,
                write,
            } => {
                let remote = if remote.is_empty() {
                    "origin".into()
                } else {
                    remote
                };
                if let Some(denied) = self.ensure_approved(
                    GitRisk::HighRisk,
                    "git.fetch",
                    &worktree,
                    &write,
                    Some(json!({ "remote": remote })),
                )? {
                    return Ok(denied);
                }
                let (handle_id, cancel) = self.begin_op(
                    GitOperationKind::Fetch,
                    GitRisk::HighRisk,
                    Some(worktree.clone()),
                    "fetch",
                );
                let result = self.with_timeout(&write, &cancel, || {
                    self.backend.fetch(&worktree, &remote, &cancel)
                });
                self.finish_op(
                    &handle_id,
                    if result.is_ok() {
                        GitOperationState::Succeeded
                    } else {
                        GitOperationState::Failed
                    },
                );
                let (summary, head) = result?;
                Ok(GitServiceResponse::RemoteOk { summary, head })
            }
            GitServiceRequest::Merge {
                worktree,
                revision,
                write,
            } => {
                if let Some(denied) = self.ensure_approved(
                    GitRisk::HighRisk,
                    "git.merge",
                    &worktree,
                    &write,
                    Some(json!({ "revision": revision })),
                )? {
                    return Ok(denied);
                }
                let (handle_id, cancel) = self.begin_op(
                    GitOperationKind::Merge,
                    GitRisk::HighRisk,
                    Some(worktree.clone()),
                    "merge",
                );
                let result = self.with_timeout(&write, &cancel, || {
                    self.backend.merge(&worktree, &revision, &cancel)
                });
                self.finish_op(
                    &handle_id,
                    if result.is_ok() {
                        GitOperationState::Succeeded
                    } else {
                        GitOperationState::Failed
                    },
                );
                let (summary, head) = result?;
                Ok(GitServiceResponse::Merged { summary, head })
            }
            GitServiceRequest::Rebase {
                worktree,
                onto,
                write,
            } => {
                if let Some(denied) = self.ensure_approved(
                    GitRisk::HighRisk,
                    "git.rebase",
                    &worktree,
                    &write,
                    Some(json!({ "onto": onto })),
                )? {
                    return Ok(denied);
                }
                let (handle_id, cancel) = self.begin_op(
                    GitOperationKind::Rebase,
                    GitRisk::HighRisk,
                    Some(worktree.clone()),
                    "rebase",
                );
                let result = self.with_timeout(&write, &cancel, || {
                    self.backend.rebase(&worktree, &onto, &cancel)
                });
                self.finish_op(
                    &handle_id,
                    if result.is_ok() {
                        GitOperationState::Succeeded
                    } else {
                        GitOperationState::Failed
                    },
                );
                let (summary, head) = result?;
                Ok(GitServiceResponse::Merged { summary, head })
            }
            GitServiceRequest::Reset {
                worktree,
                revision,
                mode,
                write,
            } => {
                let mode = if mode.is_empty() {
                    "mixed".into()
                } else {
                    mode
                };
                if let Some(denied) = self.ensure_approved(
                    GitRisk::HighRisk,
                    "git.reset",
                    &worktree,
                    &write,
                    Some(json!({ "revision": revision, "mode": mode })),
                )? {
                    return Ok(denied);
                }
                let (handle_id, cancel) = self.begin_op(
                    GitOperationKind::Reset,
                    GitRisk::HighRisk,
                    Some(worktree.clone()),
                    "reset",
                );
                let result = self.with_timeout(&write, &cancel, || {
                    self.backend.reset(&worktree, &revision, &mode, &cancel)
                });
                self.finish_op(
                    &handle_id,
                    if result.is_ok() {
                        GitOperationState::Succeeded
                    } else {
                        GitOperationState::Failed
                    },
                );
                let (summary, head) = result?;
                Ok(GitServiceResponse::Merged { summary, head })
            }
            GitServiceRequest::CherryPick {
                worktree,
                revision,
                write,
            } => {
                if let Some(denied) = self.ensure_approved(
                    GitRisk::HighRisk,
                    "git.cherry_pick",
                    &worktree,
                    &write,
                    Some(json!({ "revision": revision })),
                )? {
                    return Ok(denied);
                }
                let (handle_id, cancel) = self.begin_op(
                    GitOperationKind::CherryPick,
                    GitRisk::HighRisk,
                    Some(worktree.clone()),
                    "cherry-pick",
                );
                let result = self.with_timeout(&write, &cancel, || {
                    self.backend.cherry_pick(&worktree, &revision, &cancel)
                });
                self.finish_op(
                    &handle_id,
                    if result.is_ok() {
                        GitOperationState::Succeeded
                    } else {
                        GitOperationState::Failed
                    },
                );
                let (summary, head) = result?;
                Ok(GitServiceResponse::Merged { summary, head })
            }
            GitServiceRequest::OperationState { handle_id } => {
                let active = self.active.lock().expect("git active");
                let op = active
                    .get(&handle_id)
                    .ok_or_else(|| AgentError::not_found("git operation handle was not found"))?;
                Ok(GitServiceResponse::Operation(op.handle.clone()))
            }
            GitServiceRequest::Cancel { handle_id } => {
                if let Some(op) = self.active.lock().expect("git active").get(&handle_id) {
                    op.cancel.store(true, Ordering::Relaxed);
                }
                self.backend.cancel_process(&handle_id)?;
                self.finish_op(&handle_id, GitOperationState::Cancelled);
                Ok(GitServiceResponse::Ack)
            }
        }
    }
}

fn write_affinity(request: &GitServiceRequest) -> Option<String> {
    match request {
        GitServiceRequest::Stage { worktree, .. }
        | GitServiceRequest::Unstage { worktree, .. }
        | GitServiceRequest::Commit { worktree, .. }
        | GitServiceRequest::BranchCreate { worktree, .. }
        | GitServiceRequest::BranchSwitch { worktree, .. }
        | GitServiceRequest::BranchDelete { worktree, .. }
        | GitServiceRequest::WorktreeRemove { worktree, .. }
        | GitServiceRequest::Push { worktree, .. }
        | GitServiceRequest::Pull { worktree, .. }
        | GitServiceRequest::Fetch { worktree, .. }
        | GitServiceRequest::Merge { worktree, .. }
        | GitServiceRequest::Rebase { worktree, .. }
        | GitServiceRequest::Reset { worktree, .. }
        | GitServiceRequest::CherryPick { worktree, .. } => Some(worktree.worktree_id.clone()),
        GitServiceRequest::WorktreeCreate { repository, .. } => {
            Some(format!("repository:{}", repository.repo_id))
        }
        _ => None,
    }
}

fn truncate(input: &str, limit: usize) -> String {
    if input.chars().count() <= limit {
        return input.to_string();
    }
    input.chars().take(limit).collect::<String>() + "…"
}

impl AgentService for SharedGitService {
    fn descriptor(&self) -> &AgentServiceDescriptor {
        &self.descriptor
    }

    fn call(&self, request: Value) -> Result<Value, AgentError> {
        let request: GitServiceRequest = serde_json::from_value(request)
            .map_err(|error| AgentError::invalid_input(error.to_string()))?;
        let response = self.dispatch(request)?;
        serde_json::to_value(response).map_err(|error| AgentError::invalid_input(error.to_string()))
    }

    fn drain(&self) -> Result<(), AgentError> {
        Ok(())
    }

    fn dispose(&self) -> Result<(), AgentError> {
        let handles = self
            .active
            .lock()
            .expect("git active")
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for handle_id in handles {
            if let Some(op) = self.active.lock().expect("git active").get(&handle_id) {
                op.cancel.store(true, Ordering::Relaxed);
            }
            self.backend.cancel_process(&handle_id)?;
            self.finish_op(&handle_id, GitOperationState::Cancelled);
        }
        Ok(())
    }
}

impl ToolProvider for SharedGitService {
    fn tools(&self) -> Vec<AgentToolDescriptor> {
        SharedGitService::plugin_descriptor(1)
            .map(|descriptor| descriptor.tools)
            .unwrap_or_default()
    }
}

impl ContextProvider for SharedGitService {
    fn provider_id(&self) -> &str {
        CONTEXT_PROVIDER_ID
    }

    fn collect(
        &self,
        request: mutsuki_agent_contracts::ContextProviderRequest,
    ) -> Result<mutsuki_agent_contracts::ContextProviderResult, AgentError> {
        let path = request
            .input
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or("");
        if path.is_empty() {
            return Ok(mutsuki_agent_contracts::ContextProviderResult {
                provider_id: CONTEXT_PROVIDER_ID.into(),
                summary: format!("git ready active_handles={}", self.active_handle_count()),
                details: None,
                estimated_tokens: 8,
                estimated_bytes: 64,
                priority: 0,
                required: false,
            });
        }
        let (worktree, head) = self.backend.discover(path)?;
        let status = self.backend.status(&worktree)?;
        let details = self.resources.put_json(
            SERVICE_ID,
            "mutsuki.agent.git.context",
            "mutsuki.agent.git.context@1",
            1,
            &json!({
                "worktree": worktree,
                "head": status.head,
                "clean": status.clean,
                "changes": status.changes.len(),
            }),
        )?;
        Ok(mutsuki_agent_contracts::ContextProviderResult {
            provider_id: CONTEXT_PROVIDER_ID.into(),
            summary: format!(
                "{} @ {} ({} change(s))",
                head.branch.unwrap_or_else(|| "DETACHED".into()),
                head.commit,
                status.changes.len()
            ),
            details: Some(details),
            estimated_tokens: 64,
            estimated_bytes: 256,
            priority: 10,
            required: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn git(cwd: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .expect("git command should start");
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn seeded() -> (SharedGitService, GitWorktreeRef) {
        let mut files = BTreeMap::new();
        files.insert("README.md".into(), "hello\n".into());
        files.insert("src/main.rs".into(), "fn main() {}\n".into());
        let backend = InMemoryGitBackend::default().seed_repo("/workspace", files);
        let service = SharedGitService::in_memory(backend);
        let discovered = service
            .call_value(json!({ "op": "discover", "path": "/workspace" }))
            .unwrap();
        let worktree: GitWorktreeRef =
            serde_json::from_value(discovered["worktree"].clone()).unwrap();
        (service, worktree)
    }

    fn write_ctx(approved: bool) -> Value {
        json!({
            "session_id": "s1",
            "turn_id": "t1",
            "approved": approved,
            "approval_version": 2,
            "timeout_ms": 5_000
        })
    }

    #[test]
    fn status_diff_stage_unstage_commit_branch_worktree_e2e() {
        let (service, worktree) = seeded();
        let agent = Arc::new(service);
        let ui = agent.clone();

        let status = agent
            .call_value(json!({ "op": "status", "worktree": worktree }))
            .unwrap();
        assert_eq!(status["kind"], "status");
        assert_eq!(status["clean"], true);

        // UI mutates working tree through the shared service backend.
        ui.backend.stage(&worktree, &["README.md".into()]).unwrap();
        // Direct stage via shared API after unstage path.
        let _ = ui
            .call_value(json!({
                "op": "unstage",
                "worktree": worktree,
                "paths": ["README.md"],
                "session_id": "s1",
                "turn_id": "t1",
                "approved": true
            }))
            .unwrap();

        // Modify file via backend commit path: stage + commit.
        agent
            .call_value(json!({
                "op": "stage",
                "worktree": worktree,
                "paths": ["README.md"],
                "session_id": "s1",
                "turn_id": "t1",
                "approved": true
            }))
            .unwrap();
        let committed = agent
            .call_value(json!({
                "op": "commit",
                "worktree": worktree,
                "message": "touch readme",
                "allow_empty": true,
                "session_id": "s1",
                "turn_id": "t1",
                "approved": true
            }))
            .unwrap();
        assert_eq!(committed["kind"], "committed");

        let branch = agent
            .call_value(json!({
                "op": "branch_create",
                "worktree": worktree,
                "name": "feature",
                "checkout": true,
                "session_id": "s1",
                "turn_id": "t1",
                "approved": true
            }))
            .unwrap();
        assert_eq!(branch["kind"], "branch");
        assert_eq!(branch["branch"]["name"], "feature");

        let listed = ui
            .call_value(json!({
                "op": "branch_list",
                "worktree": worktree
            }))
            .unwrap();
        assert!(
            listed["branches"]
                .as_array()
                .unwrap()
                .iter()
                .any(|branch| branch["name"] == "feature")
        );

        let worktrees = ui
            .call_value(json!({
                "op": "worktree_list",
                "repository": worktree.repository
            }))
            .unwrap();
        assert_eq!(worktrees["kind"], "worktrees");
        assert!(!worktrees["worktrees"].as_array().unwrap().is_empty());

        let created = agent
            .call_value(json!({
                "op": "worktree_create",
                "repository": worktree.repository,
                "path": "/workspace-feature",
                "branch": "wt-feature",
                "session_id": "s1",
                "turn_id": "t1",
                "approved": true
            }))
            .unwrap();
        assert_eq!(created["kind"], "worktree");
    }

    #[test]
    fn large_diff_uses_resource_ref() {
        let mut files = BTreeMap::new();
        files.insert("big.txt".into(), "x".repeat(INLINE_LIMIT + 64));
        let backend = InMemoryGitBackend::default().seed_repo("/repo2", files);
        let service = SharedGitService::in_memory(backend);
        let discovered = service
            .call_value(json!({ "op": "discover", "path": "/repo2" }))
            .unwrap();
        let worktree: GitWorktreeRef =
            serde_json::from_value(discovered["worktree"].clone()).unwrap();
        service
            .call_value(json!({
                "op": "stage",
                "worktree": worktree,
                "paths": ["big.txt"],
                "session_id": "s",
                "turn_id": "t",
                "approved": true
            }))
            .unwrap();
        let diff = service
            .call_value(json!({
                "op": "diff",
                "request": {
                    "worktree": worktree,
                    "scope": "staged",
                    "paths": []
                }
            }))
            .unwrap();
        assert_eq!(diff["kind"], "diff");
        assert!(diff["patch_ref"].is_object());
        assert!(diff["inline_patch"].is_null());
        assert_eq!(diff["truncated"], true);
    }

    #[test]
    fn stale_head_blocks_write() {
        let (service, worktree) = seeded();
        let status = service
            .call_value(json!({ "op": "status", "worktree": worktree }))
            .unwrap();
        let expected = status["head"].clone();
        service
            .call_value(json!({
                "op": "stage",
                "worktree": worktree,
                "paths": ["README.md"],
                "session_id": "s1",
                "turn_id": "t1",
                "approved": true
            }))
            .unwrap();
        let conflict = service
            .call_value(json!({
                "op": "commit",
                "worktree": worktree,
                "message": "stale",
                "allow_empty": true,
                "session_id": "s1",
                "turn_id": "t1",
                "approved": true,
                "expected_head": expected
            }))
            .unwrap();
        assert_eq!(conflict["kind"], "conflict");
    }

    #[test]
    fn cli_state_token_fences_head_index_and_worktree_across_service_restarts() {
        let root = TempDir::new().unwrap();
        git(root.path(), &["init"]);
        git(root.path(), &["config", "user.name", "Mutsuki Test"]);
        git(
            root.path(),
            &["config", "user.email", "mutsuki@example.invalid"],
        );
        fs::write(root.path().join("README.md"), "initial\n").unwrap();
        git(root.path(), &["add", "README.md"]);
        git(root.path(), &["commit", "-m", "initial"]);
        fs::create_dir(root.path().join("nested")).unwrap();

        let service = SharedGitService::new(
            Arc::new(CliGitBackend::default()),
            AgentResourceStore::default(),
        );
        let discovered = service
            .call_value(json!({
                "op": "discover",
                "path": root.path().to_string_lossy()
            }))
            .unwrap();
        let worktree: GitWorktreeRef =
            serde_json::from_value(discovered["worktree"].clone()).unwrap();
        let nested_discovered = service
            .call_value(json!({
                "op": "discover",
                "path": root.path().join("nested").to_string_lossy()
            }))
            .unwrap();
        assert_eq!(nested_discovered["worktree"], discovered["worktree"]);
        let initial = service
            .call_value(json!({ "op": "status", "worktree": worktree }))
            .unwrap();
        let initial_state: GitWorktreeState =
            serde_json::from_value(initial["state"].clone()).unwrap();

        let restarted = SharedGitService::new(
            Arc::new(CliGitBackend::default()),
            AgentResourceStore::default(),
        );
        let after_restart = restarted
            .call_value(json!({ "op": "status", "worktree": worktree }))
            .unwrap();
        assert_eq!(after_restart["state"], initial["state"]);

        fs::write(root.path().join("outside-nested.txt"), "untracked\n").unwrap();
        let untracked_changed = service
            .call_value(json!({ "op": "status", "worktree": worktree }))
            .unwrap();
        assert_ne!(
            untracked_changed["state"]["worktree_hash"],
            initial["state"]["worktree_hash"]
        );
        fs::remove_file(root.path().join("outside-nested.txt")).unwrap();

        fs::write(root.path().join("README.md"), "changed outside service\n").unwrap();
        let worktree_changed = service
            .call_value(json!({ "op": "status", "worktree": worktree }))
            .unwrap();
        let worktree_state: GitWorktreeState =
            serde_json::from_value(worktree_changed["state"].clone()).unwrap();
        assert_eq!(initial_state.head_commit, worktree_state.head_commit);
        assert_eq!(initial_state.index_hash, worktree_state.index_hash);
        assert_ne!(initial_state.worktree_hash, worktree_state.worktree_hash);

        let conflict = service
            .call_value(json!({
                "op": "stage",
                "worktree": worktree,
                "paths": ["README.md"],
                "session_id": "session",
                "turn_id": "turn",
                "approved": true,
                "expected_state": initial_state
            }))
            .unwrap();
        assert_eq!(conflict["kind"], "conflict");
        assert!(conflict["expected_state"].is_object());
        assert!(conflict["actual_state"].is_object());

        let staged = service
            .call_value(json!({
                "op": "stage",
                "worktree": worktree,
                "paths": ["README.md"],
                "session_id": "session",
                "turn_id": "turn",
                "approved": true,
                "expected_state": worktree_state
            }))
            .unwrap();
        assert_eq!(staged["kind"], "staged");
        let index_changed = service
            .call_value(json!({ "op": "status", "worktree": worktree }))
            .unwrap();
        let index_state: GitWorktreeState =
            serde_json::from_value(index_changed["state"].clone()).unwrap();
        assert_ne!(index_state.index_hash, worktree_state.index_hash);

        git(root.path(), &["commit", "-m", "external commit"]);
        let head_conflict = service
            .call_value(json!({
                "op": "commit",
                "worktree": worktree,
                "message": "must not commit",
                "allow_empty": true,
                "session_id": "session",
                "turn_id": "turn",
                "approved": true,
                "expected_state": index_state
            }))
            .unwrap();
        assert_eq!(head_conflict["kind"], "conflict");
        assert_ne!(
            head_conflict["expected_state"]["head_commit"],
            head_conflict["actual_state"]["head_commit"]
        );
    }

    #[test]
    fn high_risk_ops_require_approval() {
        let (service, worktree) = seeded();
        for (op, extra) in [
            ("push", json!({"remote": "origin", "force": false})),
            ("merge", json!({"revision": "main"})),
            ("rebase", json!({"onto": "main"})),
            ("reset", json!({"revision": "main", "mode": "hard"})),
            ("branch_delete", json!({"name": "main", "force": true})),
        ] {
            let mut body = write_ctx(false);
            let obj = body.as_object_mut().unwrap();
            obj.insert("op".into(), json!(op));
            obj.insert("worktree".into(), json!(worktree));
            for (key, value) in extra.as_object().unwrap() {
                obj.insert(key.clone(), value.clone());
            }
            let denied = service.call_value(body).unwrap();
            assert_eq!(denied["kind"], "plan", "op={op}");
            assert_eq!(denied["approval"]["version"], 2);
            assert_eq!(denied["risk"], "high_risk");
        }
        let descriptor = SharedGitService::plugin_descriptor(1).unwrap();
        assert!(descriptor.tools.iter().any(|tool| tool.requires_approval));
        assert!(
            descriptor
                .tools
                .iter()
                .any(|tool| tool.name == "git.status" && !tool.requires_approval)
        );
    }

    #[test]
    fn cancel_and_dispose_clear_handles() {
        let (service, worktree) = seeded();
        let (handle_id, cancel) = service.begin_op(
            GitOperationKind::Push,
            GitRisk::HighRisk,
            Some(worktree),
            "push",
        );
        assert_eq!(service.active_handle_count(), 1);
        assert!(!cancel.load(Ordering::Relaxed));
        service
            .call_value(json!({ "op": "cancel", "handle_id": handle_id }))
            .unwrap();
        assert_eq!(service.active_handle_count(), 0);

        let (handle_id, cancel) =
            service.begin_op(GitOperationKind::Fetch, GitRisk::HighRisk, None, "fetch");
        assert!(!handle_id.is_empty());
        assert_eq!(service.active_handle_count(), 1);
        AgentService::dispose(&service).unwrap();
        assert!(cancel.load(Ordering::Relaxed));
        assert_eq!(service.active_handle_count(), 0);
    }

    #[test]
    fn performance_smoke_status_diff_stage_loop() {
        let (service, worktree) = seeded();
        let started = Instant::now();
        for index in 0..200 {
            service
                .call_value(json!({ "op": "status", "worktree": worktree }))
                .unwrap();
            if index % 10 == 0 {
                service
                    .call_value(json!({
                        "op": "stage",
                        "worktree": worktree,
                        "paths": ["README.md"],
                        "session_id": "s",
                        "turn_id": format!("t{index}"),
                        "approved": true
                    }))
                    .unwrap();
                service
                    .call_value(json!({
                        "op": "diff",
                        "request": {
                            "worktree": worktree,
                            "scope": "staged",
                            "paths": []
                        }
                    }))
                    .unwrap();
                service
                    .call_value(json!({
                        "op": "unstage",
                        "worktree": worktree,
                        "paths": ["README.md"],
                        "session_id": "s",
                        "turn_id": format!("t{index}"),
                        "approved": true
                    }))
                    .unwrap();
            }
        }
        assert_eq!(service.active_handle_count(), 0);
        assert!(
            started.elapsed().as_millis() < 750,
            "git smoke exceeded budget"
        );
    }
}
