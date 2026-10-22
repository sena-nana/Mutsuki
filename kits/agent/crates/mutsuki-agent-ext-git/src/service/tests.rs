//! Behavior tests for the shared Git service over in-memory and CLI backends.

use super::*;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use mutsuki_agent_contracts::GitWorktreeState;
use tempfile::TempDir;

use crate::CliGitBackend;

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
    let worktree: GitWorktreeRef = serde_json::from_value(discovered["worktree"].clone()).unwrap();
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
    let worktree: GitWorktreeRef = serde_json::from_value(discovered["worktree"].clone()).unwrap();
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
    let worktree: GitWorktreeRef = serde_json::from_value(discovered["worktree"].clone()).unwrap();
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
    let initial_state: GitWorktreeState = serde_json::from_value(initial["state"].clone()).unwrap();

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
