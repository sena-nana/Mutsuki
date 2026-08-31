use std::path::{Path, PathBuf};

use futures::{SinkExt, StreamExt};
use mutsuki_bot::{CONSOLE_AUTH_TOKEN_KEY, load_single_instance_product_for_test};
use mutsuki_bot_testkit::{FakeQqGatewayScript, FakeQqServer};
use mutsuki_config_service::{ConfigApplyRequest, ConfigContext};
use mutsuki_plugin_bot_adapter_qqbot::{QQBOT_ADAPTER_PLUGIN_ID, qq_config_value};
use mutsuki_service_control::{HealthReport, TaskSnapshot};
use mutsuki_web_protocol::{RpcRequest, WEB_PROTOCOL_VERSION, WireMessage};
use uuid::Uuid;

pub struct ProductFixture {
    pub executable_path: PathBuf,
    pub console_address: String,
    pub console_token: String,
    _console_permit: tokio::sync::OwnedSemaphorePermit,
}

pub async fn fake_qq_product(root: &Path) -> (FakeQqServer, ProductFixture) {
    fake_qq_product_with_script(root, FakeQqGatewayScript::default()).await
}

pub async fn fake_qq_product_with_script(
    root: &Path,
    script: FakeQqGatewayScript,
) -> (FakeQqServer, ProductFixture) {
    let console_permit = console_semaphore()
        .acquire_owned()
        .await
        .expect("single-instance console semaphore");
    let fake = FakeQqServer::start_with_gateway_script(script).await;
    let secret_key = "QQBOT_CLIENT_SECRET";
    let qq = fake.config("template", "TEST_APP_ID", secret_key);
    let executable_path = copy_product_executable(root);
    let instance_root = root.join(".mutsuki-bot");
    std::fs::create_dir_all(&instance_root).expect("create single-instance root");
    let console_token = "test-console-token".to_string();
    std::fs::write(
        instance_root.join("secrets.toml"),
        format!(
            "[secrets]\n\"{CONSOLE_AUTH_TOKEN_KEY}\" = \"{console_token}\"\n{secret_key} = \"TEST_CLIENT_SECRET\"\n"
        ),
    )
    .expect("write local smoke secret");

    let first = load_single_instance_product_for_test(&instance_root, "unused")
        .await
        .expect("load single-instance product");
    let qq_snapshot = first
        .config
        .read(
            QQBOT_ADAPTER_PLUGIN_ID,
            ConfigContext::global(),
            &["*".into()],
        )
        .await
        .expect("read QQ owner config");
    first
        .config
        .apply(
            QQBOT_ADAPTER_PLUGIN_ID,
            ConfigApplyRequest {
                candidate: qq_config_value(true, &qq),
                expected_revision: qq_snapshot.revision,
                dry_run: false,
            },
            ConfigContext::global(),
            &["*".into()],
        )
        .await
        .expect("persist fake QQ owner config");
    drop(first);

    let product = load_single_instance_product_for_test(&instance_root, "unused")
        .await
        .expect("restore fake QQ product config");
    (
        fake,
        ProductFixture {
            executable_path,
            console_address: product.console.listen,
            console_token,
            _console_permit: console_permit,
        },
    )
}

fn console_semaphore() -> std::sync::Arc<tokio::sync::Semaphore> {
    static CONSOLE: std::sync::OnceLock<std::sync::Arc<tokio::sync::Semaphore>> =
        std::sync::OnceLock::new();
    CONSOLE
        .get_or_init(|| std::sync::Arc::new(tokio::sync::Semaphore::new(1)))
        .clone()
}

fn copy_product_executable(root: &Path) -> PathBuf {
    let source = Path::new(env!("CARGO_BIN_EXE_mutsuki-bot"));
    let file_name = source.file_name().expect("product executable file name");
    let target = root.join(file_name);
    std::fs::copy(source, &target).expect("copy product executable beside fixture data");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&target).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&target, permissions).unwrap();
    }
    target
}

pub fn gateway_ready(health: &HealthReport) -> bool {
    health.event_sources == "ok"
        && health.components["mutsuki.bot.qqbot.gateway:template"]["identified"] == true
}

pub fn assert_gateway_health(health: &HealthReport) {
    assert_eq!(health.service, "ok");
    assert!(gateway_ready(health));
}

pub fn assert_gateway_only_task_surface(tasks: &[TaskSnapshot]) {
    assert!(
        tasks
            .iter()
            .any(|task| task.protocol_id == "mutsuki.bot.qqbot.gateway/frame@1")
    );
    for forbidden in [
        "mutsuki.bot.command/parse@1",
        "mutsuki.bot.command/handle@1",
        "mutsuki.bot.message/send@1",
    ] {
        assert!(tasks.iter().all(|task| task.protocol_id != forbidden));
    }
    let encoded = serde_json::to_string(tasks).expect("encode task snapshots");
    assert!(!encoded.contains("TEST_CLIENT_SECRET"));
    assert!(!encoded.contains("TEST_ACCESS_TOKEN"));
}

pub async fn try_health(fixture: &ProductFixture) -> Result<HealthReport, String> {
    serde_json::from_value(rpc(fixture, "health").await?).map_err(|error| error.to_string())
}

pub async fn task_list(fixture: &ProductFixture) -> Vec<TaskSnapshot> {
    serde_json::from_value(rpc(fixture, "task_list").await.unwrap()).unwrap()
}

pub async fn shutdown(fixture: &ProductFixture) -> Result<(), String> {
    rpc(fixture, "service_shutdown").await.map(drop)
}

async fn rpc(fixture: &ProductFixture, method: &str) -> Result<serde_json::Value, String> {
    use tokio_tungstenite::{connect_async, tungstenite::Message};
    let (mut socket, _) = connect_async(format!("ws://{}/ws", fixture.console_address))
        .await
        .map_err(|error| error.to_string())?;
    socket
        .send(Message::Binary(
            WireMessage::Hello {
                protocol_version: WEB_PROTOCOL_VERSION.into(),
                capabilities: Vec::new(),
                auth_token: Some(fixture.console_token.clone()),
            }
            .encode()
            .map_err(|error| error.to_string())?
            .into(),
        ))
        .await
        .map_err(|error| error.to_string())?;
    let _ = socket.next().await.ok_or("missing hello ack")?;
    socket
        .send(Message::Binary(
            WireMessage::Rpc(RpcRequest {
                id: Uuid::new_v4(),
                namespace: "control".into(),
                method: method.into(),
                params: serde_json::json!({}),
            })
            .encode()
            .map_err(|error| error.to_string())?
            .into(),
        ))
        .await
        .map_err(|error| error.to_string())?;
    let message = socket
        .next()
        .await
        .ok_or("missing RPC response")?
        .map_err(|error| error.to_string())?;
    let Message::Binary(bytes) = message else {
        return Err("unexpected WebSocket response".into());
    };
    match WireMessage::decode(bytes.as_ref()).map_err(|error| error.to_string())? {
        WireMessage::RpcResult(result) => result.error.map_or_else(
            || Ok(result.result.unwrap_or_default()),
            |error| Err(error.message),
        ),
        _ => Err("unexpected Web RPC response".into()),
    }
}
