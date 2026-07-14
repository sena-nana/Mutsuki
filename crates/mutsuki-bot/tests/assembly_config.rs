use std::path::Path;

use mutsuki_bot::assemble_service;
use mutsuki_service_config::{ConfigOverrides, ServiceConfig};
use tempfile::tempdir;

const SIMPLE_TEMPLATE: &str = include_str!("../../../config/template.toml");

#[tokio::test]
async fn empty_external_config_starts_and_stops_neutral_runtime() {
    let root = tempdir().unwrap();
    let config_path = root.path().join("product.toml");
    std::fs::write(&config_path, service_toml(root.path(), "")).unwrap();
    let service = load(&config_path);

    let runtime = assemble_service(service).unwrap().start().await.unwrap();
    runtime.shutdown().await;
}

#[tokio::test]
async fn unknown_configured_plugin_fails_loud() {
    let root = tempdir().unwrap();
    let config_path = root.path().join("product.toml");
    std::fs::write(
        &config_path,
        service_toml(
            root.path(),
            r#"
[[plugins.configured]]
id = "owner.plugin.not-linked"
"#,
        ),
    )
    .unwrap();

    let error = match assemble_service(load(&config_path)).unwrap().start().await {
        Ok(runtime) => {
            runtime.shutdown().await;
            panic!("unknown configured plugin started")
        }
        Err(error) => error,
    };
    assert!(error.to_string().contains("owner.plugin.not-linked"));
}

#[tokio::test]
async fn configured_qq_integration_fails_preflight_without_host_secret() {
    let root = tempdir().unwrap();
    let config_path = root.path().join("product.toml");
    std::fs::write(
        &config_path,
        service_toml(
            root.path(),
            r#"
[[plugins.configured]]
id = "mutsuki.bot.adapter.qqbot"

[plugins.configured.config]
account_id = "configured-account"
app_id = "configured-app"
client_secret_key = "MISSING_TEMPLATE_QQ_SECRET"
"#,
        ),
    )
    .unwrap();
    let service = load(&config_path);

    let error = match assemble_service(service).unwrap().start().await {
        Ok(runtime) => {
            runtime.shutdown().await;
            panic!("QQ integration started without required Host secret")
        }
        Err(error) => error,
    };
    assert!(error.to_string().contains("MISSING_TEMPLATE_QQ_SECRET"));
}

#[tokio::test]
async fn workshop_fails_startup_without_explicit_media_provider() {
    let root = tempdir().unwrap();
    let config_path = root.path().join("product.toml");
    std::fs::write(
        &config_path,
        service_toml(
            root.path(),
            r#"
[[plugins.configured]]
id = "mutsuki.bot.bilibili.workshop"

[plugins.configured.config]
media_provider_id = "missing.media.provider"
max_media_bytes = 8388608
"#,
        ),
    )
    .unwrap();
    let error = assemble_service(load(&config_path))
        .unwrap()
        .start()
        .await
        .err()
        .expect("missing provider must fail startup");
    assert!(error.to_string().contains("missing.media.provider"));
}

#[tokio::test]
async fn mihuashi_fails_startup_without_browser_protocol() {
    let root = tempdir().unwrap();
    let config_path = root.path().join("product.toml");
    std::fs::write(
        &config_path,
        service_toml(
            root.path(),
            r#"
[[plugins.configured]]
id = "mutsuki.std.resource.memory"

[[plugins.configured]]
id = "mutsuki.bot.mihuashi"

[plugins.configured.config]
media_provider_id = "mutsuki.std.resource.memory"
max_media_bytes = 8388608
"#,
        ),
    )
    .unwrap();
    let error = assemble_service(load(&config_path))
        .unwrap()
        .start()
        .await
        .err()
        .expect("missing browser protocol must fail startup");
    assert!(error.to_string().contains("mutsuki.browser.snapshot"));
}

#[tokio::test]
async fn chromium_factory_rejects_missing_artifact_during_assembly() {
    let root = tempdir().unwrap();
    let config_path = root.path().join("product.toml");
    std::fs::write(
        &config_path,
        service_toml(
            root.path(),
            r#"
[[plugins.configured]]
id = "mutsuki.std.io.browser.chromium"

[plugins.configured.config]
executable = "/definitely/missing/chromium"
domain_allowlist = ["mihuashi.com"]
timeout_ms = 10000
max_dom_bytes = 2097152
"#,
        ),
    )
    .unwrap();
    let error = assemble_service(load(&config_path))
        .unwrap()
        .start()
        .await
        .err()
        .expect("missing Chromium artifact must fail startup");
    assert!(error.to_string().contains("Chromium executable"));
}

#[test]
fn committed_template_exposes_only_product_configuration() {
    let root = tempdir().unwrap();
    let config_path = root.path().join("local.toml");
    let secret_path = root.path().join("local.secret.toml");
    let home = root
        .path()
        .join("home")
        .to_string_lossy()
        .replace('\\', "/");
    let config = SIMPLE_TEMPLATE.replace("[service]", &format!("[service]\nhome_dir = \"{home}\""));
    std::fs::write(&config_path, config).unwrap();
    std::fs::write(&secret_path, "[secrets]\n").unwrap();

    let service = load(&config_path);

    assert_eq!(service.service.instance_id, "mutsuki-bot");
    assert!(service.plugins.configured.is_empty());
    assert_eq!(service.core.max_tasks, 4096);
    assert!(service.runners.restart);
    assert!(!SIMPLE_TEMPLATE.contains("[core]"));
    assert!(!SIMPLE_TEMPLATE.contains("[runners]"));
    assert!(!SIMPLE_TEMPLATE.contains("[observe]"));
}

fn load(path: &Path) -> ServiceConfig {
    ServiceConfig::load(ConfigOverrides {
        config_file: Some(path.to_path_buf()),
        ..Default::default()
    })
    .unwrap()
}

fn service_toml(root: &Path, configured: &str) -> String {
    format!(
        r#"[service]
profile = "test"
instance_id = "template-test"
home_dir = "{}"
data_dir = "data"
log_dir = "logs"
plugin_dir = "plugins"
run_dir = "run"

[ipc]
enabled = false
transport = "named-pipe"
name = "template-test"
token = "test-token"

[plugins]
dynamic_dirs = []
disabled_dir = "disabled"
{}

[observe]
console = false
json = false
log_file = "service.log"
panic_file = "panic.log"
"#,
        root.to_string_lossy().replace('\\', "/"),
        configured
    )
}
