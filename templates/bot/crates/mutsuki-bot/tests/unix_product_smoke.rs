#![cfg(unix)]

#[path = "support/process.rs"]
mod process;
mod support;

use std::time::Duration;

use tempfile::Builder;

use process::ProductProcess;
use support::{
    assert_gateway_health, assert_gateway_only_task_surface, fake_qq_product, gateway_ready,
    shutdown, task_list, try_health,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn production_binary_runs_fake_qq_from_config_repository_and_shuts_down_cleanly() {
    let root = Builder::new()
        .prefix("mtk-bot-")
        .tempdir_in("/tmp")
        .expect("short Unix smoke directory");
    let (fake, product) = fake_qq_product(root.path()).await;
    let mut process =
        ProductProcess::spawn(&product.bootstrap_path, root.path().join("product.log"));

    let health = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            process.assert_running();
            if let Ok(health) = try_health(&product).await
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

    let tasks = task_list(&product).await;
    assert_gateway_only_task_surface(&tasks);

    shutdown(&product).await.unwrap();
    let status = process.wait_for_exit(Duration::from_secs(30)).await;
    assert!(
        status.success(),
        "product exited with {status}: {}",
        process.summary()
    );
    assert!(
        tokio::net::TcpStream::connect(&product.console_address)
            .await
            .is_err(),
        "console survived process exit"
    );

    let snapshot = fake.shutdown().await;
    assert_eq!(snapshot.websocket_connections, 2);
    assert_eq!(snapshot.gateway_auth_frames[0]["op"], 2);
    assert_eq!(snapshot.gateway_auth_frames[1]["op"], 6);
    assert_eq!(snapshot.clean_closes, 1);
}
