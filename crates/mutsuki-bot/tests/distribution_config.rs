use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use mutsuki_bot::{
    FallbackPolicy, NodeRole, TopologyKind, UnavailableDecision, assemble_service,
    load_distribution_plan,
};
use mutsuki_distributed_contracts::{AcceptanceMode, DistributionMode, RecoveryTier};
use mutsuki_runtime_contracts::{ExecutionMobility, LatencyClass, RetrySafety};
use mutsuki_service_config::{ConfigOverrides, ServiceConfig};
use tempfile::tempdir;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_owned()
}

fn deployment(name: &str) -> PathBuf {
    workspace_root()
        .join("deploy")
        .join("distribution")
        .join(name)
}

fn load_example(name: &str) -> mutsuki_bot::DistributionPlan {
    let root = tempdir().unwrap();
    let product = root.path().join("product.toml");
    let path = deployment(name).to_string_lossy().replace('\\', "/");
    fs::write(
        &product,
        format!(
            r#"[distribution]
mode = "clustered"
deployment = "{path}"
"#,
        ),
    )
    .unwrap();
    load_distribution_plan(&product).unwrap()
}

#[test]
fn committed_template_defaults_to_zero_side_effect_distribution() {
    let plan = load_distribution_plan(&workspace_root().join("config/template.toml")).unwrap();
    assert_eq!(plan.mode, DistributionMode::Disabled);
    assert_eq!(plan.template_managed_processes(), 0);
    assert!(!plan.template_opens_distributed_network());
    assert!(!plan.requires_external_service());
    assert!(plan.deployment.is_none());
    assert!(plan.task_policies.is_empty());
}

#[tokio::test]
async fn disabled_product_starts_and_stops_the_real_local_service_runtime() {
    let root = tempdir().unwrap();
    let product = root.path().join("product.toml");
    let home = root
        .path()
        .join("runtime")
        .to_string_lossy()
        .replace('\\', "/");
    fs::write(
        &product,
        format!(
            r#"[service]
profile = "bot"
instance_id = "distribution-disabled-test"
home_dir = "{home}"

[ipc]
enabled = false
transport = "named-pipe"
name = "distribution-disabled-test"
token = "test-token"

[observe]
console = false
json = false
log_file = "service.log"
panic_file = "panic.log"

[distribution]
mode = "disabled"
"#,
        ),
    )
    .unwrap();

    let plan = load_distribution_plan(&product).unwrap();
    assert_eq!(plan.template_managed_processes(), 0);
    let service = ServiceConfig::load(ConfigOverrides {
        config_file: Some(product),
        ..Default::default()
    })
    .unwrap();
    let runtime = assemble_service(service).unwrap().start().await.unwrap();
    runtime.shutdown().await;
}

#[test]
fn all_supported_topologies_are_parseable_and_strictly_shaped() {
    for (file, expected_kind, voters, workers) in [
        ("single-node.toml", TopologyKind::SingleNode, 1, 1),
        (
            "controller-worker.toml",
            TopologyKind::ControllerWorker,
            1,
            1,
        ),
        (
            "ha-three-voters-worker.toml",
            TopologyKind::HaThreeVotersWorker,
            3,
            1,
        ),
    ] {
        let plan = load_example(file);
        let deployment = plan.deployment.as_ref().unwrap();
        assert_eq!(deployment.topology.kind, expected_kind);
        assert_eq!(
            deployment.external_service.local_host_control_endpoint,
            "servicehost://local/control"
        );
        assert_eq!(
            deployment
                .topology
                .nodes
                .iter()
                .filter(|node| node.roles.contains(&NodeRole::Voter))
                .count(),
            voters
        );
        assert_eq!(
            deployment
                .topology
                .nodes
                .iter()
                .filter(|node| node.roles.contains(&NodeRole::Worker))
                .count(),
            workers
        );
        assert!(deployment.channels.control.authenticated);
        assert!(deployment.channels.control.encrypted);
        assert!(deployment.channels.data.authenticated);
        assert!(deployment.channels.data.encrypted);
        assert!(deployment.channels.data.direct_worker_transfer);
        assert!(!deployment.channels.data.leader_proxy);
        assert_ne!(
            deployment.channels.control.endpoint,
            deployment.channels.data.endpoint
        );
        assert!(deployment.budgets.cpu_units > 0);
        assert!(deployment.budgets.memory_bytes > 0);
        assert!(deployment.budgets.network_bytes_per_second > 0);
        assert!(deployment.budgets.transfer_concurrency > 0);
        assert!(deployment.budgets.checkpoint_bytes_per_second > 0);
        assert_eq!(plan.template_managed_processes(), 0);
        assert!(!plan.template_opens_distributed_network());
        assert!(plan.requires_external_service());
    }
}

#[test]
fn policy_catalog_covers_mobility_latency_acceptance_effect_and_explicit_output_choices() {
    let plan = load_example("ha-three-voters-worker.toml");
    let mobility: BTreeSet<_> = plan
        .task_policies
        .iter()
        .map(|policy| format!("{:?}", policy.mobility))
        .collect();
    assert!(mobility.contains("LocalOnly"));
    assert!(mobility.contains("Restartable"));
    assert!(mobility.contains("Checkpointable"));
    assert!(mobility.contains("Portable"));

    for latency in [
        LatencyClass::HardRealtime,
        LatencyClass::Interactive,
        LatencyClass::Batch,
        LatencyClass::Background,
    ] {
        assert!(
            plan.task_policies
                .iter()
                .any(|policy| policy.latency == latency)
        );
    }
    assert!(
        plan.task_policies
            .iter()
            .any(|policy| matches!(policy.acceptance, AcceptanceMode::Fast))
    );
    assert!(
        plan.task_policies
            .iter()
            .any(|policy| matches!(policy.acceptance, AcceptanceMode::Durable))
    );
    assert!(plan.task_policies.iter().any(|policy| matches!(
        policy.acceptance,
        AcceptanceMode::Critical {
            minimum_replicas: 3
        }
    )));
    for effect in [
        RetrySafety::Idempotent,
        RetrySafety::Verifiable,
        RetrySafety::Compensatable,
        RetrySafety::Unsafe,
    ] {
        assert!(
            plan.task_policies
                .iter()
                .any(|policy| policy.effect == effect)
        );
    }
    assert!(plan.task_policies.iter().all(|policy| {
        policy.quality.minimum_level <= policy.quality.requested_level
            && matches!(
                policy.fallback,
                FallbackPolicy::LocalOnly | FallbackPolicy::Reject
            )
    }));
    assert!(plan.task_policies.iter().any(|policy| {
        matches!(policy.mobility, ExecutionMobility::Checkpointable)
            && matches!(policy.recovery, RecoveryTier::Checkpointed)
    }));
}

#[test]
fn unavailable_sidecar_never_fakes_durable_acceptance() {
    let plan = load_example("controller-worker.toml");
    let local = plan
        .task_policies
        .iter()
        .find(|policy| policy.name == "portable-restartable-interactive")
        .unwrap();
    assert_eq!(
        plan.unavailable_decision(local).unwrap(),
        UnavailableDecision::RunLocal
    );

    for name in ["checkpointed-batch", "critical-verifiable"] {
        let policy = plan
            .task_policies
            .iter()
            .find(|policy| policy.name == name)
            .unwrap();
        let error = plan.unavailable_decision(policy).unwrap_err();
        assert_eq!(error.code, "distribution.durability_unavailable");
    }
}

#[test]
fn enabled_mode_requires_an_explicit_readable_deployment() {
    let root = tempdir().unwrap();
    let product = root.path().join("product.toml");
    fs::write(&product, "[distribution]\nmode = \"clustered\"\n").unwrap();
    assert_eq!(
        load_distribution_plan(&product).unwrap_err().code,
        "distribution.deployment_required"
    );

    fs::write(
        &product,
        "[distribution]\nmode = \"clustered\"\ndeployment = \"missing.toml\"\n",
    )
    .unwrap();
    assert_eq!(
        load_distribution_plan(&product).unwrap_err().code,
        "distribution.deployment_unreadable"
    );
}

#[test]
fn unknown_distribution_fields_and_leader_data_proxy_fail_loud() {
    let root = tempdir().unwrap();
    let product = root.path().join("product.toml");
    fs::write(
        &product,
        "[distribution]\nmode = \"disabled\"\nmagic = true\n",
    )
    .unwrap();
    assert_eq!(
        load_distribution_plan(&product).unwrap_err().code,
        "distribution.selection_invalid"
    );

    let bad_deployment = root.path().join("deployment.toml");
    let source = fs::read_to_string(deployment("single-node.toml"))
        .unwrap()
        .replace("leader_proxy = false", "leader_proxy = true");
    fs::write(&bad_deployment, source).unwrap();
    let deployment_path = bad_deployment.to_string_lossy().replace('\\', "/");
    fs::write(
        &product,
        format!("[distribution]\nmode = \"clustered\"\ndeployment = \"{deployment_path}\"\n"),
    )
    .unwrap();
    assert_eq!(
        load_distribution_plan(&product).unwrap_err().code,
        "distribution.leader_data_proxy_forbidden"
    );
}

#[test]
fn committed_deployment_examples_contain_only_secret_references() {
    let policies = fs::read_to_string(deployment("task-policies.toml")).unwrap();
    assert!(!policies.contains("node_id"));
    assert!(!policies.contains("trust_level"));

    for file in [
        "single-node.toml",
        "controller-worker.toml",
        "ha-three-voters-worker.toml",
    ] {
        let source = fs::read_to_string(deployment(file)).unwrap();
        assert!(source.contains("control_secret_key = \"MUTSUKI_"));
        assert!(source.contains("identity_secret_key = \"MUTSUKI_"));
        assert!(!source.contains("BEGIN PRIVATE KEY"));
        assert!(!source.contains("password ="));
    }
}
