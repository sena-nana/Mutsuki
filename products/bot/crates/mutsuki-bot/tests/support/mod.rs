use std::path::{Path, PathBuf};

use futures_util::{SinkExt, StreamExt};
use mutsuki_bot::PRODUCT_CONFIG_PROVIDER_ID;
use mutsuki_bot::load_bootstrapped_product;
use mutsuki_bot_testkit::FakeQqServer;
use mutsuki_config_service::{ConfigApplyRequest, ConfigContext, ConfigValue};
use mutsuki_service_control::{HealthReport, TaskSnapshot};
use mutsuki_web_protocol::{RpcRequest, WEB_PROTOCOL_VERSION, WireMessage};
use uuid::Uuid;

pub struct ProductFixture {
    pub bootstrap_path: PathBuf,
    pub console_address: String,
    pub console_token: String,
}

pub async fn fake_qq_product(root: &Path) -> (FakeQqServer, ProductFixture) {
    let fake = FakeQqServer::start().await;
    let secret_key = "QQBOT_CLIENT_SECRET";
    let qq = fake.config("template", "TEST_APP_ID", secret_key);
    std::fs::write(
        root.join("product.secret.toml"),
        format!("[secrets]\n{secret_key} = \"TEST_CLIENT_SECRET\"\n"),
    )
    .expect("write local smoke secret");
    let bootstrap_path = root.join("bootstrap.toml");
    std::fs::write(
        &bootstrap_path,
        format!(
            r#"[host]
instance_id = "product-qqbot-fake"
home_dir = "{}"
data_dir = "data"

[security]
secret_file = "product.secret.toml"

[config_repository]
repository_plugin_id = "mutsuki.config.repository.sqlite"
document_namespace = "product-qqbot-fake"

[config_repository.options]
path = "config.sqlite3"
"#,
            root.to_string_lossy().replace('\\', "/"),
        ),
    )
    .expect("write product bootstrap");

    let first = load_bootstrapped_product(&bootstrap_path)
        .await
        .expect("load product bootstrap");
    let snapshot = first
        .config
        .read(
            PRODUCT_CONFIG_PROVIDER_ID,
            ConfigContext::global(),
            &["*".into()],
        )
        .await
        .expect("read product config");
    let mut product = snapshot.value.to_json();
    product["console_listen"] = serde_json::Value::String(free_loopback_address());
    product["runtime_plugins"]["mutsuki.bot.adapter.qqbot"] = serde_json::json!({
        "enabled": true,
        "config": qq,
    });
    first
        .config
        .apply(
            PRODUCT_CONFIG_PROVIDER_ID,
            ConfigApplyRequest {
                candidate: ConfigValue::from_json(&product),
                expected_revision: snapshot.revision,
                dry_run: false,
            },
            ConfigContext::global(),
            &["*".into()],
        )
        .await
        .expect("persist fake QQ product config");
    drop(first);

    let product = load_bootstrapped_product(&bootstrap_path)
        .await
        .expect("restore fake QQ product config");
    let console_token = product
        .service
        .host_secret_store()
        .resolve(
            product
                .console
                .auth_token_key
                .as_deref()
                .expect("console auth token key"),
        )
        .expect("console auth token");
    (
        fake,
        ProductFixture {
            bootstrap_path,
            console_address: product.console.listen,
            console_token,
        },
    )
}

fn free_loopback_address() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("reserve console port");
    listener.local_addr().expect("console address").to_string()
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
