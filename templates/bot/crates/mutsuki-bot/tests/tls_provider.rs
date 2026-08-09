use mutsuki_bot::assemble_service;
use mutsuki_service_config::{ConfigOverrides, ServiceConfig};
use tempfile::tempdir;

#[tokio::test]
async fn service_runtime_installs_tls_provider_before_clients_start() {
    let root = tempdir().unwrap();
    let config_path = root.path().join("product.toml");
    let home = root.path().to_string_lossy().replace('\\', "/");
    std::fs::write(
        &config_path,
        format!(
            r#"[service]
profile = "test"
instance_id = "tls-provider-test"
home_dir = "{home}"
data_dir = "data"
log_dir = "logs"
plugin_dir = "plugins"
run_dir = "run"

[ipc]
enabled = false
transport = "named-pipe"
name = "tls-provider-test"
token = "test-token"

[plugins]
dynamic_dirs = []
disabled_dir = "disabled"

[observe]
console = false
json = false
log_file = "service.log"
panic_file = "panic.log"
"#,
        ),
    )
    .unwrap();
    let service = ServiceConfig::load(ConfigOverrides {
        config_file: Some(config_path),
        ..Default::default()
    })
    .unwrap();

    let runtime = assemble_service(service).unwrap().start().await.unwrap();
    let _client = reqwest::Client::new();
    runtime.shutdown().await;
}
