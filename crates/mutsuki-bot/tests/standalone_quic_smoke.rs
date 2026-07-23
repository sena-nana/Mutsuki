use std::path::Path;
use std::time::Duration;

use mutsuki_bot::{
    assemble_service, build_standalone_console_from_product, load_web_console_config,
};
use mutsuki_service_config::{ConfigOverrides, ServiceConfig};
use mutsuki_web_host::WebHost;
use tempfile::tempdir;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn standalone_quic_console_reaches_runtime_control() {
    let root = tempdir().unwrap();
    let generated = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
    let cert_pem = generated.cert.pem();
    let key_pem = generated.key_pair.serialize_pem();

    let secret_path = root.path().join("product.secret.toml");
    std::fs::write(
        &secret_path,
        format!(
            "[secrets]\nWEB_CONSOLE_AUTH_TOKEN = \"test-token\"\nLINK_QUIC_CERT_PEM = \"{cert}\"\nLINK_QUIC_KEY_PEM = \"{key}\"\nLINK_QUIC_CA_CERT_PEM = \"{cert}\"\n",
            cert = escape_toml(&cert_pem),
            key = escape_toml(&key_pem),
        ),
    )
    .unwrap();

    let runtime_config_path = root.path().join("runtime.toml");
    std::fs::write(
        &runtime_config_path,
        service_toml(
            root.path(),
            &format!(
                r#"
[security]
secret_file = "{secret}"

[link.quic]
enabled = true
listen = "127.0.0.1:0"
cert_pem_key = "LINK_QUIC_CERT_PEM"
key_pem_key = "LINK_QUIC_KEY_PEM"
"#,
                secret = secret_path.to_string_lossy().replace('\\', "/")
            ),
        ),
    )
    .unwrap();

    let service = load(&runtime_config_path);
    let runtime = assemble_service(service.clone())
        .unwrap()
        .start()
        .await
        .unwrap();
    let quic_addr = runtime.quic_link_addr().expect("quic link listener");
    tokio::time::sleep(Duration::from_millis(50)).await;

    let console_config_path = root.path().join("console.toml");
    std::fs::write(
        &console_config_path,
        service_toml(
            root.path(),
            &format!(
                r#"
[security]
secret_file = "{secret}"

[web.console]
enabled = true
listen = "127.0.0.1:0"
auth_token_key = "WEB_CONSOLE_AUTH_TOKEN"
link_endpoint = "quic://{quic_addr}"
quic_server_name = "localhost"
quic_ca_cert_key = "LINK_QUIC_CA_CERT_PEM"
"#,
                secret = secret_path.to_string_lossy().replace('\\', "/"),
                quic_addr = quic_addr,
            ),
        ),
    )
    .unwrap();

    let console_service = load(&console_config_path);
    let config = load_web_console_config(&console_config_path).unwrap();
    assert!(
        config
            .link_endpoint
            .as_deref()
            .unwrap()
            .starts_with("quic://")
    );

    let (mut host, _dirs) =
        build_standalone_console_from_product(&console_config_path, &console_service).unwrap();
    host.start().await.unwrap();
    let addr = host.listen_addr().unwrap().to_string();

    let health = ws_rpc(&addr, "control", "health", "test-token")
        .await
        .expect("health");
    assert_eq!(health["service"], "ok");

    let status = ws_rpc(&addr, "control", "service_status", "test-token")
        .await
        .expect("service_status");
    assert_eq!(status["instance_id"], "template-console-test");

    host.stop().await.unwrap();
    runtime.shutdown().await;
}

#[tokio::test]
async fn link_quic_enabled_without_secrets_fails_loud() {
    let root = tempdir().unwrap();
    let secret_path = root.path().join("product.secret.toml");
    std::fs::write(&secret_path, "[secrets]\nPLACEHOLDER = \"value\"\n").unwrap();
    let config_path = root.path().join("runtime.toml");
    std::fs::write(
        &config_path,
        service_toml(
            root.path(),
            &format!(
                r#"
[security]
secret_file = "{secret}"

[link.quic]
enabled = true
listen = "127.0.0.1:0"
cert_pem_key = "LINK_QUIC_CERT_PEM"
key_pem_key = "LINK_QUIC_KEY_PEM"
"#,
                secret = secret_path.to_string_lossy().replace('\\', "/")
            ),
        ),
    )
    .unwrap();
    let service = load(&config_path);
    let error = match assemble_service(service).unwrap().start().await {
        Ok(_) => panic!("expected missing TLS identity failure"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("LINK_QUIC_CERT_PEM"), "{error}");
}

async fn ws_rpc(
    addr: &str,
    namespace: &str,
    method: &str,
    auth_token: &str,
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
    ws.send(Message::Text(
        serde_json::to_string(&WireMessage::Rpc(RpcRequest {
            id,
            namespace: namespace.into(),
            method: method.into(),
            params: serde_json::json!({"capabilities": ["runtime.read", "*"]}),
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

fn escape_toml(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
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

[observe]
console = false

[plugins]
dynamic_dirs = []
disabled_dir = "disabled"
{}
"#,
        root.to_string_lossy().replace('\\', "/"),
        extra
    )
}
