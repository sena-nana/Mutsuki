use std::time::Duration;

use mutsuki_bot::assemble_service;
use mutsuki_bot_testkit::FakeQqServer;
use mutsuki_service_config::{ConfigOverrides, ServiceConfig};
use mutsuki_service_control::ControlMethod;
use serde_json::Value;
use tempfile::tempdir;
use tokio::net::TcpListener;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn external_config_runs_real_service_runtime_through_fake_qq_boundaries() {
    let fake = FakeQqServer::start().await;
    let secret_key = format!("TEMPLATE_QQ_SECRET_{}", fake.websocket_addr().port());

    let root = tempdir().unwrap();
    let probe = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ipc_addr = probe.local_addr().unwrap();
    drop(probe);
    let qq = fake.config("template", "TEST_APP_ID", &secret_key);
    std::fs::write(
        root.path().join("product.secret.toml"),
        format!("[secrets]\n{secret_key} = \"TEST_CLIENT_SECRET\"\n"),
    )
    .unwrap();
    let config_path = root.path().join("product.toml");
    std::fs::write(&config_path, product_toml(root.path(), ipc_addr, &qq)).unwrap();
    let service = ServiceConfig::load(ConfigOverrides {
        config_file: Some(config_path),
        ..Default::default()
    })
    .unwrap();
    let control_config = service.clone();

    let runtime = assemble_service(service).unwrap().start().await.unwrap();
    let health = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let health = control(&control_config, ControlMethod::HealthCheck).await;
            if health["event_sources"] == "ok"
                && health["components"]["mutsuki.bot.qqbot.gateway:template"]["identified"] == true
            {
                break health;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("configured QQ Gateway becomes healthy");
    assert_eq!(health["service"], "ok");
    assert_eq!(
        health["components"]["mutsuki.bot.qqbot.gateway:template"]["identified"],
        true
    );

    let tasks = control(&control_config, ControlMethod::TaskList).await;
    let task_json = tasks.to_string();
    assert!(task_json.contains("mutsuki.bot.qqbot.gateway/frame@1"));
    assert!(!task_json.contains("mutsuki.bot.command/parse@1"));
    assert!(!task_json.contains("mutsuki.bot.command/handle@1"));
    assert!(!task_json.contains("mutsuki.bot.message/send@1"));
    assert!(!task_json.contains("TEST_CLIENT_SECRET"));
    assert!(!task_json.contains("TEST_ACCESS_TOKEN"));

    runtime.shutdown().await;
    let snapshot = fake.shutdown().await;
    assert_eq!(snapshot.websocket_connections, 2);
    assert_eq!(snapshot.gateway_auth_frames[0]["op"], 2);
    assert_eq!(snapshot.gateway_auth_frames[1]["op"], 6);
    assert_eq!(snapshot.clean_closes, 1);
    assert!(TcpListener::bind(ipc_addr).await.is_ok());
}

fn product_toml(
    root: &std::path::Path,
    ipc_addr: std::net::SocketAddr,
    qq: &mutsuki_plugin_bot_adapter_qqbot::QqBotConfig,
) -> String {
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
transport = "tcp-debug"
name = "template-qqbot-fake"
tcp_debug_addr = "{}"
token = "test-token"

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
        ipc_addr,
        qq.account_id,
        qq.app_id,
        qq.client_secret_key,
        qq.token_url,
        qq.openapi_base_url,
    )
}

async fn control(config: &ServiceConfig, method: ControlMethod) -> Value {
    let client = mutsuki_service_ipc::ControlClient::new(config.into());
    let response = client.request(method, Value::Null).await.unwrap();
    assert!(response.ok, "control failed: {:?}", response.error);
    response.result.unwrap_or(Value::Null)
}
