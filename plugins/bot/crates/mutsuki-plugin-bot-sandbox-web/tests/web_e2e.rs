use std::sync::Arc;

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use mutsuki_plugin_bot_sandbox_web::*;
use mutsuki_web_extension_api::{WebExtension, content_hash};
use mutsuki_web_host::{MinimalWebApplication, MutsukiWebHost, WebHost};
use mutsuki_web_protocol::{
    DeploymentMode, RpcRequest, WEB_PROTOCOL_VERSION, WebApplicationDescriptor, WebShellAssets,
    WireMessage,
};
use serde_json::{Value, json};
use uuid::Uuid;

struct TestRuntime;

#[async_trait]
impl SandboxRuntime for TestRuntime {
    fn live_available(&self) -> bool {
        false
    }

    async fn ingest(&self, _event: mutsuki_bot_protocol::BotEvent) -> Result<(), SandboxError> {
        Ok(())
    }

    async fn deliver(
        &self,
        _operation_id: &str,
        _conversation: &mutsuki_bot_protocol::QqConversationRef,
        _segments: &[mutsuki_bot_protocol::MessageSegment],
        _reply_to: Option<&str>,
    ) -> Result<Value, SandboxError> {
        Err(SandboxError::new("qq.owner_unavailable", "尚未连接 QQ"))
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn sandbox_rpc_simulate_send_and_confirm_live_send() {
    let service = Arc::new(SandboxService::new());
    service.set_runtime(Arc::new(TestRuntime));
    let assets_dir = tempfile::tempdir().unwrap();
    let shell_dir = tempfile::tempdir().unwrap();
    let assets = materialize_frontend_assets(assets_dir.path()).unwrap();
    let extension = SandboxWebExtension::new(service.clone()).with_frontend_assets(&assets);
    let descriptor = extension.descriptor();
    // The bundle's rendered copy is not a contract. What the Host serves and the client verifies is
    // the manifest, so that is what this test pins.
    assert_eq!(descriptor.entry, "index.js");
    let entry = descriptor
        .assets
        .iter()
        .find(|asset| asset.path == "index.js")
        .expect("manifest declares its entry asset");
    let bytes = std::fs::read(assets.join("index.js")).unwrap();
    assert_eq!(entry.bytes, bytes.len() as u64);
    assert_eq!(entry.content_hash, content_hash(&bytes));
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
        .extension(extension)
        .auth_token("local-dev")
        .build()
        .unwrap();
    host.start().await.unwrap();
    let address = host.listen_addr().unwrap().to_string();

    let snapshot = rpc(&address, "snapshot", json!({})).await.unwrap();
    assert_eq!(snapshot["mode"], "simulate");
    assert_eq!(snapshot["flow_available"], true);
    let conversation_id = snapshot["conversations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["kind"] == "group")
        .unwrap()["conversation_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let user_id = snapshot["conversations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["kind"] == "group")
        .unwrap()["users"][0]["user_id"]
        .as_str()
        .unwrap()
        .to_owned();
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

    let added = rpc(
        &address,
        "write",
        json!({
            "request": {
                "expected_revision": written["revision"],
                "action": { "action": "add_user" }
            }
        }),
    )
    .await
    .unwrap();
    assert_eq!(added["result"]["display_name"], "Carol");
    let carol = added["result"]["user_id"].as_str().unwrap().to_owned();
    let renamed = rpc(
        &address,
        "write",
        json!({
            "request": {
                "expected_revision": added["revision"],
                "action": {
                    "action": "update_user",
                    "user_id": carol,
                    "new_user_id": "custom-openid",
                    "display_name": "测试昵称"
                }
            }
        }),
    )
    .await
    .unwrap();
    assert_eq!(renamed["result"]["user_id"], "custom-openid");
    assert_eq!(renamed["result"]["display_name"], "测试昵称");

    let uploaded = rpc(
        &address,
        "media.upload",
        json!({
            "name": "pic.png",
            "mime": "image/png",
            "bytes": "aGk="
        }),
    )
    .await
    .unwrap();
    let media_id = uploaded["media_id"].as_str().unwrap().to_owned();
    assert!(media_id.starts_with("sha256:"));
    let blob = rpc(&address, "media.get", json!({ "media_id": media_id }))
        .await
        .unwrap();
    assert_eq!(blob["mime"], "image/png");
    assert_eq!(blob["bytes"], "aGk=");
    let uploaded_again = rpc(
        &address,
        "media.upload",
        json!({
            "name": "pic-again.png",
            "mime": "image/png",
            "bytes": "aGk="
        }),
    )
    .await
    .unwrap();
    assert_eq!(uploaded_again["media_id"], media_id);

    let sticker = rpc(
        &address,
        "sticker.upload",
        json!({
            "name": "pack.png",
            "mime": "image/png",
            "bytes": "c3RpY2tlcg=="
        }),
    )
    .await
    .unwrap();
    let sticker_id = sticker["sticker_id"].as_str().unwrap().to_owned();
    assert!(sticker_id.starts_with("sha256:"));
    assert_ne!(sticker_id, media_id);
    let listed = rpc(&address, "sticker.list", json!({})).await.unwrap();
    assert!(
        listed
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == sticker_id && item["kind"] == "custom")
    );
    let sticker_blob = rpc(&address, "sticker.get", json!({ "sticker_id": sticker_id }))
        .await
        .unwrap();
    assert_eq!(sticker_blob["bytes"], "c3RpY2tlcg==");

    let with_media = rpc(
        &address,
        "write",
        json!({
            "request": {
                "expected_revision": renamed["revision"],
                "action": {
                    "action": "ingest_as_user",
                    "conversation_id": conversation_id,
                    "user_id": user_id,
                    "segments": [{
                        "type": "platform_specific",
                        "platform": "sandbox",
                        "kind": "media",
                        "payload": {
                            "media_id": media_id,
                            "mime": "image/png",
                            "name": "pic.png"
                        }
                    }]
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
    let media_message = messages
        .as_array()
        .unwrap()
        .iter()
        .find(|item| {
            item["refs"].as_array().is_some_and(|refs| {
                refs.iter()
                    .any(|item| item["h"].as_str() == Some(media_id.as_str()))
            })
        })
        .expect("hashed media ref");
    assert!(
        media_message
            .get("segments")
            .and_then(Value::as_array)
            .is_none_or(Vec::is_empty)
    );
    assert!(
        media_message["refs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item.get("resource").is_none())
    );

    let switched = rpc(
        &address,
        "write",
        json!({
            "request": {
                "expected_revision": with_media["revision"],
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

    // Actions the page no longer offers have to be gone from the action surface, not just from the
    // bundle's rendered controls.
    assert!(
        rpc(
            &address,
            "write",
            json!({
                "request": {
                    "expected_revision": switched["revision"],
                    "action": { "action": "inject_into_flow" }
                }
            }),
        )
        .await
        .is_err()
    );

    host.stop().await.unwrap();
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
        return Err("unexpected wire message".into());
    };
    match WireMessage::decode(bytes.as_ref()).map_err(|error| error.to_string())? {
        WireMessage::RpcResult(result) => match result.error {
            Some(error) => Err(error.message),
            None => Ok(result.result.unwrap_or(Value::Null)),
        },
        _ => Err("unexpected wire message".into()),
    }
}
