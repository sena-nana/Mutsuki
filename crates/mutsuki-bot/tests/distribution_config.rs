use std::fs;
use std::path::{Path, PathBuf};

use mutsuki_bot::{assemble_service, validate_distribution_config};
use mutsuki_distributed_contracts::DistributionMode;
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
    workspace_root().join("deploy/distribution").join(name)
}

fn product_for(deployment: &Path, mode: &str) -> (tempfile::TempDir, PathBuf) {
    let root = tempdir().unwrap();
    let product = root.path().join("product.toml");
    let deployment = deployment.to_string_lossy().replace('\\', "/");
    fs::write(
        &product,
        format!("[distribution]\nmode = \"{mode}\"\ndeployment = \"{deployment}\"\n"),
    )
    .unwrap();
    (root, product)
}

#[test]
fn committed_template_is_explicitly_disabled() {
    let template = workspace_root().join("config/template.toml");
    assert_eq!(
        validate_distribution_config(&template).unwrap(),
        DistributionMode::Disabled
    );
    let source = fs::read_to_string(template).unwrap();
    assert!(source.contains("mode = \"disabled\""));
    assert!(!source.contains("deployment ="));
}

#[tokio::test]
async fn disabled_distribution_keeps_the_real_local_runtime_unchanged() {
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

    validate_distribution_config(&product).unwrap();
    let service = ServiceConfig::load(ConfigOverrides {
        config_file: Some(product),
        ..Default::default()
    })
    .unwrap();
    let runtime = assemble_service(service).unwrap().start().await.unwrap();
    runtime.shutdown().await;
}

#[test]
fn every_committed_topology_is_a_valid_external_deployment() {
    for file in [
        "single-node.toml",
        "controller-worker.toml",
        "ha-three-voters-worker.toml",
    ] {
        let (_root, product) = product_for(&deployment(file), "clustered");
        assert_eq!(
            validate_distribution_config(&product).unwrap(),
            DistributionMode::Clustered
        );
    }

    let (_root, product) = product_for(&deployment("single-node.toml"), "local_observable");
    assert_eq!(
        validate_distribution_config(&product).unwrap(),
        DistributionMode::LocalObservable
    );
}

#[test]
fn missing_unknown_or_unsafe_deployment_configuration_fails_loud() {
    let root = tempdir().unwrap();
    let product = root.path().join("product.toml");
    fs::write(&product, "[distribution]\nmode = \"clustered\"\n").unwrap();
    assert_eq!(
        validate_distribution_config(&product).unwrap_err().code,
        "distribution.deployment_required"
    );

    fs::write(
        &product,
        "[distribution]\nmode = \"disabled\"\nunknown = true\n",
    )
    .unwrap();
    assert_eq!(
        validate_distribution_config(&product).unwrap_err().code,
        "distribution.selection_invalid"
    );

    let unsafe_deployment = root.path().join("unsafe.toml");
    let source = fs::read_to_string(deployment("single-node.toml"))
        .unwrap()
        .replace("leader_proxy = false", "leader_proxy = true");
    fs::write(&unsafe_deployment, source).unwrap();
    let (_product_root, product) = product_for(&unsafe_deployment, "clustered");
    assert_eq!(
        validate_distribution_config(&product).unwrap_err().code,
        "distribution.channel_policy_invalid"
    );
}

#[test]
fn policy_examples_cover_epic_choices_without_plugin_cluster_context() {
    let source = fs::read_to_string(deployment("task-policies.toml")).unwrap();
    let catalog: toml::Value = toml::from_str(&source).unwrap();
    let tasks = catalog["task"].as_array().unwrap();

    for expected in [
        "local_only",
        "restartable",
        "checkpointable",
        "portable",
        "hard_realtime",
        "interactive",
        "batch",
        "background",
        "fast",
        "durable",
        "critical",
        "idempotent",
        "verifiable",
        "compensatable",
        "unsafe",
    ] {
        assert!(
            source.contains(expected),
            "missing policy example: {expected}"
        );
    }
    assert_eq!(tasks.len(), 5);
    let policy = |name: &str| {
        tasks
            .iter()
            .find(|task| task["name"].as_str() == Some(name))
            .unwrap()
    };
    assert_eq!(
        policy("portable-restartable-interactive")["fallback"].as_str(),
        Some("local_only")
    );
    for name in ["checkpointed-batch", "critical-verifiable"] {
        assert_eq!(policy(name)["fallback"].as_str(), Some("reject"));
    }
    assert!(!source.contains("node_id"));
    assert!(!source.contains("trust_level"));
    assert!(source.contains("minimum_trust = \"managed\""));
    assert!(source.contains("minimum_trust = \"trusted\""));
}
