//! overview.summary WebHost E2E.
// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(clippy::default_trait_access, clippy::doc_markdown)]

use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use mutsuki_plugin_bot_control_web::{ControlRpcCaller, FixtureControlHandler};
use mutsuki_plugin_bot_overview_web::{OverviewWebExtension, materialize_frontend_assets};
use mutsuki_web_host::{MinimalWebApplication, MutsukiWebHost, WebHost};
use mutsuki_web_protocol::{
    DeploymentMode, RpcRequest, WEB_PROTOCOL_VERSION, WebApplicationDescriptor, WebShellAssets,
    WireMessage,
};
use serde_json::json;
use uuid::Uuid;

async fn start(fail_statistics: bool) -> MutsukiWebHost {
    let assets_dir = tempfile::tempdir().unwrap();
    let shell_dir = tempfile::tempdir().unwrap();
    let assets = materialize_frontend_assets(assets_dir.path()).unwrap();
    let frontend = std::fs::read_to_string(assets.join("index.js")).unwrap();
    assert!(frontend.contains("overview.cards"));
    assert!(frontend.contains("overview-cards"));
    assert!(frontend.contains(r#"className = "card card--outlined""#));
    assert!(frontend.contains(r#"aria-label", "刷新""#));
    assert!(!frontend.contains("mountQqAccountCards"));
    assert!(!frontend.contains("qq-bot/index.js"));
    std::fs::write(
        assets.join("test-shell.html"),
        "<!doctype html><title>Mutsuki Console</title><div id=app></div>",
    )
    .unwrap();
    let fixture = FixtureControlHandler {
        fail_statistics,
        ..FixtureControlHandler::default()
    };
    let extension =
        OverviewWebExtension::new(ControlRpcCaller::new(Arc::new(fixture), "local-dev"))
            .with_frontend_assets(&assets);
    let mut host = MutsukiWebHost::builder()
        .application(MinimalWebApplication::new(
            WebApplicationDescriptor {
                id: "mutsuki.bot.overview".into(),
                name: "Overview".into(),
                version: "0.1.0".into(),
                brand: Some("Mutsuki".into()),
                theme: Some("lilia".into()),
            },
            WebShellAssets {
                root_dir: assets,
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

async fn ws_rpc(addr: &str, method: &str) -> Result<serde_json::Value, String> {
    use tokio_tungstenite::{connect_async, tungstenite::Message};
    let (mut ws, _) = connect_async(format!("ws://{addr}/ws")).await.expect("ws");
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
    let Message::Binary(ack) = ws.next().await.unwrap().unwrap() else {
        panic!("ack");
    };
    assert!(matches!(
        WireMessage::decode(ack.as_ref()).unwrap(),
        WireMessage::HelloAck { .. }
    ));
    let id = Uuid::new_v4();
    ws.send(Message::Binary(
        WireMessage::Rpc(RpcRequest {
            id,
            namespace: "overview".into(),
            method: method.into(),
            params: json!({}),
        })
        .encode()
        .unwrap()
        .into(),
    ))
    .await
    .unwrap();
    let Message::Binary(bytes) = ws.next().await.unwrap().unwrap() else {
        panic!("rpc");
    };
    match WireMessage::decode(bytes.as_ref()).unwrap() {
        WireMessage::RpcResult(result) => match result.error {
            Some(error) => Err(error.message),
            None => Ok(result.result.unwrap_or(serde_json::Value::Null)),
        },
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test]
async fn overview_summary() {
    let mut host = start(false).await;
    let addr = host.listen_addr().unwrap().to_string();
    let summary = ws_rpc(&addr, "summary").await.unwrap();
    assert_eq!(summary["service"]["instance_id"], "demo");
    assert_eq!(summary["counts"]["runners"], 1);
    assert_eq!(summary["counts"]["tasks"]["running"], 2);
    assert_eq!(summary["host"]["unavailable"], false);
    assert_eq!(summary["host"]["available"], true);
    assert_eq!(summary["host"]["pid"], 4242);
    assert_eq!(summary["host"]["rss_bytes"], 64 * 1024 * 1024);
    assert!(
        summary["plugins"]["plugins"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["plugin_id"] == "demo.plugin")
    );
    host.stop().await.unwrap();
}

#[tokio::test]
async fn overview_summary_without_core_statistics() {
    let mut host = start(true).await;
    let addr = host.listen_addr().unwrap().to_string();
    let summary = ws_rpc(&addr, "summary").await.unwrap();
    assert!(summary["counts"]["tasks"].is_null());
    host.stop().await.unwrap();
}
