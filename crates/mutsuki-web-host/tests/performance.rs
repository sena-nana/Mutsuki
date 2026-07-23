//! Performance baselines for WebHost startup and critical RPC path.

use std::time::{Duration, Instant};

use mutsuki_web_host::{MinimalWebApplication, MutsukiWebHost, WebHost};
use mutsuki_web_protocol::{DeploymentMode, WEB_PROTOCOL_VERSION};
use tempfile::tempdir;
use uuid::Uuid;

#[tokio::test]
async fn startup_under_budget_without_extensions() {
    let shell_dir = tempdir().unwrap();
    let mut host = MutsukiWebHost::builder()
        .application(MinimalWebApplication::empty("perf.app"))
        .listen("127.0.0.1:0")
        .mode(DeploymentMode::Embedded)
        .shell_dir(shell_dir.path())
        .build()
        .unwrap();

    let started = Instant::now();
    host.start().await.unwrap();
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_millis(1500),
        "startup too slow: {elapsed:?}"
    );

    let bridge = host.bridge().cloned().unwrap();
    let hello_started = Instant::now();
    let hello = bridge
        .handle_message(
            None,
            mutsuki_web_protocol::WireMessage::Hello {
                protocol_version: WEB_PROTOCOL_VERSION.into(),
                capabilities: vec!["host.read".into()],
                auth_token: None,
            },
        )
        .unwrap();
    let session_id = match hello {
        mutsuki_web_bridge::HandleOutcome::Reply(mutsuki_web_protocol::WireMessage::HelloAck {
            session,
            ..
        }) => session.session_id,
        _ => panic!("hello"),
    };
    let _ = bridge
        .handle_message(
            Some(session_id),
            mutsuki_web_protocol::WireMessage::Rpc(mutsuki_web_protocol::RpcRequest {
                id: Uuid::new_v4(),
                namespace: "host".into(),
                method: "status".into(),
                params: serde_json::Value::Null,
            }),
        )
        .unwrap();
    let rpc_elapsed = hello_started.elapsed();
    assert!(
        rpc_elapsed < Duration::from_millis(100),
        "rpc path too slow: {rpc_elapsed:?}"
    );

    let stop_started = Instant::now();
    host.stop().await.unwrap();
    assert!(stop_started.elapsed() < Duration::from_millis(1000));
    std::mem::forget(shell_dir);
}

#[tokio::test]
async fn disabled_host_has_no_listen_overhead() {
    // Building without start must not bind ports or spawn workers.
    let host = MutsukiWebHost::builder()
        .application(MinimalWebApplication::empty("idle.app"))
        .listen("127.0.0.1:0")
        .build()
        .unwrap();
    assert!(host.listen_addr().is_none());
    assert!(host.bridge().is_none());
    assert_eq!(
        host.status().status,
        mutsuki_web_protocol::WebHostStatus::Created
    );
}
