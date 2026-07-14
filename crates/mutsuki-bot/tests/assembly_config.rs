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
async fn bilibili_management_starts_with_host_owned_persistence_boundaries() {
    let root = tempdir().unwrap();
    let config_path = root.path().join("product.toml");
    let secret_path = root.path().join("product.secret.toml");
    std::fs::write(&secret_path, "[secrets]\n").unwrap();
    let secret_path = secret_path.to_string_lossy().replace('\\', "/");
    std::fs::write(
        &config_path,
        service_toml(
            root.path(),
            &format!(
                r#"
[security]
secret_file = "{secret_path}"

[[plugins.configured]]
id = "mutsuki.std.resource.memory"

[[plugins.configured]]
id = "mutsuki.bot.command"

[plugins.configured.config]
prefixes = ["/"]

[[plugins.configured]]
id = "mutsuki.bot.bilibili"

[plugins.configured.config]
cookie_secret_key = "BILIBILI_COOKIE"
live_interval_ms = 60000
dynamic_interval_ms = 60000
video_interval_ms = 60000
retry = {{ max_attempts = 3, initial_backoff_ms = 100, max_backoff_ms = 1000 }}
subscriptions = []
link_resolver = {{ enabled = false, cooldown_ms = 1000, account_to_binding = {{}} }}
media_provider_id = "mutsuki.std.resource.memory"
management = {{ enabled = true, allow_self_binding = true, command = "bili", admin_user_ids = ["admin"], self_binding_notifications = ["dynamic"], self_binding_outbound_binding = "qq-main" }}
"#,
            ),
        ),
    )
    .unwrap();

    let runtime = assemble_service(load(&config_path))
        .unwrap()
        .start()
        .await
        .unwrap();
    runtime.shutdown().await;

    let product = std::fs::read_to_string(&config_path).unwrap();
    assert!(product.contains("cookie_secret_key = \"BILIBILI_COOKIE\""));
    assert!(!product.contains("SESSDATA"));
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
async fn bilibili_chromium_backend_fails_startup_without_browser_protocol() {
    let root = tempdir().unwrap();
    let config_path = root.path().join("product.toml");
    let secret_path = root.path().join("product.secret.toml");
    std::fs::write(&secret_path, "[secrets]\n").unwrap();
    let secret_path = secret_path.to_string_lossy().replace('\\', "/");
    std::fs::write(
        &config_path,
        service_toml(
            root.path(),
            &format!(
                r#"
[security]
secret_file = "{secret_path}"

[[plugins.configured]]
id = "mutsuki.std.resource.memory"

[[plugins.configured]]
id = "mutsuki.bot.bilibili"

[plugins.configured.config]
cookie_secret_key = "BILIBILI_COOKIE"
live_interval_ms = 60000
dynamic_interval_ms = 60000
video_interval_ms = 60000
retry = {{ max_attempts = 3, initial_backoff_ms = 100, max_backoff_ms = 1000 }}
subscriptions = []
link_resolver = {{ enabled = false, cooldown_ms = 1000, account_to_binding = {{}} }}
media_provider_id = "mutsuki.std.resource.memory"
risk_control = {{ backend = "chromium", timeout_ms = 10000, max_response_bytes = 2097152 }}
management = {{ enabled = true, allow_self_binding = true, command = "bili", admin_user_ids = ["admin"], self_binding_notifications = ["dynamic"], self_binding_outbound_binding = "qq-main" }}
"#,
            ),
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
