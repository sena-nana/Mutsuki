//! Per-request dispatch of `GitServiceRequest` operations to the backend.

use std::sync::atomic::Ordering;

use mutsuki_agent_contracts::{
    AgentError, GitLogResult, GitOperationKind, GitOperationState, GitRisk, GitServiceRequest,
    GitServiceResponse, GitWorktreeRef,
};
use serde_json::json;

use super::SharedGitService;
use crate::SUMMARY_LIMIT;
use crate::support::truncate;

impl SharedGitService {
    pub(super) fn dispatch_inner(
        &self,
        request: GitServiceRequest,
    ) -> Result<GitServiceResponse, AgentError> {
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
