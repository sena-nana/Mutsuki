#![cfg(unix)]

#[path = "support/process.rs"]
mod process;
mod support;

use std::path::PathBuf;
use std::time::Duration;

use mutsuki_service_control::{ControlCommand, ControlResponse, ControlResult};
use tempfile::Builder;

use process::ProductProcess;
use support::{
    IpcConfig, assert_gateway_health, assert_gateway_only_task_surface, fake_qq_product,
    gateway_ready, task_list, try_health,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn production_binary_runs_fake_qq_over_unix_ipc_and_shuts_down_cleanly() {
    let root = Builder::new()
        .prefix("mtk-bot-")
        .tempdir_in("/tmp")
        .expect("short Unix smoke directory");
    let (fake, service, config_path) = fake_qq_product(
        root.path(),
        IpcConfig {
            transport: "unix-socket",
            name: "bot",
            tcp_debug_addr: None,
        },
    )
    .await;
    let socket_path = PathBuf::from(service.ipc_endpoint());
    let mut process = ProductProcess::spawn(&config_path, root.path().join("product.log"));

    let health = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            process.assert_running();
            if let Ok(health) = try_health(&service).await
                && gateway_ready(&health)
            {
                break health;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("product did not become healthy: {}", process.summary()));
    assert_gateway_health(&health);

    let tasks = task_list(&service).await;
    assert_gateway_only_task_surface(&tasks);

    let response = mutsuki_service_ipc::ControlClient::new((&service).into())
        .request(ControlCommand::ServiceShutdown)
        .await
        .unwrap();
    assert!(matches!(
        response,
        ControlResponse::Ok(ControlResult::ServiceShutdown)
    ));
    let status = process.wait_for_exit(Duration::from_secs(30)).await;
    assert!(
        status.success(),
        "product exited with {status}: {}",
        process.summary()
    );
    assert!(!socket_path.exists(), "Unix socket survived process exit");

    let snapshot = fake.shutdown().await;
    assert_eq!(snapshot.websocket_connections, 2);
    assert_eq!(snapshot.gateway_auth_frames[0]["op"], 2);
    assert_eq!(snapshot.gateway_auth_frames[1]["op"], 6);
    assert_eq!(snapshot.clean_closes, 1);
}
