use std::time::{Duration, Instant};

use bot_echo::{echo_manifest, echo_runner};
use mutsuki_bot_service_host_integration::configured_bot_plugin_catalog;
use mutsuki_bot_testkit::FakeQqServer;
use mutsuki_service_config::{ConfigOverrides, ServiceConfig};
use mutsuki_service_runtime::ServiceRuntimeBuilder;
use serde_json::json;

use crate::measurement::{Sample, allocation_delta, allocation_snapshot, process_cpu_time_ns};

pub fn reconnect_sample() -> Sample {
    let allocation_start = allocation_snapshot();
    let started = Instant::now();
    let run = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(run_connection_workload(None));
    let elapsed_ns = started.elapsed().as_nanos();
    let (allocations, allocated_bytes) = allocation_delta(allocation_start);
    Sample {
        elapsed_ns,
        cpu_time_ns: 0,
        idle_cpu_time_ns: 0,
        simulated_platform_ns: 10_000_000,
        events: 2,
        queue_depth: 1,
        dropped: 0,
        deferred: 1,
        retried: 1,
        fairness: 1.0,
        duplicate_executions: 0,
        retained_units: 0,
        output: run.output,
        allocations,
        allocated_bytes,
    }
}

pub fn connection_idle_sample(idle_window_ms: u64) -> Sample {
    let run = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(run_connection_workload(Some(Duration::from_millis(
            idle_window_ms,
        ))));
    Sample {
        elapsed_ns: run.idle_elapsed_ns,
        // Idle case owns its CPU boundary: only the post-resume idle window counts.
        // The outer harness must not wrap setup/reconnect/shutdown into cpu_time_ns.
        cpu_time_ns: run.idle_cpu_time_ns,
        idle_cpu_time_ns: run.idle_cpu_time_ns,
        simulated_platform_ns: u128::from(idle_window_ms) * 1_000_000,
        events: 0,
        queue_depth: 0,
        dropped: 0,
        deferred: 0,
        retried: 0,
        fairness: 1.0,
        duplicate_executions: 0,
        retained_units: 1,
        output: json!({
            "idle_window_ms": idle_window_ms,
            "connections": run.output["connections"],
            "auth_ops": run.output["auth_ops"],
            "clean_closes": run.output["clean_closes"]
        }),
        allocations: run.idle_allocations,
        allocated_bytes: run.idle_allocated_bytes,
    }
}

struct ConnectionRun {
    output: serde_json::Value,
    idle_elapsed_ns: u128,
    idle_cpu_time_ns: u128,
    idle_allocations: u64,
    idle_allocated_bytes: u64,
}

async fn run_connection_workload(idle_window: Option<Duration>) -> ConnectionRun {
    let fake = FakeQqServer::start().await;
    let secret_key = format!("QQBOT_BENCHMARK_SECRET_{}", fake.websocket_addr().port());
    let home = tempfile::tempdir().unwrap();
    let qq = fake.config("benchmark", "BENCHMARK_APP", &secret_key);
    std::fs::write(
        home.path().join("local.secret.toml"),
        format!("[secrets]\n{secret_key} = \"BENCHMARK_CLIENT_SECRET\"\n"),
    )
    .unwrap();
    let config_path = home.path().join("local.toml");
    std::fs::write(&config_path, product_toml(home.path(), &qq)).unwrap();
    let service = ServiceConfig::load(ConfigOverrides {
        config_file: Some(config_path),
        ..Default::default()
    })
    .unwrap();
    let runtime = ServiceRuntimeBuilder::new(service)
        .with_configured_plugin_catalog(configured_bot_plugin_catalog().unwrap())
        .register_builtin_plugin(echo_manifest(1))
        .register_builtin_runner(|| echo_runner(1))
        .start()
        .await
        .unwrap();
    let sends = fake.wait_for_sends(2, Duration::from_secs(5)).await;
    let idle_allocation_start = allocation_snapshot();
    let idle_cpu_start = process_cpu_time_ns();
    let idle_started = Instant::now();
    if let Some(idle_window) = idle_window {
        tokio::time::sleep(idle_window).await;
    }
    let idle_elapsed_ns = idle_started.elapsed().as_nanos();
    let idle_cpu_time_ns = process_cpu_time_ns().saturating_sub(idle_cpu_start);
    let (idle_allocations, idle_allocated_bytes) = allocation_delta(idle_allocation_start);
    runtime.shutdown().await;
    let snapshot = fake.shutdown().await;
    assert_eq!(snapshot.websocket_connections, 2);
    assert_eq!(snapshot.gateway_auth_frames[0]["op"], 2);
    assert_eq!(snapshot.gateway_auth_frames[1]["op"], 6);
    assert_eq!(snapshot.clean_closes, 1);
    let output = json!({
        "sends": sends
            .iter()
            .map(|send| send["content"].clone())
            .collect::<Vec<_>>(),
        "connections": snapshot.websocket_connections,
        "auth_ops": snapshot
            .gateway_auth_frames
            .iter()
            .map(|frame| frame["op"].clone())
            .collect::<Vec<_>>(),
        "account_checks_at_least_two": snapshot.account_checks >= 2,
        "clean_closes": snapshot.clean_closes
    });
    ConnectionRun {
        output,
        idle_elapsed_ns,
        idle_cpu_time_ns,
        idle_allocations,
        idle_allocated_bytes,
    }
}

fn product_toml(
    root: &std::path::Path,
    qq: &mutsuki_plugin_bot_adapter_qqbot::QqBotConfig,
) -> String {
    format!(
        r#"[service]
profile = "bot-benchmark"
instance_id = "bot-benchmark"
home_dir = "{}"
data_dir = "data"
log_dir = "logs"
plugin_dir = "plugins"
run_dir = "run"

[ipc]
enabled = false
transport = "tcp-debug"
name = "bot-benchmark"
token = "benchmark-token"

[plugins]
dynamic_dirs = []
disabled_dir = "disabled"

[[plugins.configured]]
id = "mutsuki.bot.router.event"
[plugins.configured.config]
subscriptions = [{{ subscription_id = "qq-command", handler_protocol_id = "mutsuki.bot.command/parse@1", platform = "qqbot", event_kind = "message_created" }}]

[[plugins.configured]]
id = "mutsuki.bot.command"
[plugins.configured.config]
prefixes = ["/"]

[[plugins.configured]]
id = "mutsuki.bot.adapter.qqbot"
[plugins.configured.config]
account_id = "{}"
app_id = "{}"
client_secret_key = "{}"
token_url = "{}"
openapi_base_url = "{}"
allow_insecure_transport = true
gateway_hello_timeout_ms = 1000
gateway_ack_timeout_ms = 500
retry_base_delay_ms = 0
retry_max_delay_ms = 0
reconnect_initial_delay_ms = 10
reconnect_max_delay_ms = 20
reconnect_jitter_ms = 0

[[plugins.configured]]
id = "example.bot.echo"

[security]
secret_file = "local.secret.toml"

[observe]
console = false
json = false
log_file = "service.log"
panic_file = "panic.log"
"#,
        root.to_string_lossy().replace('\\', "/"),
        qq.account_id,
        qq.app_id,
        qq.client_secret_key,
        qq.token_url,
        qq.openapi_base_url,
    )
}
