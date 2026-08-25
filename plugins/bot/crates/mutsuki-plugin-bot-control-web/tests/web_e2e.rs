//! control.* WebHost E2E.
// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(clippy::default_trait_access, clippy::doc_markdown)]

use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use mutsuki_plugin_bot_control_web::{ControlWebExtension, FixtureControlHandler};
use mutsuki_web_host::{MinimalWebApplication, MutsukiWebHost, WebHost};
use mutsuki_web_protocol::{
    DeploymentMode, RpcRequest, WEB_PROTOCOL_VERSION, WebApplicationDescriptor, WebShellAssets,
    WireMessage,
};
use serde_json::json;
use uuid::Uuid;

async fn start(handler: Arc<FixtureControlHandler>) -> MutsukiWebHost {
    start_with_local_admin(handler, true).await
}

async fn start_with_local_admin(
    handler: Arc<FixtureControlHandler>,
    local_admin: bool,
) -> MutsukiWebHost {
    let shell_dir = tempfile::tempdir().unwrap();
    let extension = ControlWebExtension::from_handler(handler, "local-dev");
    let mut builder = MutsukiWebHost::builder()
        .application(MinimalWebApplication::new(
            WebApplicationDescriptor {
                id: "mutsuki.bot.control".into(),
                name: "Control".into(),
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
        .extension(extension);
    if local_admin {
        builder = builder.auth_token("local-dev");
    }
    let mut host = builder.build().unwrap();
    host.start().await.unwrap();
    std::mem::forget(shell_dir);
    host
}

async fn ws_rpc(
    addr: &str,
    method: &str,
    params: serde_json::Value,
    hello_caps: &[&str],
) -> Result<serde_json::Value, String> {
    ws_rpc_with_token(addr, method, params, hello_caps, Some("local-dev")).await
}

async fn ws_rpc_with_token(
    addr: &str,
    method: &str,
    params: serde_json::Value,
    hello_caps: &[&str],
    auth_token: Option<&str>,
) -> Result<serde_json::Value, String> {
    use tokio_tungstenite::{connect_async, tungstenite::Message};
    let (mut ws, _) = connect_async(format!("ws://{addr}/ws")).await.expect("ws");
    ws.send(Message::Binary(
        WireMessage::Hello {
            protocol_version: WEB_PROTOCOL_VERSION.into(),
            capabilities: hello_caps.iter().map(|cap| (*cap).into()).collect(),
            auth_token: auth_token.map(str::to_string),
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
            namespace: "control".into(),
            method: method.into(),
            params,
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

fn read_params() -> serde_json::Value {
    json!({})
}

fn write_params() -> serde_json::Value {
    json!({})
}

#[tokio::test]
async fn control_read_methods() {
    let mut host = start(Arc::new(FixtureControlHandler::default())).await;
    let addr = host.listen_addr().unwrap().to_string();
    let status = ws_rpc(&addr, "service_status", read_params(), &["runtime.read"])
        .await
        .unwrap();
    assert_eq!(status["instance_id"], "demo");
    let health = ws_rpc(&addr, "health", read_params(), &["runtime.read"])
        .await
        .unwrap();
    assert_eq!(health["service"], "ok");
    let plugins = ws_rpc(&addr, "plugin_list", read_params(), &["runtime.read"])
        .await
        .unwrap();
    assert_eq!(plugins["plugins"][0]["plugin_id"], "demo.plugin");
    host.stop().await.unwrap();
}

#[tokio::test]
async fn control_log_tail_and_task_list() {
    let mut host = start(Arc::new(FixtureControlHandler::default())).await;
    let addr = host.listen_addr().unwrap().to_string();
    let logs = ws_rpc(&addr, "log_tail", json!({"lines": 20}), &["runtime.read"])
        .await
        .unwrap();
    assert_eq!(logs["entries"][0]["line"], "demo log line");
    let tasks = ws_rpc(&addr, "task_list", read_params(), &["runtime.read"])
        .await
        .unwrap();
    assert_eq!(tasks[0]["task_id"], "demo.task");
    host.stop().await.unwrap();
}

#[tokio::test]
async fn client_capabilities_cannot_grant_control_access() {
    let handler = Arc::new(FixtureControlHandler::default());
    let mut host = start_with_local_admin(handler.clone(), false).await;
    let addr = host.listen_addr().unwrap().to_string();
    // No console token is configured, so the session is the anonymous read-only one. The Hello
    // capability list is client-supplied and must not widen it.
    let denied = ws_rpc_with_token(
        &addr,
        "plugin_reload",
        json!({"capabilities": ["runtime.write", "*"]}),
        &["runtime.read", "runtime.write", "*"],
        None,
    )
    .await
    .unwrap_err();
    assert!(denied.contains("capability denied"), "{denied}");
    assert!(handler.mutations.lock().unwrap().is_empty());
    host.stop().await.unwrap();
}

#[tokio::test]
async fn control_write_deployment_and_event_source_restart() {
    let handler = Arc::new(FixtureControlHandler::default());
    let mut host = start(handler.clone()).await;
    let addr = host.listen_addr().unwrap().to_string();
    ws_rpc(
        &addr,
        "plugin_deployment_set",
        json!({
            "plugin_id": "demo.plugin",
            "deployment": "builtin",
        }),
        &["runtime.read", "runtime.write"],
    )
    .await
    .unwrap();
    ws_rpc(
        &addr,
        "plugin_deployment_clear",
        json!({
            "plugin_id": "demo.plugin",
        }),
        &["runtime.read", "runtime.write"],
    )
    .await
    .unwrap();
    ws_rpc(
        &addr,
        "event_source_restart",
        json!({
            "id": "demo.source",
        }),
        &["runtime.read", "runtime.write"],
    )
    .await
    .unwrap();
    {
        let mutations = handler.mutations.lock().unwrap();
        assert!(mutations.contains(&"plugin_deployment_set".to_string()));
        assert!(mutations.contains(&"plugin_deployment_clear".to_string()));
        assert!(mutations.contains(&"event_source_restart".to_string()));
    }
    host.stop().await.unwrap();
}

#[tokio::test]
async fn control_plugin_list_includes_candidates_and_diagnostics() {
    let mut host = start(Arc::new(FixtureControlHandler::default())).await;
    let addr = host.listen_addr().unwrap().to_string();
    let plugins = ws_rpc(&addr, "plugin_list", read_params(), &["runtime.read"])
        .await
        .unwrap();
    assert_eq!(
        plugins["plugins"][0]["candidates"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(plugins["diagnostics"][0]["plugin_id"], "broken.plugin");
    host.stop().await.unwrap();
}

#[tokio::test]
async fn control_task_debug_and_lifecycle_methods() {
    let handler = Arc::new(FixtureControlHandler::default());
    let mut host = start(handler.clone()).await;
    let addr = host.listen_addr().unwrap().to_string();

    let events = ws_rpc(
        &addr,
        "task_events_after",
        json!({"sequence": 0, "limit": 8}),
        &["runtime.read"],
    )
    .await
    .unwrap();
    assert_eq!(events["lost"], 0);

    let drain = ws_rpc(
        &addr,
        "core_begin_drain",
        write_params(),
        &["runtime.read", "runtime.write"],
    )
    .await
    .unwrap();
    assert_eq!(drain["state"], "draining");

    ws_rpc(
        &addr,
        "service_shutdown",
        write_params(),
        &["runtime.read", "runtime.write"],
    )
    .await
    .unwrap();

    let batch = mutsuki_runtime_contracts::TaskBatch::one(
        "console-debug",
        mutsuki_runtime_contracts::Task::new(
            "debug-task-1",
            "control.input",
            json!({ "value": 1 }),
        ),
    );
    ws_rpc(
        &addr,
        "task_submit_batch",
        json!({
            "batch": batch,
        }),
        &["runtime.read", "runtime.write"],
    )
    .await
    .unwrap();

    ws_rpc(
        &addr,
        "task_cancel",
        json!({
            "id": "demo.task",
        }),
        &["runtime.read", "runtime.write"],
    )
    .await
    .unwrap();

    {
        let mutations = handler.mutations.lock().unwrap();
        assert!(mutations.contains(&"core_begin_drain".to_string()));
        assert!(mutations.contains(&"service_shutdown".to_string()));
        assert!(mutations.contains(&"task_submit_batch".to_string()));
        assert!(mutations.contains(&"task_cancel".to_string()));
    }

    host.stop().await.unwrap();
}
