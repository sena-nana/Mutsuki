mod support;

use std::time::Duration;

use mutsuki_bot::assemble_service;
use mutsuki_service_control::ControlMethod;
use tempfile::tempdir;
use tokio::net::TcpListener;

use support::{
    IpcConfig, assert_gateway_health, assert_gateway_only_task_surface, control, fake_qq_product,
    gateway_ready,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn external_config_runs_real_service_runtime_through_fake_qq_boundaries() {
    let root = tempdir().unwrap();
    let probe = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ipc_addr = probe.local_addr().unwrap();
    drop(probe);
    let (fake, service, _config_path) = fake_qq_product(
        root.path(),
        IpcConfig {
            transport: "tcp-debug",
            name: "template-qqbot-fake",
            tcp_debug_addr: Some(ipc_addr),
        },
    )
    .await;
    let control_config = service.clone();

    let runtime = assemble_service(service).unwrap().start().await.unwrap();
    let health = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let health = control(&control_config, ControlMethod::HealthCheck).await;
            if gateway_ready(&health) {
                break health;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("configured QQ Gateway becomes healthy");
    assert_gateway_health(&health);

    let tasks = control(&control_config, ControlMethod::TaskList).await;
    assert_gateway_only_task_surface(&tasks);

    runtime.shutdown().await;
    let snapshot = fake.shutdown().await;
    assert_eq!(snapshot.websocket_connections, 2);
    assert_eq!(snapshot.gateway_auth_frames[0]["op"], 2);
    assert_eq!(snapshot.gateway_auth_frames[1]["op"], 6);
    assert_eq!(snapshot.clean_closes, 1);
    assert!(TcpListener::bind(ipc_addr).await.is_ok());
}
