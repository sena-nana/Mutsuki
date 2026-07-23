//! control.* WebHost E2E.

use std::sync::Arc;
use std::time::Duration;

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
    let shell_dir = tempfile::tempdir().unwrap();
    let extension = ControlWebExtension::from_handler(handler, "local-dev");
    let mut host = MutsukiWebHost::builder()
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
        .extension(extension)
        .auth_token("local-dev")
        .build()
        .unwrap();
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
    use tokio_tungstenite::{connect_async, tungstenite::Message};
    let (mut ws, _) = connect_async(format!("ws://{addr}/ws")).await.expect("ws");
    ws.send(Message::Text(
        serde_json::to_string(&WireMessage::Hello {
            protocol_version: WEB_PROTOCOL_VERSION.into(),
            capabilities: hello_caps.iter().map(|cap| (*cap).into()).collect(),
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
            namespace: "control".into(),
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

fn read_params() -> serde_json::Value {
    json!({"capabilities": ["runtime.read"]})
}

fn write_params() -> serde_json::Value {
    json!({"capabilities": ["runtime.read", "runtime.write"]})
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
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test]
async fn control_log_tail_and_task_list() {
    let mut host = start(Arc::new(FixtureControlHandler::default())).await;
    let addr = host.listen_addr().unwrap().to_string();
    let logs = ws_rpc(
        &addr,
        "log_tail",
        json!({"capabilities": ["runtime.read"], "lines": 20}),
        &["runtime.read"],
    )
    .await
    .unwrap();
    assert_eq!(logs["entries"][0]["line"], "demo log line");
    let tasks = ws_rpc(&addr, "task_list", read_params(), &["runtime.read"])
        .await
        .unwrap();
    assert_eq!(tasks[0]["task_id"], "demo.task");
    host.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test]
async fn control_write_requires_runtime_write_capability() {
    let handler = Arc::new(FixtureControlHandler::default());
    let mut host = start(handler.clone()).await;
    let addr = host.listen_addr().unwrap().to_string();
    let denied = ws_rpc(
        &addr,
        "plugin_reload",
        read_params(),
        &["runtime.read", "runtime.write"],
    )
    .await
    .unwrap_err();
    assert!(denied.contains("runtime.write"));
    let ok = ws_rpc(
        &addr,
        "plugin_reload",
        write_params(),
        &["runtime.read", "runtime.write"],
    )
    .await
    .unwrap();
    assert_eq!(ok["registry_generation"], 2);
    assert!(
        handler
            .mutations
            .lock()
            .unwrap()
            .contains(&"plugin_reload".to_string())
    );
    host.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
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
            "capabilities": ["runtime.read", "runtime.write"],
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
            "capabilities": ["runtime.read", "runtime.write"],
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
            "capabilities": ["runtime.read", "runtime.write"],
            "id": "demo.source",
        }),
        &["runtime.read", "runtime.write"],
    )
    .await
    .unwrap();
    let mutations = handler.mutations.lock().unwrap();
    assert!(mutations.contains(&"plugin_deployment_set".to_string()));
    assert!(mutations.contains(&"plugin_deployment_clear".to_string()));
    assert!(mutations.contains(&"event_source_restart".to_string()));
    host.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
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
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test]
async fn control_task_debug_and_lifecycle_methods() {
    let handler = Arc::new(FixtureControlHandler::default());
    let mut host = start(handler.clone()).await;
    let addr = host.listen_addr().unwrap().to_string();

    let events = ws_rpc(
        &addr,
        "task_events_after",
        json!({"capabilities": ["runtime.read"], "sequence": 0, "limit": 8}),
        &["runtime.read"],
    )
    .await
    .unwrap();
    assert_eq!(events["lost"], 0);

    let denied = ws_rpc(
        &addr,
        "core_begin_drain",
        read_params(),
        &["runtime.read", "runtime.write"],
    )
    .await
    .unwrap_err();
    assert!(denied.contains("runtime.write"));

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

    ws_rpc(
        &addr,
        "task_submit_batch",
        json!({
            "capabilities": ["runtime.read", "runtime.write"],
            "batch": {
                "batch_id": "console-debug",
                "tasks": [{
                    "task_id": "debug-task-1",
                    "protocol_id": "control.input",
                    "input": { "value": 1 }
                }]
            }
        }),
        &["runtime.read", "runtime.write"],
    )
    .await
    .unwrap();

    ws_rpc(
        &addr,
        "task_cancel",
        json!({
            "capabilities": ["runtime.read", "runtime.write"],
            "id": "demo.task",
        }),
        &["runtime.read", "runtime.write"],
    )
    .await
    .unwrap();

    let mutations = handler.mutations.lock().unwrap();
    assert!(mutations.contains(&"core_begin_drain".to_string()));
    assert!(mutations.contains(&"service_shutdown".to_string()));
    assert!(mutations.contains(&"task_submit_batch".to_string()));
    assert!(mutations.contains(&"task_cancel".to_string()));

    host.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
}
