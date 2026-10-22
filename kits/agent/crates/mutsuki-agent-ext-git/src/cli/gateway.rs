//! `GitGateway` implementation for the `git` CLI backend, including porcelain output parsing.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use mutsuki_agent_contracts::{
    AgentError, GitBlameLine, GitBlameResult, GitBranchInfo, GitCommitInfo, GitCommitResult,
    GitDiffRequest, GitDiffResult, GitDiffScope, GitFileChange, GitFileStatus, GitHeadIdentity,
    GitRepositoryRef, GitShowResult, GitStatusSnapshot, GitWorktreeInfo, GitWorktreeRef,
};

use super::CliGitBackend;
use crate::SUMMARY_LIMIT;
use crate::gateway::GitGateway;
use crate::support::truncate;

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
