#![cfg(unix)]

mod support;

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use mutsuki_service_control::ControlMethod;
use tempfile::Builder;

use support::{
    IpcConfig, assert_gateway_health, assert_gateway_only_task_surface, control, fake_qq_product,
    gateway_ready, try_control,
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
            if let Ok(health) = try_control(&service, ControlMethod::HealthCheck).await
                && gateway_ready(&health)
            {
                break health;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("product did not become healthy: {}", process.diagnostics()));
    assert_gateway_health(&health);

    let tasks = control(&service, ControlMethod::TaskList).await;
    assert_gateway_only_task_surface(&tasks);

    control(&service, ControlMethod::ServiceShutdown).await;
    let status = process.wait_for_exit(Duration::from_secs(30)).await;
    assert!(
        status.success(),
        "product exited with {status}: {}",
        process.diagnostics()
    );
    assert!(!socket_path.exists(), "Unix socket survived process exit");

    let snapshot = fake.shutdown().await;
    assert_eq!(snapshot.websocket_connections, 2);
    assert_eq!(snapshot.gateway_auth_frames[0]["op"], 2);
    assert_eq!(snapshot.gateway_auth_frames[1]["op"], 6);
    assert_eq!(snapshot.clean_closes, 1);
}

struct ProductProcess {
    child: Child,
    log_path: PathBuf,
}

impl ProductProcess {
    fn spawn(config_path: &Path, log_path: PathBuf) -> Self {
        let output = File::create(&log_path).expect("create product process log");
        let error = output.try_clone().expect("clone product process log");
        let child = Command::new(env!("CARGO_BIN_EXE_mutsuki-bot"))
            .arg(config_path)
            .stdin(Stdio::null())
            .stdout(Stdio::from(output))
            .stderr(Stdio::from(error))
            .spawn()
            .expect("start production mutsuki-bot binary");
        Self { child, log_path }
    }

    fn assert_running(&mut self) {
        if let Some(status) = self.child.try_wait().expect("inspect product process") {
            panic!(
                "product exited before becoming healthy with {status}: {}",
                self.diagnostics()
            );
        }
    }

    async fn wait_for_exit(&mut self, timeout: Duration) -> ExitStatus {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait().expect("inspect product process") {
                return status;
            }
            if Instant::now() >= deadline {
                panic!(
                    "product did not exit after shutdown: {}",
                    self.diagnostics()
                );
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    fn diagnostics(&self) -> String {
        std::fs::read_to_string(&self.log_path)
            .unwrap_or_else(|error| format!("failed to read process log: {error}"))
    }
}

impl Drop for ProductProcess {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}
