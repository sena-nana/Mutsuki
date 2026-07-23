//! upgrade.* WebHost E2E with fixture remote head provider.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use mutsuki_plugin_bot_upgrade_web::UpgradeWebExtension;
use mutsuki_plugin_catalog::FixtureRemoteHeadProvider;
use mutsuki_web_host::{MinimalWebApplication, MutsukiWebHost, WebHost};
use mutsuki_web_protocol::{
    DeploymentMode, RpcRequest, WEB_PROTOCOL_VERSION, WebApplicationDescriptor, WebShellAssets,
    WireMessage,
};
use serde_json::json;
use uuid::Uuid;

async fn start() -> MutsukiWebHost {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let release_set = root
        .join("..")
        .join("mutsuki-plugin-catalog")
        .join("tests")
        .join("fixtures")
        .join("release-set.toml");
    let remote = Arc::new(
        FixtureRemoteHeadProvider::default()
            .with_head(
                "https://github.com/sena-nana/MutsukiCore.git",
                "bbbb2222cccc3333dddd4444",
            )
            .with_head(
                "https://github.com/sena-nana/MutsukiBotPlugins.git",
                "cccc3333dddd4444eeee5555",
            ),
    );
    let extension = UpgradeWebExtension::new(&release_set)
        .unwrap()
        .with_remote_provider(remote);
    let shell_dir = tempfile::tempdir().unwrap();
    let mut host = MutsukiWebHost::builder()
        .application(MinimalWebApplication::new(
            WebApplicationDescriptor {
                id: "mutsuki.bot.upgrade".into(),
                name: "Auto Upgrade".into(),
                version: "0.1.0".into(),
                brand: Some("Mutsuki".into()),
                theme: Some("lilia".into()),
            },
            WebShellAssets {
                root_dir: shell_dir.path().to_path_buf(),
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
    std::mem::forget(shell_dir);
    host
}

#[tokio::test]
async fn upgrade_check_and_plan_use_fixture_remote() {
    let mut host = start().await;
    let addr = host.listen_addr().unwrap().to_string();

    let check = ws_rpc(&addr, "check", json!({})).await.unwrap();
    assert_eq!(check["release_set"], "mutsuki-0.1-alpha-3");
    assert!(check["modules"].as_array().unwrap().len() >= 2);
    assert!(check["update_count"].as_u64().unwrap() >= 1);

    let plan = ws_rpc(
        &addr,
        "plan",
        json!({"module_id": "core", "target_revision": "bbbb2222cccc3333dddd4444"}),
    )
    .await
    .unwrap();
    assert_eq!(plan["module_id"], "core");
    assert!(plan["plan"]["steps"].as_array().unwrap().len() >= 4);
    assert_eq!(plan["reload"]["method"], "plugin_reload");

    host.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test]
async fn upgrade_execute_dry_run_via_web() {
    let mut host = start().await;
    let addr = host.listen_addr().unwrap().to_string();

    let report = ws_rpc(
        &addr,
        "execute",
        json!({"module_id": "core", "target_revision": "bbbb2222cccc3333dddd4444", "dry_run": true}),
    )
    .await
    .unwrap();
    assert_eq!(report["dry_run"], true);
    assert!(report["cli_command"].as_str().unwrap().contains("execute"));
    assert!(report["report"]["steps"].as_array().unwrap().len() >= 3);

    host.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

async fn ws_rpc(
    addr: &str,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    use tokio_tungstenite::{connect_async, tungstenite::Message};
    let (mut ws, _) = connect_async(format!("ws://{addr}/ws")).await.expect("ws");
    ws.send(Message::Text(
        serde_json::to_string(&WireMessage::Hello {
            protocol_version: WEB_PROTOCOL_VERSION.into(),
            capabilities: vec!["runtime.read".into()],
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
            namespace: "upgrade".into(),
            method: method.into(),
            params,
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
