use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use mutsuki_plugin_bot_sandbox_web::*;
use mutsuki_web_host::{MinimalWebApplication, MutsukiWebHost, WebHost};
use mutsuki_web_protocol::{
    DeploymentMode, RpcRequest, WEB_PROTOCOL_VERSION, WebApplicationDescriptor, WebShellAssets,
    WireMessage,
};
use serde_json::{Value, json};
use uuid::Uuid;

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn sandbox_rpc_simulate_send_and_confirm_live_send() {
    let api = Arc::new(SandboxService::new());
    let assets_dir = tempfile::tempdir().unwrap();
    let shell_dir = tempfile::tempdir().unwrap();
    let assets = materialize_frontend_assets(assets_dir.path()).unwrap();
    let frontend = std::fs::read_to_string(assets.join("index.js")).unwrap();
    assert!(frontend.contains("模拟模式"));
    assert!(frontend.contains("发送到 QQ"));
    std::fs::write(
        shell_dir.path().join("index.html"),
        "<!doctype html><main></main>",
    )
    .unwrap();
    let mut host = MutsukiWebHost::builder()
        .application(MinimalWebApplication::new(
            WebApplicationDescriptor {
                id: "mutsuki.bot.sandbox".into(),
                name: "Sandbox".into(),
                version: "0.1.0".into(),
                brand: Some("Mutsuki".into()),
                theme: Some("lilia".into()),
            },
            WebShellAssets {
                root_dir: shell_dir.path().into(),
                index_file: "index.html".into(),
                import_map: serde_json::Map::default(),
            },
        ))
        .listen("127.0.0.1:0")
        .mode(DeploymentMode::Embedded)
        .shell_dir(shell_dir.path())
        .extension(SandboxWebExtension::new(api).with_frontend_assets(&assets))
        .auth_token("local-dev")
        .build()
        .unwrap();
    host.start().await.unwrap();
    let address = host.listen_addr().unwrap().to_string();

    let snapshot = rpc(&address, "snapshot", json!({})).await.unwrap();
    assert_eq!(snapshot["mode"], "simulate");
    let conversation_id = snapshot["conversations"][0]["conversation_id"]
        .as_str()
        .unwrap();
    let user_id = snapshot["conversations"][0]["users"][0]["user_id"]
        .as_str()
        .unwrap();
    let written = rpc(
        &address,
        "write",
        json!({
            "request": {
                "expected_revision": snapshot["revision"],
                "action": {
                    "action": "ingest_as_user",
                    "conversation_id": conversation_id,
                    "user_id": user_id,
                    "text": "沙盒你好"
                }
            }
        }),
    )
    .await
    .unwrap();
    let messages = rpc(
        &address,
        "messages",
        json!({ "conversation_id": conversation_id }),
    )
    .await
    .unwrap();
    assert!(
        messages
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["text"] == "沙盒你好")
    );

    let switched = rpc(
        &address,
        "write",
        json!({
            "request": {
                "expected_revision": written["revision"],
                "action": { "action": "set_mode", "mode": "live" }
            }
        }),
    )
    .await
    .unwrap();
    assert!(
        rpc(
            &address,
            "write",
            json!({
                "confirmed": false,
                "request": {
                    "expected_revision": switched["revision"],
                    "action": {
                        "action": "send_as_bot",
                        "conversation_id": conversation_id,
                        "text": "后台消息"
                    }
                }
            }),
        )
        .await
        .is_err()
    );

    host.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
}

async fn rpc(address: &str, method: &str, params: Value) -> Result<Value, String> {
    use tokio_tungstenite::{connect_async, tungstenite::Message};
    let (mut socket, _) = connect_async(format!("ws://{address}/ws"))
        .await
        .map_err(|error| error.to_string())?;
    socket
        .send(Message::Binary(
            WireMessage::Hello {
                protocol_version: WEB_PROTOCOL_VERSION.into(),
                capabilities: Vec::new(),
                auth_token: Some("local-dev".into()),
            }
            .encode()
            .unwrap()
            .into(),
        ))
        .await
        .map_err(|error| error.to_string())?;
    let ack = socket
        .next()
        .await
        .ok_or("missing hello ack")?
        .map_err(|error| error.to_string())?;
    let Message::Binary(bytes) = ack else {
        return Err("unexpected hello ack".into());
    };
    match WireMessage::decode(bytes.as_ref()).map_err(|error| error.to_string())? {
        WireMessage::HelloAck { .. } => {}
        _ => return Err("unexpected hello ack".into()),
    }
    socket
        .send(Message::Binary(
            WireMessage::Rpc(RpcRequest {
                id: Uuid::new_v4(),
                namespace: PLUGIN_ID.into(),
                method: method.into(),
                params,
            })
            .encode()
            .unwrap()
            .into(),
        ))
        .await
        .map_err(|error| error.to_string())?;
    let message = socket
        .next()
        .await
        .ok_or("missing response")?
        .map_err(|error| error.to_string())?;
    let Message::Binary(bytes) = message else {
        return Err("unexpected response".into());
    };
    match WireMessage::decode(bytes.as_ref()).map_err(|error| error.to_string())? {
        WireMessage::RpcResult(result) => match result.error {
            Some(error) => Err(error.message),
            None => Ok(result.result.unwrap_or(Value::Null)),
        },
        _ => Err("unexpected wire message".into()),
    }
}
