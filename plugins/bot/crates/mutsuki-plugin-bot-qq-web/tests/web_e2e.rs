use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use mutsuki_plugin_bot_qq_web::*;
use mutsuki_web_host::{MinimalWebApplication, MutsukiWebHost, WebHost};
use mutsuki_web_protocol::{
    DeploymentMode, RpcRequest, WEB_PROTOCOL_VERSION, WebApplicationDescriptor, WebShellAssets,
    WireMessage,
};
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Default)]
struct Api {
    revision: Mutex<u64>,
    writes: Mutex<Vec<(String, QqManagementWriteRequest)>>,
}

#[async_trait]
impl QqBotManagementApi for Api {
    async fn snapshot(
        &self,
        _query: &str,
        include_secret_status: bool,
    ) -> Result<QqBotManagementSnapshot, QqManagementError> {
        Ok(QqBotManagementSnapshot {
            revision: *self.revision.lock().unwrap(),
            accounts: vec![QqAccountView {
                account_id: "main".into(),
                app_id: "app".into(),
                enabled: true,
                health: "ok".into(),
                connection_state: QqGatewayConnectionState::Resumable,
                last_heartbeat_unix_ms: Some(100),
                last_error: None,
                reconnect_count: 0,
                intents: 1 << 25,
                shard: [0, 1],
                credential_reference: if include_secret_status {
                    "secret://QQBOT_CLIENT_SECRET".into()
                } else {
                    String::new()
                },
                credential_status: if include_secret_status {
                    "configured"
                } else {
                    "restricted"
                }
                .into(),
                rate_limit_status: "ready".into(),
                capability: capability(),
                self_user: Some(mutsuki_bot_protocol::BotUser {
                    user_id: "BOT_OPENID".into(),
                    display_name: Some("mutsuki".into()),
                    avatar_url: Some("https://q.qlogo.cn/qqapp/app/BOT_OPENID/640".into()),
                }),
            }],
            deliveries: Vec::new(),
            interactions: Vec::new(),
        })
    }

    async fn write(
        &self,
        actor_id: &str,
        request: QqManagementWriteRequest,
    ) -> Result<QqManagementWriteResult, QqManagementError> {
        let mut revision = self.revision.lock().unwrap();
        if request.expected_revision != *revision {
            return Err(QqManagementError {
                code: "revision.conflict".into(),
                message: "stale revision".into(),
            });
        }
        self.writes.lock().unwrap().push((actor_id.into(), request));
        *revision += 1;
        Ok(QqManagementWriteResult {
            revision: *revision,
            audit_id: format!("audit-{revision}"),
            result: json!({"reconnected": true}),
        })
    }
}

#[tokio::test]
async fn qq_management_rpc_uses_authenticated_capabilities_confirmation_and_fixed_actor() {
    let api = Arc::new(Api::default());
    let assets_dir = tempfile::tempdir().unwrap();
    let shell_dir = tempfile::tempdir().unwrap();
    let assets = materialize_frontend_assets(assets_dir.path()).unwrap();
    let frontend = std::fs::read_to_string(assets.join("index.js")).unwrap();
    assert!(frontend.contains("请到配置里填写账号"));
    assert!(!frontend.contains("保存登录配置"));
    assert!(frontend.contains("self_user"));
    assert!(frontend.contains("OpenID"));
    assert!(frontend.contains("qq-account-avatar"));
    std::fs::write(
        shell_dir.path().join("index.html"),
        "<!doctype html><main></main>",
    )
    .unwrap();
    let extension = QqBotWebExtension::new(api.clone()).with_frontend_assets(&assets);
    let mut host = MutsukiWebHost::builder()
        .application(MinimalWebApplication::new(
            WebApplicationDescriptor {
                id: "mutsuki.bot.qq".into(),
                name: "QQ Bot".into(),
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

    let authenticated = rpc(&address, "snapshot", json!({})).await.unwrap();
    assert_eq!(
        authenticated["accounts"][0]["credential_reference"],
        "secret://QQBOT_CLIENT_SECRET"
    );
    assert_eq!(
        authenticated["accounts"][0]["credential_status"],
        "configured"
    );
    assert_eq!(
        authenticated["accounts"][0]["self_user"]["user_id"],
        "BOT_OPENID"
    );
    assert_eq!(
        authenticated["accounts"][0]["self_user"]["display_name"],
        "mutsuki"
    );

    let forged = json!({
        "actor_id": "operator",
        "expected_revision": 0,
        "action": {"action": "account_reconnect", "account_id": "main"}
    });
    assert!(
        rpc(
            &address,
            "write",
            json!({
                "confirmed": true,
                "request": forged
            }),
        )
        .await
        .is_err()
    );
    let request = json!({
        "expected_revision": 0,
        "action": {"action": "account_reconnect", "account_id": "main"}
    });
    assert!(
        rpc(
            &address,
            "write",
            json!({
                "confirmed": false,
                "request": request.clone()
            }),
        )
        .await
        .is_err()
    );
    let written = rpc(
        &address,
        "write",
        json!({
            "confirmed": true,
            "request": request
        }),
    )
    .await
    .unwrap();
    assert_eq!(written["revision"], 1);
    assert_eq!(written["audit_id"], "audit-1");
    {
        let writes = api.writes.lock().unwrap();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].0, "local-web-console");
    }

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
        .ok_or_else(|| "missing hello ack".to_string())?
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
        .ok_or_else(|| "missing response".to_string())?
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

fn capability() -> mutsuki_bot_protocol::QqBotCapabilityMatrix {
    mutsuki_bot_protocol::QqBotCapabilityMatrix {
        account_id: "main".into(),
        conversation_kinds: vec![mutsuki_bot_protocol::BotConversationKind::Private],
        outbound_conversation_kinds: vec![mutsuki_bot_protocol::BotConversationKind::Private],
        active_message_kinds: vec![mutsuki_bot_protocol::BotConversationKind::Private],
        inbound_segments: vec![mutsuki_bot_protocol::QqMessageSegmentKind::Text],
        outbound_segments: vec![mutsuki_bot_protocol::QqMessageSegmentKind::Text],
        inbound_media: Vec::new(),
        outbound_media: Vec::new(),
        active_message: true,
        message_edit: false,
        message_recall: true,
        reply: true,
        quote: true,
        mention: true,
        upload: mutsuki_bot_protocol::QqUploadConstraints::default(),
        rate_limit: mutsuki_bot_protocol::QqRateLimitPolicy::default(),
        streaming: vec![mutsuki_bot_protocol::QqStreamingStrategy::FinalOnly],
        configured_intents: 1 << 25,
        shard: [0, 1],
        required_intents: vec!["group_and_c2c_event".into()],
        required_permissions: vec![mutsuki_bot_protocol::QqPermissionRequirement::ReadC2cMessages],
    }
}
