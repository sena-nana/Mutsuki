use std::path::PathBuf;
use std::time::Duration;

use mutsuki_bot::{assemble_service, repository_local_config_path};
use mutsuki_service_config::{ConfigOverrides, ServiceConfig};
use mutsuki_service_control::ControlMethod;
use serde_json::Value;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires ignored local QQBot config and secret files"]
async fn real_qqbot_gateway_health_smoke() {
    let path = std::env::var_os("MUTSUKI_QQBOT_SMOKE_CONFIG")
        .or_else(|| std::env::var_os("MUTSUKI_CONFIG"))
        .map(PathBuf::from)
        .unwrap_or_else(repository_local_config_path);
    let service = ServiceConfig::load(ConfigOverrides {
        config_file: Some(path),
        ..Default::default()
    })
    .expect("load local QQBot smoke config");
    assert!(
        service.ipc.enabled,
        "real smoke requires IPC health inspection"
    );
    let control_config = service.clone();
    let runtime = assemble_service(service)
        .expect("assemble configured QQBot")
        .start()
        .await
        .expect("start real QQBot ServiceRuntime");

    let health = tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            let health = control(&control_config, ControlMethod::HealthCheck).await;
            if health["event_sources"] == "ok" {
                break health;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await;
    let health = match health {
        Ok(health) => health,
        Err(_) => {
            let health = control(&control_config, ControlMethod::HealthCheck).await;
            runtime.shutdown().await;
            panic!("QQ Gateway did not become healthy: {health}");
        }
    };
    assert_eq!(health["service"], "ok");
    assert_eq!(health["core"], "ok");
    runtime.shutdown().await;
}

async fn control(config: &ServiceConfig, method: ControlMethod) -> Value {
    let client = mutsuki_service_ipc::ControlClient::new(config.into());
    let response = client.request(method, Value::Null).await.unwrap();
    assert!(response.ok, "control failed: {:?}", response.error);
    response.result.unwrap_or(Value::Null)
}
