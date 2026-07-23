//! overview.summary WebHost E2E.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use mutsuki_plugin_bot_overview_web::{
    FixtureControlHandler, OverviewWebExtension, materialize_frontend_assets,
};
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
    let mut fixture = FixtureControlHandler::default();
    fixture.fail_statistics = fail_statistics;
    let extension =
        OverviewWebExtension::new(Arc::new(fixture), "local-dev").with_frontend_assets(&assets);
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
                index_file: "index.html".into(),
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
    let Message::Text(ack) = ws.next().await.unwrap().unwrap() else {
        panic!("ack");
    };
    assert!(matches!(
        serde_json::from_str::<WireMessage>(&ack).unwrap(),
        WireMessage::HelloAck { .. }
    ));
    let id = Uuid::new_v4();
    ws.send(Message::Text(
        serde_json::to_string(&WireMessage::Rpc(RpcRequest {
            id,
            namespace: "overview".into(),
            method: method.into(),
            params: json!({}),
        }))
        .unwrap()
        .into(),
    ))
    .await
    .unwrap();
    let Message::Text(text) = ws.next().await.unwrap().unwrap() else {
        panic!("rpc");
    };
    match serde_json::from_str::<WireMessage>(&text).unwrap() {
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
    assert!(
        summary["plugins"]["plugins"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["plugin_id"] == "demo.plugin")
    );
    host.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test]
async fn overview_summary_without_core_statistics() {
    let mut host = start(true).await;
    let addr = host.listen_addr().unwrap().to_string();
    let summary = ws_rpc(&addr, "summary").await.unwrap();
    assert!(summary["counts"]["tasks"].is_null());
    host.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
}
