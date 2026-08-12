//! WebHost fixture E2E for config.* RPC (Issue #15 Phase 4).

use std::sync::Arc;
use std::time::Duration;

use mutsuki_config_service::{
    ConfigApplyMode, ConfigProviderRegistry, ConfigService, ConfigValue, MemoryConfigProvider,
    MutsukiConfig, MutsukiConfigSchema, SecretState,
};
use mutsuki_plugin_config_web::{ConfigWebExtension, materialize_frontend_assets};
use mutsuki_web_host::{MinimalWebApplication, MutsukiWebHost, WebHost};
use mutsuki_web_protocol::{
    DeploymentMode, RpcRequest, WEB_PROTOCOL_VERSION, WebApplicationDescriptor, WebShellAssets,
    WireMessage,
};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

#[derive(MutsukiConfig)]
#[config(provider_id = "demo", title = "Demo")]
#[allow(dead_code)]
struct DemoConfig {
    #[config(title = "Name", required)]
    name: String,
    #[config(title = "Token", secret)]
    token: String,
}

async fn start() -> MutsukiWebHost {
    let registry = Arc::new(ConfigProviderRegistry::default());
    let defaults = ConfigValue::Object(
        [
            ("name".into(), ConfigValue::String("demo".into())),
            ("token".into(), ConfigValue::Secret(SecretState::Absent)),
        ]
        .into_iter()
        .collect(),
    );
    registry
        .register(Arc::new(MemoryConfigProvider::new(
            DemoConfig::schema(),
            defaults,
            ConfigApplyMode::HotReload,
        )))
        .unwrap();
    let service = Arc::new(
        ConfigService::new(
            registry,
            Arc::new(mutsuki_config_service::InMemoryConfigRepository::default()),
        )
        .unwrap(),
    );
    let assets_dir = tempfile::tempdir().unwrap();
    let shell_dir = tempfile::tempdir().unwrap();
    let assets = materialize_frontend_assets(assets_dir.path()).unwrap();
    std::fs::write(
        assets.join("test-shell.html"),
        "<!doctype html><title>Mutsuki Console</title><div id=app></div>",
    )
    .unwrap();
    let extension = ConfigWebExtension::new(service).with_frontend_assets(&assets);
    let mut host = MutsukiWebHost::builder()
        .application(MinimalWebApplication::new(
            WebApplicationDescriptor {
                id: "mutsuki.bot.config.console".into(),
                name: "Config Console".into(),
                version: "0.1.0".into(),
                brand: Some("Mutsuki".into()),
                theme: Some("lilia".into()),
            },
            WebShellAssets {
                root_dir: assets.clone(),
                index_file: "test-shell.html".into(),
                import_map: Default::default(),
            },
        ))
        .listen("127.0.0.1:0")
        .mode(DeploymentMode::Embedded)
        .shell_dir(shell_dir.path())
        .extension(extension)
        .auth_token("local-dev")
        .build()
        .unwrap();
    host.start().await.unwrap();
    std::mem::forget(assets_dir);
    std::mem::forget(shell_dir);
    host
}

async fn ws_rpc(
    addr: &str,
    namespace: &str,
    method: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    // Use the same binary MessagePack WebSocket frames as the browser SDK.
    use tokio_tungstenite::{connect_async, tungstenite::Message};
    let url = format!("ws://{addr}/ws");
    let (mut ws, _) = connect_async(url).await.expect("ws connect");
    ws.send(Message::Binary(
        WireMessage::Hello {
            protocol_version: WEB_PROTOCOL_VERSION.into(),
            capabilities: vec![],
            auth_token: Some("local-dev".into()),
        }
        .encode()
        .unwrap()
        .into(),
    ))
    .await
    .unwrap();
    let ack = ws.next().await.unwrap().unwrap();
    let Message::Binary(bytes) = ack else {
        panic!("expected binary")
    };
    let hello = WireMessage::decode(bytes.as_ref()).unwrap();
    let WireMessage::HelloAck { .. } = hello else {
        panic!("expected hello ack")
    };

    let id = Uuid::new_v4();
    ws.send(Message::Binary(
        WireMessage::Rpc(RpcRequest {
            id,
            namespace: namespace.into(),
            method: method.into(),
            params,
        })
        .encode()
        .unwrap()
        .into(),
    ))
    .await
    .unwrap();
    let resp = ws.next().await.unwrap().unwrap();
    let Message::Binary(bytes) = resp else {
        panic!("expected rpc binary")
    };
    let msg = WireMessage::decode(bytes.as_ref()).unwrap();
    match msg {
        WireMessage::RpcResult(result) => {
            assert!(result.error.is_none(), "{:?}", result.error);
            result.result.unwrap_or(serde_json::Value::Null)
        }
        other => panic!("unexpected {other:?}"),
    }
}

// Re-export StreamExt for ws.next()
use futures_util::{SinkExt, StreamExt};

#[tokio::test]
async fn config_rpc_list_schema_read_validate_apply() {
    let mut host = start().await;
    let addr = host.listen_addr().unwrap().to_string();

    // Shell is served.
    let mut stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();
    let body = String::from_utf8_lossy(&buf);
    assert!(body.contains("Mutsuki Console") || body.contains("mutsuki-ui"));

    let providers = ws_rpc(&addr, "config", "providers.list", json!({})).await;
    assert!(providers.to_string().contains("demo"));

    let schema = ws_rpc(&addr, "config", "schema.get", json!({"provider_id":"demo"})).await;
    assert_eq!(schema["provider_id"], "demo");

    let snap = ws_rpc(
        &addr,
        "config",
        "snapshot.read",
        json!({
            "provider_id":"demo",
            "context":{"scope":"mutsuki.plugin.instance","qualifiers":{"plugin_instance_id":"demo"}}
        }),
    )
    .await;
    assert_eq!(snap["revision"], 0);

    let apply = ws_rpc(
        &addr,
        "config",
        "apply",
        json!({
            "provider_id":"demo",
            "context":{"scope":"mutsuki.plugin.instance","qualifiers":{"plugin_instance_id":"demo"}},
            "request":{
                "expected_revision": 0,
                "dry_run": false,
                "candidate": {
                    "type":"object",
                    "value":{
                        "name":{"type":"string","value":"updated"},
                        "token":{"type":"secret","value":{"state":"set","value":"s3cr3t"}}
                    }
                }
            }
        }),
    )
    .await;
    assert_eq!(apply["applied"], true);

    let snap2 = ws_rpc(
        &addr,
        "config",
        "snapshot.read",
        json!({
            "provider_id":"demo",
            "context":{"scope":"mutsuki.plugin.instance","qualifiers":{"plugin_instance_id":"demo"}}
        }),
    )
    .await;
    let token = &snap2["value"]["value"]["token"];
    assert_eq!(token["type"], "secret");
    assert_eq!(token["value"]["state"], "configured");
    assert!(!snap2.to_string().contains("s3cr3t"));

    host.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
}
