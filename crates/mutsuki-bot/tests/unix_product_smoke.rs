#![cfg(unix)]

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use mutsuki_bot_testkit::FakeQqServer;
use mutsuki_service_config::{ConfigOverrides, ServiceConfig};
use mutsuki_service_control::ControlMethod;
use serde_json::Value;
use tempfile::Builder;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn production_binary_runs_fake_qq_over_unix_ipc_and_shuts_down_cleanly() {
    let fake = FakeQqServer::start().await;
    let secret_key = format!("TEMPLATE_QQ_SECRET_{}", fake.websocket_addr().port());
    let root = Builder::new()
        .prefix("mtk-bot-")
        .tempdir_in("/tmp")
        .expect("short Unix smoke directory");
    let qq = fake.config("template", "TEST_APP_ID", &secret_key);
    std::fs::write(
        root.path().join("product.secret.toml"),
        format!("[secrets]\n{secret_key} = \"TEST_CLIENT_SECRET\"\n"),
    )
    .expect("write local smoke secret");
    let config_path = root.path().join("product.toml");
    std::fs::write(&config_path, product_toml(root.path(), &qq))
        .expect("write product smoke config");
    let service = ServiceConfig::load(ConfigOverrides {
        config_file: Some(config_path.clone()),
        ..Default::default()
    })
    .expect("load product smoke config");
    let socket_path = PathBuf::from(service.ipc_endpoint());
    let mut process = ProductProcess::spawn(&config_path, root.path().join("product.log"));

    let health = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            process.assert_running();
            if let Ok(health) = try_control(&service, ControlMethod::HealthCheck).await
                && health["event_sources"] == "ok"
                && health["components"]["mutsuki.bot.qqbot.gateway:template"]["identified"] == true
            {
                break health;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("product did not become healthy: {}", process.diagnostics()));
    assert_eq!(health["service"], "ok");
    assert_eq!(
        health["components"]["mutsuki.bot.qqbot.gateway:template"]["identified"],
        true
    );

    let tasks = control(&service, ControlMethod::TaskList).await;
    let task_json = tasks.to_string();
    assert!(task_json.contains("mutsuki.bot.qqbot.gateway/frame@1"));
    assert!(!task_json.contains("mutsuki.bot.command/parse@1"));
    assert!(!task_json.contains("mutsuki.bot.command/handle@1"));
    assert!(!task_json.contains("mutsuki.bot.message/send@1"));
    assert!(!task_json.contains("TEST_CLIENT_SECRET"));
    assert!(!task_json.contains("TEST_ACCESS_TOKEN"));

    control(&service, ControlMethod::ServiceShutdown).await;
    let status = process.wait_for_exit(Duration::from_secs(30)).await;
    assert!(
        status.success(),
        "product exited with {status}: {}",
        process.diagnostics()
    );
    assert!(!socket_path.exists(), "Unix socket survived process exit");

    let snapshot = fake.shutdown().await;
    assert_eq!(snapshot.websocket_connections, 2);
    assert_eq!(snapshot.gateway_auth_frames[0]["op"], 2);
    assert_eq!(snapshot.gateway_auth_frames[1]["op"], 6);
    assert_eq!(snapshot.clean_closes, 1);
}

struct ProductProcess {
    child: Child,
    log_path: PathBuf,
}

impl ProductProcess {
    fn spawn(config_path: &Path, log_path: PathBuf) -> Self {
        let output = File::create(&log_path).expect("create product process log");
        let error = output.try_clone().expect("clone product process log");
        let child = Command::new(env!("CARGO_BIN_EXE_mutsuki-bot"))
            .arg(config_path)
            .stdin(Stdio::null())
            .stdout(Stdio::from(output))
            .stderr(Stdio::from(error))
            .spawn()
            .expect("start production mutsuki-bot binary");
        Self { child, log_path }
    }

    fn assert_running(&mut self) {
        if let Some(status) = self.child.try_wait().expect("inspect product process") {
            panic!(
                "product exited before becoming healthy with {status}: {}",
                self.diagnostics()
            );
        }
    }

    async fn wait_for_exit(&mut self, timeout: Duration) -> ExitStatus {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait().expect("inspect product process") {
                return status;
            }
            if Instant::now() >= deadline {
                panic!(
                    "product did not exit after shutdown: {}",
                    self.diagnostics()
                );
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    fn diagnostics(&self) -> String {
        std::fs::read_to_string(&self.log_path)
            .unwrap_or_else(|error| format!("failed to read process log: {error}"))
    }
}

impl Drop for ProductProcess {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

fn product_toml(root: &Path, qq: &mutsuki_plugin_bot_adapter_qqbot::QqBotConfig) -> String {
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
transport = "unix-socket"
name = "bot"
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
        qq.account_id,
        qq.app_id,
        qq.client_secret_key,
        qq.token_url,
        qq.openapi_base_url,
    )
}

async fn try_control(config: &ServiceConfig, method: ControlMethod) -> Result<Value, String> {
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

async fn control(config: &ServiceConfig, method: ControlMethod) -> Value {
    try_control(config, method).await.unwrap()
}
