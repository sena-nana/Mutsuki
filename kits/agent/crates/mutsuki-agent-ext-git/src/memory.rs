//! Deterministic in-memory Git backend for conformance and shared Agent+UI tests.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use mutsuki_agent_contracts::{
    AgentError, GitBlameLine, GitBlameResult, GitBranchInfo, GitCommitInfo, GitCommitResult,
    GitDiffRequest, GitDiffResult, GitDiffScope, GitFileChange, GitFileStatus, GitHeadIdentity,
    GitRepositoryRef, GitShowResult, GitStatusSnapshot, GitWorktreeInfo, GitWorktreeRef,
    GitWorktreeState,
};

use crate::gateway::GitGateway;
use crate::support::digest_json;

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
