//! `GitGateway` process / VCS execution backend trait.

use std::sync::atomic::AtomicBool;

use mutsuki_agent_contracts::{
    AgentError, GitBlameResult, GitBranchInfo, GitCommitInfo, GitCommitResult, GitDiffRequest,
    GitDiffResult, GitHeadIdentity, GitRepositoryRef, GitShowResult, GitStatusSnapshot,
    GitWorktreeInfo, GitWorktreeRef,
};

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
