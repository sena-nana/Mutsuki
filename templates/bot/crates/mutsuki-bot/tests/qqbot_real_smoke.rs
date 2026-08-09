#[path = "support/process.rs"]
mod process;

use std::path::PathBuf;
use std::time::Duration;

use mutsuki_bot::repository_local_config_path;
use mutsuki_plugin_bot_adapter_qqbot::QqBotConfig;
#[cfg(windows)]
use mutsuki_service_config::IpcTransport;
use mutsuki_service_config::{ConfigOverrides, ServiceConfig};
use mutsuki_service_control::{ControlCommand, ControlResponse, ControlResult, HealthReport};
use mutsuki_service_ipc::ControlClient;
use tempfile::Builder;

use process::ProductProcess;

const QQBOT_PLUGIN_ID: &str = "mutsuki.bot.adapter.qqbot";
const QQBOT_HEALTH_PREFIX: &str = "mutsuki.bot.qqbot.gateway:";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires ignored local QQBot config and secret files"]
async fn real_qqbot_product_process_is_healthy_and_shuts_down_cleanly() {
    let config_path = std::env::var_os("MUTSUKI_QQBOT_SMOKE_CONFIG")
        .or_else(|| std::env::var_os("MUTSUKI_CONFIG"))
        .map(PathBuf::from)
        .unwrap_or_else(repository_local_config_path);
    let service = ServiceConfig::load(ConfigOverrides {
        config_file: Some(config_path.clone()),
        ..Default::default()
    })
    .unwrap_or_else(|_| panic!("failed to load local QQBot smoke config"));
    assert!(service.ipc.enabled, "real smoke requires IPC");
    #[cfg(windows)]
    assert_eq!(service.ipc.transport, IpcTransport::NamedPipe);

    let credentials = credentials(&service);
    assert_endpoint_closed(&service, "before startup").await;
    let service_log = FileTail::snapshot(service.service.log_dir.join(&service.observe.log_file));
    let panic_log = FileTail::snapshot(service.service.log_dir.join(&service.observe.panic_file));
    let output_dir = Builder::new()
        .prefix("mtk-qqbot-real-")
        .tempdir()
        .expect("create smoke output directory");
    let mut process = ProductProcess::spawn(&config_path, output_dir.path().join("product.log"));

    let health_result = tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            process.assert_running();
            if let Some(health) = health(&service).await
                && product_ready(&health)
            {
                break health;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await;
    let health = match health_result {
        Ok(health) => health,
        Err(_) => {
            let summary = health(&service)
                .await
                .map(|health| health_summary(&health))
                .unwrap_or_else(|| "health unavailable".into());
            panic!(
                "QQ Gateway health timeout: {summary}; {}",
                process.summary()
            );
        }
    };
    assert!(product_ready(&health), "{}", health_summary(&health));

    let response = ControlClient::new((&service).into())
        .request(ControlCommand::ServiceShutdown)
        .await
        .expect("request ServiceShutdown");
    assert!(matches!(
        response,
        ControlResponse::Ok(ControlResult::ServiceShutdown)
    ));
    let status = process.wait_for_exit(Duration::from_secs(30)).await;
    assert!(status.success(), "product exited with {status}");
    assert_endpoint_closed(&service, "after shutdown").await;

    assert_clean("child output", &process.output_bytes(), &credentials);
    assert_clean("service log", &service_log.new_bytes(), &credentials);
    assert_clean("panic log", &panic_log.new_bytes(), &credentials);
}

fn credentials(service: &ServiceConfig) -> Vec<Vec<u8>> {
    let store = service.host_secret_store();
    let mut values = vec![service.control_token().as_bytes().to_vec()];
    for selection in service
        .plugins
        .configured
        .iter()
        .filter(|selection| selection.enabled && selection.id == QQBOT_PLUGIN_ID)
    {
        let config: QqBotConfig = serde_json::from_value(selection.config.clone())
            .unwrap_or_else(|_| panic!("configured QQBot selection is invalid"));
        values.push(config.app_id.into_bytes());
        values.push(
            store
                .resolve(&config.client_secret_key)
                .expect("resolve QQBot client secret")
                .into_bytes(),
        );
    }
    assert!(values.len() > 1, "real smoke requires a QQBot adapter");
    values.retain(|value| !value.is_empty());
    values.sort();
    values.dedup();
    values
}

async fn health(service: &ServiceConfig) -> Option<HealthReport> {
    match ControlClient::new(service.into())
        .request(ControlCommand::HealthCheck)
        .await
        .ok()?
    {
        ControlResponse::Ok(ControlResult::HealthCheck(health)) => Some(health),
        _ => None,
    }
}

async fn assert_endpoint_closed(service: &ServiceConfig, phase: &str) {
    assert!(
        ControlClient::new(service.into()).connect().await.is_err(),
        "IPC endpoint is reachable {phase}"
    );
}

fn product_ready(health: &HealthReport) -> bool {
    health.service == "ok"
        && health.core == "ok"
        && health.event_sources == "ok"
        && gateway_counts(health).0 > 0
}

fn gateway_counts(health: &HealthReport) -> (usize, usize) {
    health
        .components
        .iter()
        .filter(|(name, _)| name.starts_with(QQBOT_HEALTH_PREFIX))
        .fold((0, 0), |(ready, total), (_, state)| {
            let connected = state.get("connected").and_then(|value| value.as_bool()) == Some(true);
            let identified =
                state.get("identified").and_then(|value| value.as_bool()) == Some(true);
            (ready + usize::from(connected && identified), total + 1)
        })
}

fn health_summary(health: &HealthReport) -> String {
    let (ready, total) = gateway_counts(health);
    format!(
        "service={}, core={}, event_sources={}, qq_gateways_ready={ready}/{total}",
        health.service, health.core, health.event_sources
    )
}

fn assert_clean(label: &str, bytes: &[u8], credentials: &[Vec<u8>]) {
    assert!(
        credentials
            .iter()
            .all(|value| !bytes.windows(value.len()).any(|window| window == value)),
        "credential bytes found in new {label} bytes"
    );
}

struct FileTail {
    path: PathBuf,
    offset: u64,
}

impl FileTail {
    fn snapshot(path: PathBuf) -> Self {
        let offset = std::fs::metadata(&path).map(|file| file.len()).unwrap_or(0);
        Self { path, offset }
    }

    fn new_bytes(&self) -> Vec<u8> {
        match std::fs::read(&self.path) {
            Ok(bytes) => bytes
                .get(usize::try_from(self.offset).unwrap_or(0)..)
                .unwrap_or(&bytes)
                .to_vec(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(_) => panic!("failed to read smoke audit file"),
        }
    }
}
