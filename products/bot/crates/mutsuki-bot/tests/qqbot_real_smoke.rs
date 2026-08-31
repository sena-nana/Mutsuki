// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::manual_let_else,
    clippy::map_unwrap_or,
    clippy::match_wild_err_arm,
    clippy::redundant_closure_for_method_calls,
    clippy::single_match_else
)]

#[path = "support/process.rs"]
mod process;

use std::path::PathBuf;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use mutsuki_bot::load_single_instance_product_for_test;
use mutsuki_plugin_bot_adapter_qqbot::QqBotConfig;
use mutsuki_service_config::ServiceConfig;
use mutsuki_service_control::HealthReport;
use mutsuki_web_protocol::{RpcRequest, WEB_PROTOCOL_VERSION, WireMessage};
use tempfile::Builder;
use uuid::Uuid;

use process::ProductProcess;

const QQBOT_PLUGIN_ID: &str = "mutsuki.bot.adapter.qqbot";
const QQBOT_HEALTH_PREFIX: &str = "mutsuki.bot.qqbot.gateway:";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires ignored local QQBot config and secret files"]
async fn real_qqbot_product_process_is_healthy_and_shuts_down_cleanly() {
    let executable = PathBuf::from(env!("CARGO_BIN_EXE_mutsuki-bot"));
    let instance_root = executable
        .parent()
        .expect("product executable parent")
        .join(".mutsuki-bot");
    let product = load_single_instance_product_for_test(&instance_root, "")
        .await
        .unwrap_or_else(|_| panic!("failed to load configured single-instance QQBot product"));
    let service = product.service;
    let console = product.console;
    assert!(
        console.enabled,
        "real smoke requires the authenticated console"
    );
    let auth_token = service
        .host_secret_store()
        .resolve(
            console
                .auth_token_key
                .as_deref()
                .expect("console token key"),
        )
        .expect("resolve console token");

    let credentials = credentials(&service, &auth_token);
    assert_endpoint_closed(&console.listen, "before startup").await;
    let service_log = FileTail::snapshot(service.service.log_dir.join(&service.observe.log_file));
    let panic_log = FileTail::snapshot(service.service.log_dir.join(&service.observe.panic_file));
    let output_dir = Builder::new()
        .prefix("mtk-qqbot-real-")
        .tempdir()
        .expect("create smoke output directory");
    let mut process = ProductProcess::spawn(&executable, output_dir.path().join("product.log"));

    let health_result = tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            process.assert_running();
            if let Some(health) = health(&console.listen, &auth_token).await
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
            let summary = health(&console.listen, &auth_token)
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

    rpc(&console.listen, &auth_token, "service_shutdown")
        .await
        .expect("request ServiceShutdown");
    let status = process.wait_for_exit(Duration::from_secs(30)).await;
    assert!(status.success(), "product exited with {status}");
    assert_endpoint_closed(&console.listen, "after shutdown").await;

    assert_clean("child output", &process.output_bytes(), &credentials);
    assert_clean("service log", &service_log.new_bytes(), &credentials);
    assert_clean("panic log", &panic_log.new_bytes(), &credentials);
}

fn credentials(service: &ServiceConfig, console_token: &str) -> Vec<Vec<u8>> {
    let store = service.host_secret_store();
    let mut values = vec![
        service.control_token().as_bytes().to_vec(),
        console_token.as_bytes().to_vec(),
    ];
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

async fn health(address: &str, token: &str) -> Option<HealthReport> {
    serde_json::from_value(rpc(address, token, "health").await.ok()?).ok()
}

async fn assert_endpoint_closed(address: &str, phase: &str) {
    assert!(
        tokio::net::TcpStream::connect(address).await.is_err(),
        "console endpoint is reachable {phase}"
    );
}

async fn rpc(address: &str, token: &str, method: &str) -> Result<serde_json::Value, String> {
    use tokio_tungstenite::{connect_async, tungstenite::Message};
    let (mut socket, _) = connect_async(format!("ws://{address}/ws"))
        .await
        .map_err(|error| error.to_string())?;
    socket
        .send(Message::Binary(
            WireMessage::Hello {
                protocol_version: WEB_PROTOCOL_VERSION.into(),
                capabilities: Vec::new(),
                auth_token: Some(token.into()),
            }
            .encode()
            .map_err(|error| error.to_string())?
            .into(),
        ))
        .await
        .map_err(|error| error.to_string())?;
    let _ = socket.next().await.ok_or("missing hello ack")?;
    socket
        .send(Message::Binary(
            WireMessage::Rpc(RpcRequest {
                id: Uuid::new_v4(),
                namespace: "control".into(),
                method: method.into(),
                params: serde_json::json!({}),
            })
            .encode()
            .map_err(|error| error.to_string())?
            .into(),
        ))
        .await
        .map_err(|error| error.to_string())?;
    let message = socket
        .next()
        .await
        .ok_or("missing RPC response")?
        .map_err(|error| error.to_string())?;
    let Message::Binary(bytes) = message else {
        return Err("unexpected WebSocket response".into());
    };
    match WireMessage::decode(bytes.as_ref()).map_err(|error| error.to_string())? {
        WireMessage::RpcResult(result) => result.error.map_or_else(
            || Ok(result.result.unwrap_or_default()),
            |error| Err(error.message),
        ),
        _ => Err("unexpected Web RPC response".into()),
    }
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
