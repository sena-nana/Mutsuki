//! Embedded console assembly smoke (control + overview over fixture control plane).

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use mutsuki_bot_web_console::{
    SecretKeyResolver, SecretMonitor, WebConsoleConfig, WebConsolePaths, WebConsoleSecrets,
    build_console_host, demo_config_service, empty_config_service,
};
use mutsuki_plugin_bot_control_web::FixtureControlHandler;
use mutsuki_web_host::WebHost;
use mutsuki_web_protocol::{RpcRequest, WEB_PROTOCOL_VERSION, WireMessage};
use serde_json::json;
use uuid::Uuid;

static LINK_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn link_test_lock() -> std::sync::MutexGuard<'static, ()> {
    LINK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[tokio::test]
async fn embedded_console_reads_overview_and_control() {
    let config = WebConsoleConfig {
        enabled: true,
        listen: "127.0.0.1:0".into(),
        auth_token_key: None,
        include_config: false,
        ..Default::default()
    };
    let secrets = WebConsoleSecrets {
        auth_token: "local-dev".into(),
    };
    let (mut host, _dirs) = build_console_host(
        &config,
        &secrets,
        Arc::new(FixtureControlHandler::default()),
        "local-dev",
        None,
        None,
        &WebConsolePaths::default(),
    )
    .unwrap();
    host.start().await.unwrap();
    let addr = host.listen_addr().unwrap().to_string();

    let summary = ws_rpc(&addr, "overview", "summary").await.unwrap();
    assert_eq!(summary["service"]["instance_id"], "demo");

    let health = ws_rpc(&addr, "control", "health").await.unwrap();
    assert_eq!(health["service"], "ok");

    let logs = ws_rpc_params(&addr, "control", "log_tail", json!({"lines": 5}))
        .await
        .unwrap();
    assert!(logs["entries"].is_array());

    host.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test]
async fn embedded_console_with_config_shell() {
    let config = WebConsoleConfig {
        enabled: true,
        listen: "127.0.0.1:0".into(),
        auth_token_key: None,
        include_config: true,
        ..Default::default()
    };
    let secrets = WebConsoleSecrets {
        auth_token: "local-dev".into(),
    };
    let (mut host, _dirs) = build_console_host(
        &config,
        &secrets,
        Arc::new(FixtureControlHandler::default()),
        "local-dev",
        Some(empty_config_service()),
        None,
        &WebConsolePaths::default(),
    )
    .unwrap();
    host.start().await.unwrap();
    let providers = ws_rpc(
        &host.listen_addr().unwrap().to_string(),
        "config",
        "providers.list",
    )
    .await
    .unwrap();
    assert_eq!(providers.as_array().unwrap().len(), 0);
    host.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test]
async fn embedded_console_demo_config_provider_is_usable() {
    let config = WebConsoleConfig {
        enabled: true,
        listen: "127.0.0.1:0".into(),
        auth_token_key: None,
        include_config: true,
        ..Default::default()
    };
    let secrets = WebConsoleSecrets {
        auth_token: "local-dev".into(),
    };
    let (mut host, _dirs) = build_console_host(
        &config,
        &secrets,
        Arc::new(FixtureControlHandler::default()),
        "local-dev",
        Some(demo_config_service()),
        None,
        &WebConsolePaths::default(),
    )
    .unwrap();
    host.start().await.unwrap();
    let providers = ws_rpc(
        &host.listen_addr().unwrap().to_string(),
        "config",
        "providers.list",
    )
    .await
    .unwrap();
    assert_eq!(providers.as_array().unwrap(), &vec![json!("product")]);
    host.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test]
async fn embedded_console_starts_upgrade_extension_when_release_set_configured() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("mutsuki-plugin-catalog")
        .join("tests")
        .join("fixtures");
    let config = WebConsoleConfig {
        enabled: true,
        listen: "127.0.0.1:0".into(),
        auth_token_key: None,
        include_config: false,
        release_set: Some(root.join("release-set.toml").to_string_lossy().into()),
        ..Default::default()
    };
    let secrets = WebConsoleSecrets {
        auth_token: "local-dev".into(),
    };
    let paths = WebConsolePaths {
        release_set: config.release_set.as_ref().map(std::path::PathBuf::from),
    };
    let (mut host, _dirs) = build_console_host(
        &config,
        &secrets,
        Arc::new(FixtureControlHandler::default()),
        "local-dev",
        None,
        None,
        &paths,
    )
    .unwrap();
    host.start().await.unwrap();
    assert!(host.listen_addr().is_some());
    host.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

struct MapSecretResolver {
    values: std::collections::BTreeMap<String, String>,
}

impl SecretKeyResolver for MapSecretResolver {
    fn resolve(&self, key: &str) -> Option<String> {
        self.values.get(key).cloned()
    }
}

#[tokio::test]
async fn embedded_console_secret_status_is_read_only() {
    let config = WebConsoleConfig {
        enabled: true,
        listen: "127.0.0.1:0".into(),
        auth_token_key: Some("WEB_CONSOLE_AUTH_TOKEN".into()),
        include_config: false,
        ..Default::default()
    };
    let secrets = WebConsoleSecrets {
        auth_token: "local-dev".into(),
    };
    let monitor = SecretMonitor::new(
        vec!["WEB_CONSOLE_AUTH_TOKEN".into(), "MISSING_KEY".into()],
        Arc::new(MapSecretResolver {
            values: [("WEB_CONSOLE_AUTH_TOKEN".into(), "configured".into())].into(),
        }),
    );
    let (mut host, _dirs) = build_console_host(
        &config,
        &secrets,
        Arc::new(FixtureControlHandler::default()),
        "local-dev",
        None,
        Some(monitor),
        &WebConsolePaths::default(),
    )
    .unwrap();
    host.start().await.unwrap();
    let status = ws_rpc(&host.listen_addr().unwrap().to_string(), "secret", "status")
        .await
        .unwrap();
    let secrets = status["secrets"].as_array().unwrap();
    assert_eq!(secrets.len(), 2);
    assert_eq!(secrets[0]["state"], "present");
    assert_eq!(secrets[1]["state"], "absent");
    host.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn standalone_console_fails_loud_when_link_runtime_absent() {
    use mutsuki_bot_web_console::{
        StandaloneConsoleSpec, WebConsolePaths, build_standalone_console_host,
    };
    use mutsuki_service_link::STANDALONE_LINK_CONNECT_FAILED;

    let _guard = link_test_lock();
    let spec = StandaloneConsoleSpec {
        listen: "127.0.0.1:0".into(),
        link_endpoint: "local://mutsuki.nolink.test".into(),
        auth_token: "local-dev".into(),
        include_config: false,
        include_upgrade: false,
        quic_tls: None,
    };
    let (mut host, _dirs) =
        build_standalone_console_host(&spec, &WebConsolePaths::default()).unwrap();
    host.start().await.unwrap();
    let addr = host.listen_addr().unwrap().to_string();

    let err = ws_rpc(&addr, "control", "health").await.unwrap_err();
    assert!(
        err.contains(STANDALONE_LINK_CONNECT_FAILED),
        "expected connect failure, got: {err}"
    );

    host.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn standalone_link_control_rpc_reaches_service_host() {
    use std::sync::Arc;

    use mutsuki_bot_web_console::{
        StandaloneConsoleSpec, WebConsolePaths, build_standalone_console_host,
    };
    use mutsuki_service_control::{
        ControlError, ControlFuture, ControlHandler, ControlMethod, ControlRequest,
        ControlResponse, HealthReport, ServiceStatus,
    };
    use mutsuki_service_link::{LinkControlServer, SERVICE_LINK_APP_ID};
    use tempfile::tempdir;

    let _guard = link_test_lock();

    struct StandaloneFixture;

    impl ControlHandler for StandaloneFixture {
        fn handle(&self, request: ControlRequest) -> ControlFuture {
            Box::pin(async move {
                if request.token != "local-dev" {
                    return ControlResponse::err(ControlError::Unauthorized);
                }
                match request.method {
                    ControlMethod::HealthCheck => ControlResponse::ok(HealthReport {
                        service: "ok".into(),
                        core: "ok".into(),
                        plugins: "ok".into(),
                        runners: "ok".into(),
                        event_sources: "ok".into(),
                        event_source_details: Vec::new(),
                        recent_errors: Vec::new(),
                        components: Default::default(),
                    }),
                    ControlMethod::ServiceStatus => ControlResponse::ok(ServiceStatus {
                        instance_id: "standalone-link".into(),
                        profile: "standalone".into(),
                        uptime_ms: 1,
                        ipc_endpoint: format!("local://{SERVICE_LINK_APP_ID}"),
                        core_running: true,
                        plugin_count: 0,
                        runner_count: 0,
                    }),
                    other => ControlResponse::err(ControlError::Unsupported(format!("{other:?}"))),
                }
            })
        }
    }

    let dir = tempdir().unwrap();
    let _link =
        LinkControlServer::start(dir.path(), "standalone-link", Arc::new(StandaloneFixture))
            .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let spec = StandaloneConsoleSpec {
        listen: "127.0.0.1:0".into(),
        link_endpoint: format!("local://{SERVICE_LINK_APP_ID}"),
        auth_token: "local-dev".into(),
        include_config: false,
        include_upgrade: false,
        quic_tls: None,
    };
    let (mut host, _dirs) =
        build_standalone_console_host(&spec, &WebConsolePaths::default()).unwrap();
    host.start().await.unwrap();
    let addr = host.listen_addr().unwrap().to_string();

    let health = ws_rpc(&addr, "control", "health").await.unwrap();
    assert_eq!(health["service"], "ok");
    let status = ws_rpc(&addr, "control", "service_status").await.unwrap();
    assert_eq!(status["instance_id"], "standalone-link");

    host.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test]
async fn standalone_console_requires_link_endpoint() {
    use mutsuki_bot_web_console::{
        StandaloneConsoleSpec, WebConsolePaths, build_standalone_console_host,
    };

    let spec = StandaloneConsoleSpec {
        listen: "127.0.0.1:0".into(),
        link_endpoint: "   ".into(),
        auth_token: "local-dev".into(),
        include_config: false,
        include_upgrade: false,
        quic_tls: None,
    };
    assert!(build_standalone_console_host(&spec, &WebConsolePaths::default()).is_err());
}

#[tokio::test]
async fn standalone_quic_requires_tls_identity() {
    use mutsuki_bot_web_console::{
        StandaloneConsoleSpec, WebConsolePaths, build_standalone_console_host,
    };

    let spec = StandaloneConsoleSpec {
        listen: "127.0.0.1:0".into(),
        link_endpoint: "quic://127.0.0.1:4433".into(),
        auth_token: "local-dev".into(),
        include_config: false,
        include_upgrade: false,
        quic_tls: None,
    };
    let err = match build_standalone_console_host(&spec, &WebConsolePaths::default()) {
        Ok(_) => panic!("expected tls identity failure"),
        Err(error) => error,
    };
    assert!(
        err.to_string().contains("quic_tls"),
        "expected tls identity failure, got: {err}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn standalone_quic_link_control_rpc_roundtrip() {
    use std::sync::Arc;
    use std::time::Duration;

    use mutsuki_bot_web_console::{
        StandaloneConsoleSpec, StandaloneQuicTlsIdentity, WebConsolePaths,
        build_standalone_console_host,
    };
    use mutsuki_service_control::{
        ControlError, ControlFuture, ControlHandler, ControlMethod, ControlRequest,
        ControlResponse, HealthReport, ServiceStatus,
    };
    use mutsuki_service_link::{QuicLinkControlServer, server_config_from_pem};

    let _guard = link_test_lock();

    struct QuicFixture;

    impl ControlHandler for QuicFixture {
        fn handle(&self, request: ControlRequest) -> ControlFuture {
            Box::pin(async move {
                if request.token != "local-dev" {
                    return ControlResponse::err(ControlError::Unauthorized);
                }
                match request.method {
                    ControlMethod::HealthCheck => ControlResponse::ok(HealthReport {
                        service: "ok".into(),
                        core: "ok".into(),
                        plugins: "ok".into(),
                        runners: "ok".into(),
                        event_sources: "ok".into(),
                        event_source_details: Vec::new(),
                        recent_errors: Vec::new(),
                        components: Default::default(),
                    }),
                    ControlMethod::ServiceStatus => ControlResponse::ok(ServiceStatus {
                        instance_id: "standalone-quic".into(),
                        profile: "standalone".into(),
                        uptime_ms: 1,
                        ipc_endpoint: "quic://127.0.0.1:0".into(),
                        core_running: true,
                        plugin_count: 0,
                        runner_count: 0,
                    }),
                    other => ControlResponse::err(ControlError::Unsupported(format!("{other:?}"))),
                }
            })
        }
    }

    let generated = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
    let cert_pem = generated.cert.pem();
    let key_pem = generated.key_pair.serialize_pem();
    let server = QuicLinkControlServer::start(
        "127.0.0.1:0".parse().unwrap(),
        server_config_from_pem(&cert_pem, &key_pem).unwrap(),
        Arc::new(QuicFixture),
    )
    .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let spec = StandaloneConsoleSpec {
        listen: "127.0.0.1:0".into(),
        link_endpoint: format!("quic://{}", server.local_addr()),
        auth_token: "local-dev".into(),
        include_config: false,
        include_upgrade: false,
        quic_tls: Some(StandaloneQuicTlsIdentity {
            server_name: "localhost".into(),
            ca_cert_pem: cert_pem,
        }),
    };
    let (mut host, _dirs) =
        build_standalone_console_host(&spec, &WebConsolePaths::default()).unwrap();
    host.start().await.unwrap();
    let addr = host.listen_addr().unwrap().to_string();

    let health = ws_rpc(&addr, "control", "health").await.unwrap();
    assert_eq!(health["service"], "ok");
    let status = ws_rpc(&addr, "control", "service_status").await.unwrap();
    assert_eq!(status["instance_id"], "standalone-quic");

    host.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

async fn ws_rpc(addr: &str, namespace: &str, method: &str) -> Result<serde_json::Value, String> {
    ws_rpc_params(addr, namespace, method, json!({})).await
}

async fn ws_rpc_params(
    addr: &str,
    namespace: &str,
    method: &str,
    extra: serde_json::Value,
) -> Result<serde_json::Value, String> {
    use tokio_tungstenite::{connect_async, tungstenite::Message};
    let (mut ws, _) = connect_async(format!("ws://{addr}/ws")).await.expect("ws");
    ws.send(Message::Text(
        serde_json::to_string(&WireMessage::Hello {
            protocol_version: WEB_PROTOCOL_VERSION.into(),
            capabilities: vec!["runtime.read".into(), "*".into()],
            auth_token: Some("local-dev".into()),
        })
        .unwrap()
        .into(),
    ))
    .await
    .unwrap();
    let Message::Text(ack) = ws.next().await.unwrap().unwrap() else {
        panic!("ack");
    };
    assert!(matches!(
        serde_json::from_str::<WireMessage>(&ack).unwrap(),
        WireMessage::HelloAck { .. }
    ));
    let id = Uuid::new_v4();
    let mut params = extra;
    if let Some(obj) = params.as_object_mut() {
        obj.entry("capabilities")
            .or_insert(json!(["runtime.read", "runtime.write", "*"]));
    }
    ws.send(Message::Text(
        serde_json::to_string(&WireMessage::Rpc(RpcRequest {
            id,
            namespace: namespace.into(),
            method: method.into(),
            params,
        }))
        .unwrap()
        .into(),
    ))
    .await
    .unwrap();
    let Message::Text(text) = ws.next().await.unwrap().unwrap() else {
        panic!("rpc");
    };
    match serde_json::from_str::<WireMessage>(&text).unwrap() {
        WireMessage::RpcResult(result) => match result.error {
            Some(error) => Err(error.message),
            None => Ok(result.result.unwrap_or(serde_json::Value::Null)),
        },
        other => panic!("unexpected {other:?}"),
    }
}
