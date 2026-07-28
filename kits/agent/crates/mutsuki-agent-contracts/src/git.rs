use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{PermissionRequest, ResourceRef, ToolSideEffect};

/// Discovered Git repository identity shared by Agent tools and product UI.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GitRepositoryRef {
    pub repo_id: String,
    pub root: String,
}

/// Worktree identity bound to a repository. Affinity key for shared GitService.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GitWorktreeRef {
    pub worktree_id: String,
    pub path: String,
    pub repository: GitRepositoryRef,
}

/// Optimistic concurrency token for write ops (HEAD / revision binding).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitHeadIdentity {
    pub commit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    /// Monotonic generation observed by the service for this worktree.
    pub generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitRevisionConflict {
    pub worktree: GitWorktreeRef,
    pub expected: GitHeadIdentity,
    pub actual: GitHeadIdentity,
    pub message: String,
}

impl GitRevisionConflict {
    pub fn stale(
        worktree: GitWorktreeRef,
        expected: GitHeadIdentity,
        actual: GitHeadIdentity,
    ) -> Self {
        Self {
            worktree,
            message: format!(
                "git write expected head {} (gen {}), observed {} (gen {})",
                expected.commit, expected.generation, actual.commit, actual.generation
            ),
            expected,
            actual,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitRisk {
    /// status / diff / log / show / blame / list
    ReadOnly,
    /// stage / unstage / commit / branch create|switch
    ReversibleWrite,
    /// push / pull / fetch / merge / rebase / reset / cherry-pick / delete / force
    HighRisk,
}

impl GitRisk {
    pub fn requires_approval(self) -> bool {
        matches!(self, Self::HighRisk)
    }

    pub fn side_effect(self) -> ToolSideEffect {
        match self {
            Self::ReadOnly => ToolSideEffect::WorkspaceRead,
            Self::ReversibleWrite => ToolSideEffect::WorkspaceWrite,
            Self::HighRisk => ToolSideEffect::ExternalWrite,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitFileStatus {
    Untracked,
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    Conflicted,
    Ignored,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GitFileChange {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    pub status: GitFileStatus,
    #[serde(default)]
    pub staged: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hunks_summary: Option<String>,
    #[serde(default)]
    pub additions: u64,
    #[serde(default)]
    pub deletions: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GitStatusSnapshot {
    pub worktree: GitWorktreeRef,
    pub head: GitHeadIdentity,
    #[serde(default)]
    pub changes: Vec<GitFileChange>,
    #[serde(default)]
    pub clean: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitDiffScope {
    WorkingTree,
    Staged,
    Range,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GitDiffRequest {
    pub worktree: GitWorktreeRef,
    pub scope: GitDiffScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    #[serde(default)]
    pub paths: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GitDiffResult {
    pub worktree: GitWorktreeRef,
    pub base: GitHeadIdentity,
    pub head: GitHeadIdentity,
    pub summary: String,
    #[serde(default)]
    pub files: Vec<GitFileChange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch_ref: Option<ResourceRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline_patch: Option<String>,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitBranchInfo {
    pub name: String,
    pub tip: String,
    #[serde(default)]
    pub current: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitWorktreeInfo {
    pub worktree: GitWorktreeRef,
    pub head: GitHeadIdentity,
    #[serde(default)]
    pub locked: bool,
    #[serde(default)]
    pub prunable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitCommitInfo {
    pub hash: String,
    pub summary: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub timestamp_unix_ms: u64,
    #[serde(default)]
    pub parents: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GitCommitResult {
    pub commit: GitCommitInfo,
    pub head: GitHeadIdentity,
    #[serde(default)]
    pub changed_files: Vec<GitFileChange>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GitLogResult {
    pub commits: Vec<GitCommitInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details_ref: Option<ResourceRef>,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GitShowResult {
    pub commit: GitCommitInfo,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch_ref: Option<ResourceRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline_patch: Option<String>,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitBlameLine {
    pub line: u32,
    pub commit: String,
    pub author: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GitBlameResult {
    pub path: String,
    pub lines: Vec<GitBlameLine>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details_ref: Option<ResourceRef>,
    pub truncated: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitOperationKind {
    Discover,
    Status,
    Diff,
    Stage,
    Unstage,
    Commit,
    BranchList,
    BranchCreate,
    BranchSwitch,
    BranchDelete,
    WorktreeList,
    WorktreeCreate,
    WorktreeRemove,
    Log,
    Show,
    Blame,
    Push,
    Pull,
    Fetch,
    Merge,
    Rebase,
    Reset,
    CherryPick,
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitOperationState {
    Queued,
    Running,
    WaitingApproval,
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GitOperationHandle {
    pub handle_id: String,
    pub kind: GitOperationKind,
    pub state: GitOperationState,
    pub risk: GitRisk,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<GitWorktreeRef>,
    #[serde(default)]
    pub summary: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GitActionPlan {
    pub action_id: String,
    pub operation: String,
    pub risk: GitRisk,
    pub summary: String,
    pub approval: PermissionRequest,
    pub worktree: GitWorktreeRef,
    pub head: GitHeadIdentity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<Value>,
}

fn default_timeout_ms() -> u64 {
    30_000
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GitWriteContext {
    pub session_id: String,
    pub turn_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_version: Option<u64>,
    #[serde(default)]
    pub approved: bool,
    /// When set, reject the write if the live HEAD / generation differs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_head: Option<GitHeadIdentity>,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum GitServiceRequest {
    Discover {
        path: String,
    },
    Status {
        worktree: GitWorktreeRef,
    },
    Diff {
        request: GitDiffRequest,
    },
    Stage {
        worktree: GitWorktreeRef,
        paths: Vec<String>,
        #[serde(flatten)]
        write: GitWriteContext,
    },
    Unstage {
        worktree: GitWorktreeRef,
        paths: Vec<String>,
        #[serde(flatten)]
        write: GitWriteContext,
    },
    Commit {
        worktree: GitWorktreeRef,
        message: String,
        #[serde(default)]
        allow_empty: bool,
        #[serde(flatten)]
        write: GitWriteContext,
    },
    BranchList {
        worktree: GitWorktreeRef,
    },
    BranchCreate {
        worktree: GitWorktreeRef,
        name: String,
        #[serde(default)]
        checkout: bool,
        #[serde(flatten)]
        write: GitWriteContext,
    },
    BranchSwitch {
        worktree: GitWorktreeRef,
        name: String,
        #[serde(flatten)]
        write: GitWriteContext,
    },
    BranchDelete {
        worktree: GitWorktreeRef,
        name: String,
        #[serde(default)]
        force: bool,
        #[serde(flatten)]
        write: GitWriteContext,
    },
    WorktreeList {
        repository: GitRepositoryRef,
    },
    WorktreeCreate {
        repository: GitRepositoryRef,
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        branch: Option<String>,
        #[serde(flatten)]
        write: GitWriteContext,
    },
    WorktreeRemove {
        worktree: GitWorktreeRef,
        #[serde(default)]
        force: bool,
        #[serde(flatten)]
        write: GitWriteContext,
    },
    Log {
        worktree: GitWorktreeRef,
        #[serde(default = "default_log_limit")]
        limit: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
    },
    Show {
        worktree: GitWorktreeRef,
        revision: String,
    },
    Blame {
        worktree: GitWorktreeRef,
        path: String,
        #[serde(default = "default_blame_limit")]
        max_lines: u32,
    },
    Push {
        worktree: GitWorktreeRef,
        #[serde(default)]
        remote: String,
        #[serde(default)]
        refspec: Option<String>,
        #[serde(default)]
        force: bool,
        #[serde(flatten)]
        write: GitWriteContext,
    },
    Pull {
        worktree: GitWorktreeRef,
        #[serde(default)]
        remote: String,
        #[serde(default)]
        rebase: bool,
        #[serde(flatten)]
        write: GitWriteContext,
    },
    Fetch {
        worktree: GitWorktreeRef,
        #[serde(default)]
        remote: String,
        #[serde(flatten)]
        write: GitWriteContext,
    },
    Merge {
        worktree: GitWorktreeRef,
        revision: String,
        #[serde(flatten)]
        write: GitWriteContext,
    },
    Rebase {
        worktree: GitWorktreeRef,
        onto: String,
        #[serde(flatten)]
        write: GitWriteContext,
    },
    Reset {
        worktree: GitWorktreeRef,
        revision: String,
        #[serde(default)]
        mode: String,
        #[serde(flatten)]
        write: GitWriteContext,
    },
    CherryPick {
        worktree: GitWorktreeRef,
        revision: String,
        #[serde(flatten)]
        write: GitWriteContext,
    },
    OperationState {
        handle_id: String,
    },
    Cancel {
        handle_id: String,
    },
}

fn default_log_limit() -> u32 {
    50
}

fn default_blame_limit() -> u32 {
    500
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GitServiceResponse {
    Discovered {
        worktree: GitWorktreeRef,
        head: GitHeadIdentity,
    },
    Status(GitStatusSnapshot),
    Diff(GitDiffResult),
    Staged {
        paths: Vec<String>,
        head: GitHeadIdentity,
    },
    Unstaged {
        paths: Vec<String>,
        head: GitHeadIdentity,
    },
    Committed(GitCommitResult),
    Branches {
        branches: Vec<GitBranchInfo>,
    },
    Branch {
        branch: GitBranchInfo,
        head: GitHeadIdentity,
    },
    Worktrees {
        worktrees: Vec<GitWorktreeInfo>,
    },
    Worktree {
        worktree: GitWorktreeInfo,
    },
    Log(GitLogResult),
    Show(GitShowResult),
    Blame(GitBlameResult),
    RemoteOk {
        summary: String,
        head: GitHeadIdentity,
    },
    Merged {
        summary: String,
        head: GitHeadIdentity,
    },
    Plan(GitActionPlan),
    Operation(GitOperationHandle),
    Ack,
    Conflict(GitRevisionConflict),
}
