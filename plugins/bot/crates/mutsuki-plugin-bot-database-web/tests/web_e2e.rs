use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use mutsuki_bot_state_db::BotStateDbRepository;
use mutsuki_plugin_bot_database_web::{
    DatabaseWebExtension, PLUGIN_ID, materialize_frontend_assets,
};
use mutsuki_web_extension::{WebExtension, content_hash};
use mutsuki_web_host::{MinimalWebApplication, MutsukiWebHost, WebHost};
use mutsuki_web_protocol::{
    DeploymentMode, RpcRequest, WEB_PROTOCOL_VERSION, WebApplicationDescriptor, WebShellAssets,
    WireMessage,
};
use serde_json::{Value, json};
use uuid::Uuid;

#[tokio::test]
async fn database_rpc_reads_live_bot_state_and_rejects_unknown_tables() {
    let root = tempfile::tempdir().unwrap();
    let repository =
        Arc::new(BotStateDbRepository::open(root.path().join("state.sqlite3")).unwrap());
    let assets_dir = tempfile::tempdir().unwrap();
    let shell_dir = tempfile::tempdir().unwrap();
    let assets = materialize_frontend_assets(assets_dir.path()).unwrap();
    let extension = DatabaseWebExtension::new(Some(repository)).with_frontend_assets(&assets);
    let descriptor = extension.descriptor();
    assert_eq!(descriptor.entry, "index.js");
    let bytes = std::fs::read(assets.join("index.js")).unwrap();
    assert_eq!(
        descriptor
            .assets
            .iter()
            .find(|asset| asset.path == "index.js")
            .map(|asset| asset.content_hash.as_str()),
        Some(content_hash(&bytes).as_str())
    );
    std::fs::write(
        shell_dir.path().join("index.html"),
        "<!doctype html><main></main>",
    )
    .unwrap();
    let mut host = MutsukiWebHost::builder()
        .application(MinimalWebApplication::new(
            WebApplicationDescriptor {
                id: "mutsuki.bot.database".into(),
                name: "Database".into(),
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
    assert!(
        snapshot["path"]
            .as_str()
            .unwrap()
            .ends_with("state.sqlite3")
    );
    assert!(
        snapshot["tables"]
            .as_array()
            .unwrap()
            .iter()
            .any(|table| table["name"] == "bot_management_meta" && table["row_count"] == 1)
    );
    let page = rpc(
        &address,
        "rows",
        json!({ "table": "bot_management_meta", "limit": 10 }),
    )
    .await
    .unwrap();
    assert_eq!(page["rows"].as_array().unwrap().len(), 1);
    assert!(
        rpc(&address, "rows", json!({ "table": "sqlite_master" }))
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
