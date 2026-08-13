#![cfg(unix)]

#[path = "support/process.rs"]
mod process;
mod support;

use std::time::Duration;

use mutsuki_bot_testkit::FakeQqGatewayScript;
use tempfile::Builder;

use process::ProductProcess;
use support::{
    assert_gateway_health, assert_gateway_only_task_surface, fake_qq_product_with_script,
    gateway_ready, shutdown, task_list, try_health,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn production_binary_runs_fake_qq_from_config_repository_and_shuts_down_cleanly() {
    let root = Builder::new()
        .prefix("mtk-bot-")
        .tempdir_in("/tmp")
        .expect("short Unix smoke directory");
    let (fake, product) = fake_qq_product_with_script(
        root.path(),
        FakeQqGatewayScript {
            close_delay: Duration::from_secs(2),
            ..FakeQqGatewayScript::default()
        },
    )
    .await;
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
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if tokio::net::TcpStream::connect(&product.console_address)
                .await
                .is_err()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("console still accepted connections while Runtime was draining");
    process.assert_running();
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sigterm_uses_the_same_orderly_product_shutdown_path() {
    let root = Builder::new()
        .prefix("mtk-bot-sigterm-")
        .tempdir_in("/tmp")
        .expect("short Unix smoke directory");
    let (fake, product) = support::fake_qq_product(root.path()).await;
    let mut process =
        ProductProcess::spawn(&product.bootstrap_path, root.path().join("product.log"));

    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            process.assert_running();
            if try_health(&product)
                .await
                .is_ok_and(|health| gateway_ready(&health))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("product did not become healthy: {}", process.summary()));

    let signal = std::process::Command::new("kill")
        .args(["-TERM", &process.id().to_string()])
        .status()
        .expect("send SIGTERM");
    assert!(signal.success(), "kill -TERM failed with {signal}");
    let status = process.wait_for_exit(Duration::from_secs(30)).await;
    assert!(
        status.success(),
        "SIGTERM exit failed: {}",
        process.summary()
    );
    assert!(
        tokio::net::TcpStream::connect(&product.console_address)
            .await
            .is_err(),
        "console survived SIGTERM"
    );
    let snapshot = fake.shutdown().await;
    assert_eq!(snapshot.clean_closes, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn console_start_failure_rolls_back_the_started_runtime() {
    let root = Builder::new()
        .prefix("mtk-bot-console-fail-")
        .tempdir_in("/tmp")
        .expect("short Unix smoke directory");
    let (fake, product) = support::fake_qq_product(root.path()).await;
    let occupied = std::net::TcpListener::bind(&product.console_address)
        .expect("occupy configured console address");
    let mut process =
        ProductProcess::spawn(&product.bootstrap_path, root.path().join("product.log"));

    let status = process.wait_for_exit(Duration::from_secs(30)).await;
    assert!(
        !status.success(),
        "product unexpectedly survived Console start failure"
    );
    assert_eq!(
        occupied.local_addr().expect("occupied address").to_string(),
        product.console_address
    );
    let _ = fake.shutdown().await;
}
