use std::fs;
use std::path::{Path, PathBuf};

use mutsuki_bot::{assemble_service, prepare_distribution, validate_distribution_config};
use mutsuki_distributed_contracts::DistributionMode;
use mutsuki_service_config::{ConfigOverrides, ServiceConfig};
use mutsuki_service_control::ControlMethod;
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
        config_file: Some(product.clone()),
        ..Default::default()
    })
    .unwrap();
    let mut distribution = prepare_distribution(&product, &service).await.unwrap();
    assert!(distribution.start_monitor().is_none());
    assert_eq!(distribution.health_snapshot()["state"], "disabled");
    let runtime = distribution
        .attach_health_probe(assemble_service(service).unwrap())
        .start()
        .await
        .unwrap();
    runtime.shutdown().await;
}

#[tokio::test]
async fn local_observable_never_claims_remote_execution_or_starts_a_monitor() {
    let (_root, product) = product_for(&deployment("single-node.toml"), "local_observable");
    let service = ServiceConfig::load(ConfigOverrides {
        config_file: Some(product.clone()),
        ..Default::default()
    })
    .unwrap();
    let mut distribution = prepare_distribution(&product, &service).await.unwrap();
    assert!(distribution.start_monitor().is_none());
    let health = distribution.health_snapshot();
    assert_eq!(health["state"], "observing_local_only");
    assert_eq!(health["execution"], "local");
    assert_eq!(health["remote_execution"], false);
}

fn clustered_product(fallback: &str, acceptance: &str) -> (tempfile::TempDir, PathBuf) {
    let root = tempdir().unwrap();
    let product = root.path().join("product.toml");
    let secret = root.path().join("product.secret.toml");
    fs::write(
        &secret,
        "[secrets]\nMUTSUKI_DISTRIBUTED_CONTROL_KEY = \"0123456789abcdef0123456789abcdef\"\n",
    )
    .unwrap();
    let deployment = deployment("controller-worker.toml")
        .to_string_lossy()
        .replace('\\', "/");
    let fallback = if fallback.is_empty() {
        String::new()
    } else {
        format!("fallback = \"{fallback}\"\n")
    };
    let home = root
        .path()
        .join("runtime")
        .to_string_lossy()
        .replace('\\', "/");
    let ipc_name = format!(
        "distribution-gate-{}",
        root.path()
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("test")
    );
    let ipc_transport = if cfg!(unix) {
        "unix-socket"
    } else {
        "named-pipe"
    };
    let run_dir = if cfg!(unix) {
        format!("/tmp/{ipc_name}")
    } else {
        root.path().join("run").to_string_lossy().replace('\\', "/")
    };
    fs::write(
        &product,
        format!(
            "[service]\nprofile = \"bot\"\ninstance_id = \"distribution-gate-test\"\nhome_dir = \"{home}\"\nrun_dir = \"{run_dir}\"\n\n[ipc]\nenabled = true\ntransport = \"{ipc_transport}\"\nname = \"{ipc_name}\"\ntoken = \"test-token\"\n\n[security]\nsecret_file = \"{}\"\n\n[observe]\nconsole = false\njson = false\nlog_file = \"service.log\"\npanic_file = \"panic.log\"\n\n[distribution]\nmode = \"clustered\"\ndeployment = \"{deployment}\"\nacceptance = \"{acceptance}\"\n{fallback}",
            secret.to_string_lossy().replace('\\', "/")
        ),
    )
    .unwrap();
    (root, product)
}

#[tokio::test]
async fn clustered_sidecar_missing_fails_before_local_runtime_start() {
    let (_root, product) = clustered_product("", "fast");
    let service = ServiceConfig::load(ConfigOverrides {
        config_file: Some(product.clone()),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(
        prepare_distribution(&product, &service)
            .await
            .err()
            .unwrap()
            .code,
        "distribution.sidecar_unavailable"
    );
}

#[tokio::test]
async fn explicit_fast_fallback_starts_only_as_visible_degraded_local_execution() {
    let (_root, product) = clustered_product("local_degraded", "fast");
    let service = ServiceConfig::load(ConfigOverrides {
        config_file: Some(product.clone()),
        ..Default::default()
    })
    .unwrap();
    let control_config = service.clone();
    let mut distribution = prepare_distribution(&product, &service).await.unwrap();
    let health = distribution.health_snapshot();
    assert_eq!(health["state"], "degraded");
    assert_eq!(health["execution"], "local_fallback");
    assert_eq!(health["remote_execution"], false);
    assert_eq!(
        health["last_error_code"],
        "distribution.sidecar_unavailable"
    );
    let runtime = distribution
        .attach_health_probe(assemble_service(service).unwrap())
        .start()
        .await
        .unwrap();
    let response = mutsuki_service_ipc::ControlClient::new((&control_config).into())
        .request(ControlMethod::HealthCheck, serde_json::Value::Null)
        .await
        .unwrap();
    assert!(response.ok);
    let health = response.result.unwrap();
    assert_eq!(health["components"]["distribution"]["state"], "degraded");
    assert_eq!(
        health["components"]["distribution"]["remote_execution"],
        false
    );
    runtime.shutdown().await;
    let monitor = distribution.start_monitor();
    assert!(monitor.is_some());
    assert!(distribution.start_monitor().is_none());
    drop(monitor);
}

#[test]
fn durable_and_critical_can_never_enable_local_fallback() {
    for acceptance in ["durable", "critical"] {
        let (_root, product) = clustered_product("local_degraded", acceptance);
        assert_eq!(
            validate_distribution_config(&product).unwrap_err().code,
            "distribution.fallback_invalid"
        );
    }
}

#[test]
fn fast_label_cannot_hide_a_durable_feature_requirement_behind_local_fallback() {
    let (root, product) = clustered_product("local_degraded", "fast");
    let committed = deployment("controller-worker.toml");
    let custom = root.path().join("durable-deployment.toml");
    let policy = deployment("task-policies.toml")
        .to_string_lossy()
        .replace('\\', "/");
    let source = fs::read_to_string(&committed)
        .unwrap()
        .replace(
            "required_features = [\"local_observation\", \"clustered\"]",
            "required_features = [\"local_observation\", \"clustered\", \"durable\"]",
        )
        .replace(
            "policy_catalog = \"task-policies.toml\"",
            &format!("policy_catalog = \"{policy}\""),
        );
    fs::write(&custom, source).unwrap();
    let product_source = fs::read_to_string(&product).unwrap().replace(
        &committed.to_string_lossy().replace('\\', "/"),
        &custom.to_string_lossy().replace('\\', "/"),
    );
    fs::write(&product, product_source).unwrap();

    assert_eq!(
        validate_distribution_config(&product).unwrap_err().code,
        "distribution.fallback_invalid"
    );
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
