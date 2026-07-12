use std::sync::Mutex;
use std::time::Duration;

use bot_echo::{echo_manifest, echo_runner};
use mutsuki_bot_protocol::{BOT_COMMAND_PARSE_PROTOCOL_ID, BotEventKind, BotEventSubscription};
use mutsuki_bot_service_host_integration::configured_bot_plugin_catalog;
use mutsuki_bot_testkit::FakeQqServer;
use mutsuki_plugin_bot_adapter_qqbot::QQBOT_ADAPTER_PLUGIN_ID;
use mutsuki_plugin_bot_command::BOT_COMMAND_PLUGIN_ID;
use mutsuki_plugin_bot_event_router::BOT_EVENT_ROUTER_PLUGIN_ID;
use mutsuki_service_config::{ConfiguredPluginSelection, IpcTransport, ServiceConfig};
use mutsuki_service_control::ControlMethod;
use mutsuki_service_runtime::ServiceRuntimeBuilder;
use serde_json::{Value, json};
use tokio::net::TcpListener;

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[tokio::test]
async fn configured_qqbot_requires_host_secret_during_service_preflight() {
    let mut service = test_service_config().await.0;
    service.plugins.configured = vec![configured_qq("MISSING_QQBOT_SECRET", json!({}))];
    let error = match ServiceRuntimeBuilder::new(service)
        .with_configured_plugin_catalog(configured_bot_plugin_catalog().unwrap())
        .start()
        .await
    {
        Ok(runtime) => {
            runtime.shutdown().await;
            panic!("QQBot ServiceRuntime unexpectedly started without Host secret")
        }
        Err(error) => error,
    };
    assert!(error.to_string().contains("MISSING_QQBOT_SECRET"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn configured_service_runtime_runs_resume_echo_ping_and_clean_shutdown() {
    let _env = ENV_LOCK.lock().unwrap();
    let fake = FakeQqServer::start().await;
    let secret_key = format!("QQBOT_TEST_SECRET_{}", fake.websocket_addr().port());
    let secret_env = format!("MUTSUKI_SECRET_{secret_key}");
    unsafe { std::env::set_var(&secret_env, "TEST_CLIENT_SECRET") };

    let (mut service, _home) = test_service_config().await;
    let control_config = service.clone();
    let qq_config =
        serde_json::to_value(fake.config("integration", "TEST_APP_ID", &secret_key)).unwrap();
    service.plugins.configured = vec![
        ConfiguredPluginSelection {
            id: BOT_EVENT_ROUTER_PLUGIN_ID.into(),
            enabled: true,
            config: json!({"subscriptions": [BotEventSubscription {
                subscription_id: "qqbot-message-to-command".into(),
                handler_protocol_id: BOT_COMMAND_PARSE_PROTOCOL_ID.into(),
                handler_binding_id: None,
                platform: Some("qqbot".into()),
                event_kind: Some(BotEventKind::MessageCreated),
            }]}),
        },
        ConfiguredPluginSelection {
            id: BOT_COMMAND_PLUGIN_ID.into(),
            enabled: true,
            config: json!({"prefixes": ["/"]}),
        },
        ConfiguredPluginSelection {
            id: QQBOT_ADAPTER_PLUGIN_ID.into(),
            enabled: true,
            config: qq_config,
        },
    ];

    let runtime = ServiceRuntimeBuilder::new(service)
        .with_configured_plugin_catalog(configured_bot_plugin_catalog().unwrap())
        .register_builtin_plugin(echo_manifest(1))
        .register_builtin_runner(|| echo_runner(1))
        .start()
        .await
        .unwrap();

    let sends = fake.wait_for_sends(2, Duration::from_secs(5)).await;
    assert_eq!(sends[0]["content"], "hello");
    assert_eq!(sends[1]["content"], "pong");

    let plugins = control(&control_config, ControlMethod::PluginList).await;
    let plugin_json = plugins.to_string();
    for id in [
        QQBOT_ADAPTER_PLUGIN_ID,
        BOT_EVENT_ROUTER_PLUGIN_ID,
        BOT_COMMAND_PLUGIN_ID,
        "example.bot.echo",
    ] {
        assert!(plugin_json.contains(id), "missing configured plugin {id}");
    }
    let health = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let health = control(&control_config, ControlMethod::HealthCheck).await;
            if health["event_sources"] == "ok" {
                break health;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("QQ Gateway health becomes ready");
    assert_eq!(health["service"], "ok");
    assert_eq!(health["event_sources"], "ok");
    assert_eq!(
        health["components"]["mutsuki.bot.qqbot.gateway:integration"]["connected"],
        true
    );
    let task_json = control(&control_config, ControlMethod::TaskList)
        .await
        .to_string();
    assert!(task_json.contains("echo-1"));
    assert!(task_json.contains("ping-1"));
    assert!(!task_json.contains("TEST_CLIENT_SECRET"));
    assert!(!task_json.contains("TEST_ACCESS_TOKEN"));

    control(&control_config, ControlMethod::ServiceShutdown).await;
    runtime
        .run_until_shutdown_signal(std::future::pending::<String>())
        .await
        .unwrap();
    let snapshot = fake.shutdown().await;
    unsafe { std::env::remove_var(&secret_env) };
    assert_eq!(snapshot.websocket_connections, 2);
    assert_eq!(snapshot.gateway_auth_frames[0]["op"], 2);
    assert_eq!(snapshot.gateway_auth_frames[1]["op"], 6);
    assert!(snapshot.account_checks >= 2);
    assert_eq!(snapshot.clean_closes, 1);
}

fn configured_qq(secret_key: &str, overrides: Value) -> ConfiguredPluginSelection {
    let mut config = serde_json::to_value(mutsuki_plugin_bot_adapter_qqbot::QqBotConfig::new(
        "configured",
        "TEST_APP_ID",
    ))
    .unwrap();
    config["client_secret_key"] = Value::String(secret_key.into());
    if let (Some(base), Some(overrides)) = (config.as_object_mut(), overrides.as_object()) {
        base.extend(overrides.clone());
    }
    ConfiguredPluginSelection {
        id: QQBOT_ADAPTER_PLUGIN_ID.into(),
        enabled: true,
        config,
    }
}

async fn test_service_config() -> (ServiceConfig, tempfile::TempDir) {
    let home = tempfile::tempdir().unwrap();
    let probe = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = probe.local_addr().unwrap();
    drop(probe);
    let mut service = ServiceConfig::default();
    service.service.instance_id = "qqbot-integration".into();
    service.service.home_dir = home.path().to_path_buf();
    service.service.log_dir = home.path().join("logs");
    service.service.run_dir = home.path().join("run");
    service.plugins.builtin.clear();
    service.plugins.dynamic_dirs.clear();
    service.plugins.disabled_dir = home.path().join("disabled");
    service.observe.console = false;
    service.ipc.enabled = true;
    service.ipc.transport = IpcTransport::TcpDebug;
    service.ipc.tcp_debug_addr = Some(address.to_string());
    service.ipc.token = Some("test-token".into());
    std::fs::create_dir_all(&service.service.log_dir).unwrap();
    std::fs::create_dir_all(&service.service.run_dir).unwrap();
    (service, home)
}

async fn control(config: &ServiceConfig, method: ControlMethod) -> Value {
    let client = mutsuki_service_ipc::ControlClient::new(config.into());
    let response = client.request(method, Value::Null).await.unwrap();
    assert!(response.ok, "control failed: {:?}", response.error);
    response.result.unwrap_or(Value::Null)
}
