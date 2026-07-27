use std::path::Path;

use mutsuki_bot::{WebConsoleGuard, assemble_service, load_web_console_config};
use mutsuki_service_config::{ConfigOverrides, ServiceConfig};
use tempfile::tempdir;

#[tokio::test]
async fn web_console_disabled_by_default_in_template() {
    let root = tempdir().unwrap();
    let config_path = root.path().join("product.toml");
    std::fs::write(&config_path, service_toml(root.path(), "")).unwrap();
    let config = load_web_console_config(&config_path).unwrap();
    assert!(!config.enabled);
}

#[tokio::test]
async fn enabled_web_console_reaches_overview_summary() {
    let root = tempdir().unwrap();
    let secret_path = root.path().join("product.secret.toml");
    std::fs::write(
        &secret_path,
        "[secrets]\nWEB_CONSOLE_AUTH_TOKEN = \"console-test-token\"\n",
    )
    .unwrap();
    let secret_path = secret_path.to_string_lossy().replace('\\', "/");
    let config_path = root.path().join("product.toml");
    std::fs::write(
        &config_path,
        service_toml(
            root.path(),
            &format!(
                r#"
[security]
secret_file = "{secret_path}"

[web.console]
enabled = true
listen = "127.0.0.1:0"
auth_token_key = "WEB_CONSOLE_AUTH_TOKEN"
release_set = "releases/mutsuki-0.1-alpha-3.toml"
"#
            ),
        ),
    )
    .unwrap();
    std::fs::create_dir_all(root.path().join("releases")).unwrap();
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../releases/mutsuki-0.1-alpha-3.toml"),
        root.path().join("releases/mutsuki-0.1-alpha-3.toml"),
    )
    .unwrap();

    let service = load(&config_path);
    let runtime = assemble_service(service.clone())
        .unwrap()
        .start()
        .await
        .unwrap();
    let console = WebConsoleGuard::start(&config_path, &service, &runtime)
        .await
        .unwrap()
        .expect("console should start");
    let addr = console.listen_addr().expect("listen addr");

    let summary = ws_rpc(
        &addr.to_string(),
        "overview",
        "summary",
        "console-test-token",
    )
    .await
    .expect("overview summary");
    assert_eq!(summary["service"]["profile"], "test");

    let plugins = ws_rpc(
        &addr.to_string(),
        "control",
        "plugin_list",
        "console-test-token",
    )
    .await
    .expect("plugin list");
    assert!(plugins["plugins"].is_array());

    let secret_status = ws_rpc(&addr.to_string(), "secret", "status", "console-test-token")
        .await
        .expect("secret status");
    assert_eq!(secret_status["secrets"][0]["key"], "WEB_CONSOLE_AUTH_TOKEN");
    assert_eq!(secret_status["secrets"][0]["state"], "present");

    let logs = ws_rpc_with_params(
        &addr.to_string(),
        "control",
        "log_tail",
        "console-test-token",
        serde_json::json!({"lines": 10}),
    )
    .await
    .expect("log tail");
    assert!(logs["entries"].is_array());

    console.stop().await.unwrap();
    runtime.shutdown().await;
}

async fn ws_rpc(
    addr: &str,
    namespace: &str,
    method: &str,
    auth_token: &str,
) -> Result<serde_json::Value, String> {
    ws_rpc_with_params(addr, namespace, method, auth_token, serde_json::json!({})).await
}

async fn ws_rpc_with_params(
    addr: &str,
    namespace: &str,
    method: &str,
    auth_token: &str,
    extra_params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    use futures_util::{SinkExt, StreamExt};
    use mutsuki_web_protocol::{RpcRequest, WEB_PROTOCOL_VERSION, WireMessage};
    use tokio_tungstenite::{connect_async, tungstenite::Message};
    use uuid::Uuid;

    let (mut ws, _) = connect_async(format!("ws://{addr}/ws"))
        .await
        .map_err(|err| err.to_string())?;
    ws.send(Message::Text(
        serde_json::to_string(&WireMessage::Hello {
            protocol_version: WEB_PROTOCOL_VERSION.into(),
            capabilities: vec!["runtime.read".into(), "*".into()],
            auth_token: Some(auth_token.into()),
        })
        .unwrap()
        .into(),
    ))
    .await
    .map_err(|err| err.to_string())?;
    let Message::Text(ack) = ws.next().await.unwrap().unwrap() else {
        return Err("missing hello ack".into());
    };
    assert!(matches!(
        serde_json::from_str::<WireMessage>(&ack).unwrap(),
        WireMessage::HelloAck { .. }
    ));
    let id = Uuid::new_v4();
    let mut params = extra_params;
    if let Some(obj) = params.as_object_mut() {
        obj.entry("capabilities")
            .or_insert(serde_json::json!(["runtime.read", "*"]));
    }
    ws.send(Message::Text(
        serde_json::to_string(&WireMessage::Rpc(RpcRequest {
            id,
            namespace: namespace.into(),
            method: method.into(),
            params,
        }))
        .unwrap()
        .into(),
    ))
    .await
    .map_err(|err| err.to_string())?;
    let Message::Text(text) = ws.next().await.unwrap().unwrap() else {
        return Err("missing rpc result".into());
    };
    match serde_json::from_str::<WireMessage>(&text).unwrap() {
        WireMessage::RpcResult(result) => match result.error {
            Some(error) => Err(error.message),
            None => Ok(result.result.unwrap_or(serde_json::Value::Null)),
        },
        other => Err(format!("unexpected wire message: {other:?}")),
    }
}

fn load(path: &Path) -> ServiceConfig {
    ServiceConfig::load(ConfigOverrides {
        config_file: Some(path.to_path_buf()),
        ..Default::default()
    })
    .unwrap()
}

fn service_toml(root: &Path, extra: &str) -> String {
    format!(
        r#"[service]
profile = "test"
instance_id = "template-console-test"
home_dir = "{}"
data_dir = "data"
log_dir = "logs"
plugin_dir = "plugins"
run_dir = "run"

[ipc]
enabled = false
transport = "named-pipe"
name = "template-console-test"
token = "test-token"

[plugins]
dynamic_dirs = []
disabled_dir = "disabled"
{}
"#,
        root.to_string_lossy().replace('\\', "/"),
        extra
    )
}
