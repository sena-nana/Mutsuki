use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use mutsuki_bot_testkit::FakeQqServer;
use mutsuki_service_config::{ConfigOverrides, ServiceConfig};
use mutsuki_service_control::ControlMethod;
use serde_json::Value;

pub struct IpcConfig {
    pub transport: &'static str,
    pub name: &'static str,
    pub tcp_debug_addr: Option<SocketAddr>,
}

pub async fn fake_qq_product(
    root: &Path,
    ipc: IpcConfig,
) -> (FakeQqServer, ServiceConfig, PathBuf) {
    let fake = FakeQqServer::start().await;
    let secret_key = format!("TEMPLATE_QQ_SECRET_{}", fake.websocket_addr().port());
    let qq = fake.config("template", "TEST_APP_ID", &secret_key);
    std::fs::write(
        root.join("product.secret.toml"),
        format!("[secrets]\n{secret_key} = \"TEST_CLIENT_SECRET\"\n"),
    )
    .expect("write local smoke secret");
    let config_path = root.join("product.toml");
    std::fs::write(&config_path, product_toml(root, &ipc, &qq))
        .expect("write product smoke config");
    let service = ServiceConfig::load(ConfigOverrides {
        config_file: Some(config_path.clone()),
        ..Default::default()
    })
    .expect("load product smoke config");
    (fake, service, config_path)
}

pub fn gateway_ready(health: &Value) -> bool {
    health["event_sources"] == "ok"
        && health["components"]["mutsuki.bot.qqbot.gateway:template"]["identified"] == true
}

pub fn assert_gateway_health(health: &Value) {
    assert_eq!(health["service"], "ok");
    assert!(gateway_ready(health));
}

pub fn assert_gateway_only_task_surface(tasks: &Value) {
    let tasks = tasks.to_string();
    assert!(tasks.contains("mutsuki.bot.qqbot.gateway/frame@1"));
    assert!(!tasks.contains("mutsuki.bot.command/parse@1"));
    assert!(!tasks.contains("mutsuki.bot.command/handle@1"));
    assert!(!tasks.contains("mutsuki.bot.message/send@1"));
    assert!(!tasks.contains("TEST_CLIENT_SECRET"));
    assert!(!tasks.contains("TEST_ACCESS_TOKEN"));
}

pub async fn try_control(config: &ServiceConfig, method: ControlMethod) -> Result<Value, String> {
    let client = mutsuki_service_ipc::ControlClient::new(config.into());
    let response = client
        .request(method, Value::Null)
        .await
        .map_err(|error| error.to_string())?;
    if !response.ok {
        return Err(format!("control failed: {:?}", response.error));
    }
    Ok(response.result.unwrap_or(Value::Null))
}

pub async fn control(config: &ServiceConfig, method: ControlMethod) -> Value {
    try_control(config, method).await.unwrap()
}

fn product_toml(
    root: &Path,
    ipc: &IpcConfig,
    qq: &mutsuki_plugin_bot_adapter_qqbot::QqBotConfig,
) -> String {
    let tcp_debug_addr = ipc
        .tcp_debug_addr
        .map(|address| format!("tcp_debug_addr = \"{address}\"\n"))
        .unwrap_or_default();
    format!(
        r#"[service]
profile = "qqbot-fake"
instance_id = "template-qqbot-fake"
home_dir = "{}"
data_dir = "data"
log_dir = "logs"
plugin_dir = "plugins"
run_dir = "run"

[ipc]
enabled = true
transport = "{}"
name = "{}"
{}token = "test-token"

[plugins]
dynamic_dirs = []
disabled_dir = "disabled"

[[plugins.configured]]
id = "mutsuki.bot.adapter.qqbot"
[plugins.configured.config]
account_id = "{}"
app_id = "{}"
client_secret_key = "{}"
token_url = "{}"
openapi_base_url = "{}"
allow_insecure_transport = true
gateway_hello_timeout_ms = 1000
gateway_ack_timeout_ms = 500
retry_base_delay_ms = 0
retry_max_delay_ms = 0
reconnect_initial_delay_ms = 10
reconnect_max_delay_ms = 20
reconnect_jitter_ms = 0

[security]
secret_file = "product.secret.toml"

[observe]
console = false
json = false
log_file = "service.log"
panic_file = "panic.log"
"#,
        root.to_string_lossy().replace('\\', "/"),
        ipc.transport,
        ipc.name,
        tcp_debug_addr,
        qq.account_id,
        qq.app_id,
        qq.client_secret_key,
        qq.token_url,
        qq.openapi_base_url,
    )
}
