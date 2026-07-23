//! WebHost fixture E2E for overview.* RPC.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use mutsuki_bot_config::{
    ConfigApplyMode, ConfigProviderRegistry, ConfigService, ConfigValue, MemoryConfigProvider,
    MutsukiConfig, MutsukiConfigSchema, SecretState,
};
use mutsuki_plugin_bot_config_web::ConfigWebExtension;
use mutsuki_plugin_bot_overview_web::{
    FixtureControlHandler, OverviewWebExtension, materialize_frontend_assets,
};
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

async fn start(fail_statistics: bool) -> MutsukiWebHost {
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
    let config_service = Arc::new(ConfigService::new(registry));

    let assets_dir = tempfile::tempdir().unwrap();
    let shell_dir = tempfile::tempdir().unwrap();
    let assets = materialize_frontend_assets(assets_dir.path()).unwrap();

    let mut fixture = FixtureControlHandler::default();
    fixture.fail_statistics = fail_statistics;
    let overview =
        OverviewWebExtension::new(Arc::new(fixture), "local-dev").with_frontend_assets(&assets);
    let config = ConfigWebExtension::new(config_service).with_frontend_assets(&assets);

    let mut host = MutsukiWebHost::builder()
        .application(MinimalWebApplication::new(
            WebApplicationDescriptor {
                id: "mutsuki.bot.console".into(),
                name: "Mutsuki Console".into(),
                version: "0.1.0".into(),
                brand: Some("Mutsuki".into()),
                theme: Some("lilia".into()),
            },
            WebShellAssets {
                root_dir: assets.clone(),
                index_file: "index.html".into(),
                import_map: Default::default(),
            },
        ))
        .listen("127.0.0.1:0")
        .mode(DeploymentMode::Embedded)
        .shell_dir(shell_dir.path())
        .extension(overview)
        .extension(config)
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
) -> Result<serde_json::Value, String> {
    use tokio_tungstenite::{connect_async, tungstenite::Message};
    let url = format!("ws://{addr}/ws");
    let (mut ws, _) = connect_async(url).await.expect("ws connect");
    ws.send(Message::Text(
        serde_json::to_string(&WireMessage::Hello {
            protocol_version: WEB_PROTOCOL_VERSION.into(),
            capabilities: vec![],
            auth_token: Some("local-dev".into()),
        })
        .unwrap()
        .into(),
    ))
    .await
    .unwrap();
    let ack = ws.next().await.unwrap().unwrap();
    let Message::Text(text) = ack else {
        panic!("expected text")
    };
    let hello: WireMessage = serde_json::from_str(&text).unwrap();
    let WireMessage::HelloAck { .. } = hello else {
        panic!("expected hello ack")
    };

    let id = Uuid::new_v4();
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
    .unwrap();
    let resp = ws.next().await.unwrap().unwrap();
    let Message::Text(text) = resp else {
        panic!("expected rpc text")
    };
    let msg: WireMessage = serde_json::from_str(&text).unwrap();
    match msg {
        WireMessage::RpcResult(result) => {
            if let Some(error) = result.error {
                Err(error.message)
            } else {
                Ok(result.result.unwrap_or(serde_json::Value::Null))
            }
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test]
async fn overview_summary_and_structure() {
    let mut host = start(false).await;
    let addr = host.listen_addr().unwrap().to_string();

    let mut stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();
    let body = String::from_utf8_lossy(&buf);
    assert!(body.contains("Mutsuki Console") || body.contains("lilia-tokens"));

    let summary = ws_rpc(&addr, "overview", "summary", json!({}))
        .await
        .unwrap();
    assert_eq!(summary["service"]["instance_id"], "demo");
    assert_eq!(summary["counts"]["runners"], 1);
    assert_eq!(summary["counts"]["tasks"]["running"], 2);
    assert_eq!(summary["uptime_ms"], 12345);
    assert!(summary["event_sources"].as_array().unwrap().len() >= 1);

    let structure = ws_rpc(&addr, "overview", "structure", json!({}))
        .await
        .unwrap();
    assert!(
        structure["plugins"]["plugins"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["plugin_id"] == "demo.plugin")
    );
    assert!(
        structure["component_ids"]
            .as_array()
            .unwrap()
            .iter()
            .any(|id| id.as_str().unwrap().contains("qqbot.gateway"))
    );

    let tasks = ws_rpc(&addr, "overview", "tasks.summary", json!({}))
        .await
        .unwrap();
    assert_eq!(tasks["ready"], 1);

    host.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test]
async fn overview_summary_tolerates_core_stopped_statistics() {
    let mut host = start(true).await;
    let addr = host.listen_addr().unwrap().to_string();
    let summary = ws_rpc(&addr, "overview", "summary", json!({}))
        .await
        .unwrap();
    assert_eq!(summary["service"]["instance_id"], "demo");
    assert!(summary["counts"]["tasks"].is_null());
    host.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test]
async fn overview_rejects_unauthorized_control_token_path() {
    // Fixture only accepts local-dev/fixture; build extension with wrong token.
    let assets_dir = tempfile::tempdir().unwrap();
    let shell_dir = tempfile::tempdir().unwrap();
    let assets = materialize_frontend_assets(assets_dir.path()).unwrap();
    let overview =
        OverviewWebExtension::new(Arc::new(FixtureControlHandler::default()), "wrong-token")
            .with_frontend_assets(&assets);
    let mut host = MutsukiWebHost::builder()
        .application(MinimalWebApplication::new(
            WebApplicationDescriptor {
                id: "mutsuki.bot.console".into(),
                name: "Mutsuki Console".into(),
                version: "0.1.0".into(),
                brand: Some("Mutsuki".into()),
                theme: Some("lilia".into()),
            },
            WebShellAssets {
                root_dir: assets.clone(),
                index_file: "index.html".into(),
                import_map: Default::default(),
            },
        ))
        .listen("127.0.0.1:0")
        .mode(DeploymentMode::Embedded)
        .shell_dir(shell_dir.path())
        .extension(overview)
        .auth_token("local-dev")
        .build()
        .unwrap();
    host.start().await.unwrap();
    let addr = host.listen_addr().unwrap().to_string();
    let err = ws_rpc(&addr, "overview", "summary", json!({}))
        .await
        .expect_err("unauthorized control token");
    assert!(
        err.contains("unauthorized") || err.contains("failed"),
        "unexpected error: {err}"
    );
    host.stop().await.unwrap();
    std::mem::forget(assets_dir);
    std::mem::forget(shell_dir);
}
