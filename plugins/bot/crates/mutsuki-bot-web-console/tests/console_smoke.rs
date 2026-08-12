//! Embedded console assembly smoke (control + overview over fixture control plane).

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use mutsuki_bot_web_console::{
    ControlPluginReloadLifecycle, ProductConfigOptions, SecretKeyResolver, SecretMonitor,
    WebConsoleConfig, WebConsolePaths, WebConsoleSecrets, build_console_host, demo_config_service,
    empty_config_service, product_config_service_with_options,
};
use mutsuki_plugin_bot_agent::BotAgentConfigHandle;
use mutsuki_plugin_bot_control_web::FixtureControlHandler;
use mutsuki_web_host::WebHost;
use mutsuki_web_protocol::{RpcRequest, WEB_PROTOCOL_VERSION, WireMessage};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

#[test]
fn console_css_declares_two_column_workspace() {
    let css = include_str!("../assets/mutsuki-ui.css");
    assert!(
        !css.contains("@import"),
        "mutsuki-ui.css must fully inline @import for static include_str serving"
    );
    assert!(css.contains(".mutsuki-console.lilia-workspace"));
    assert!(css.contains("--mutsuki-nav-width: 232px"));
    assert!(css.contains("grid-template-columns: var(--mutsuki-nav-width) minmax(0, 1fr)"));
    assert!(css.contains("display: grid"));
    assert!(css.contains(".lilia-workspace"));
    assert!(css.contains(".secondary-panel"));
    assert!(css.contains(".page-header"));
    assert!(css.contains(".page-scroll"));
    assert!(css.contains(".card"));
    assert!(css.contains(".kv"));
}

#[tokio::test]
async fn embedded_console_serves_workspace_css_and_shell_markup() {
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
        None,
        None,
    )
    .unwrap();
    host.start().await.unwrap();
    let addr = host.listen_addr().unwrap().to_string();

    let css = http_get_body(&addr, "/mutsuki-ui.css").await;
    assert!(css.contains("grid-template-columns: var(--mutsuki-nav-width) minmax(0, 1fr)"));
    assert!(css.contains(".mutsuki-console.lilia-workspace"));
    assert!(!css.contains("@import"));

    let js = http_get_body(&addr, "/index.js").await;
    assert!(js.contains("mutsuki-console lilia-workspace"));
    assert!(js.contains("secondary-panel"));
    assert!(js.contains("page-header"));
    assert!(js.contains("className = \"card\""));
    assert!(
        !js.contains("card--flat"),
        "overview content groups must use raised Lilia .card, not transparent card--flat"
    );
    assert!(js.contains("<ul class=\"kv\">") || js.contains("className = \"kv\""));
    assert!(js.contains("系统状态"));
    assert!(js.contains("overview-dashboard"));
    assert!(js.contains("metric-grid"));
    assert!(js.contains("运行时间"));
    assert!(js.contains("主机资源"));
    assert!(js.contains("健康组件"));
    assert!(js.contains("密钥状态"));
    assert!(js.contains("运行时"));
    assert!(js.contains("topology-graph"));
    assert!(js.contains("advanced-fold"));
    assert!(js.contains("mountConfigPanel"));
    assert!(!js.contains("label: \"Runners\""));
    assert!(!js.contains("label: \"EventSources\""));
    assert!(css.contains(".mutsuki-console .overview-dashboard"));
    assert!(css.contains(".mutsuki-console .metric-grid"));
    assert!(css.contains(".mutsuki-console .tab-bar"));
    assert!(css.contains(".mutsuki-console .topology-graph"));

    let html = http_get_body(&addr, "/").await;
    assert!(html.contains("mutsuki-ui.css?v="));
    assert!(html.contains("console-bootstrap.js?v="));

    let bootstrap = http_get_body(&addr, "/console-bootstrap.js").await;
    assert!(bootstrap.contains("index.js?v="));
    assert!(bootstrap.contains("createWebShellRuntime"));
    assert!(!bootstrap.contains("new WebSocket"));
    assert!(!bootstrap.contains("JSON.stringify"));
    assert!(
        http_get_body(&addr, "/shared/web-sdk.js")
            .await
            .contains("WebBridgeClient")
    );
    assert!(
        http_get_body(&addr, "/shared/web-shell.js")
            .await
            .contains("createWebShellRuntime")
    );

    host.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
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
        None,
        None,
    )
    .unwrap();
    host.start().await.unwrap();
    let addr = host.listen_addr().unwrap().to_string();

    let summary = ws_rpc(&addr, "overview", "summary").await.unwrap();
    assert_eq!(summary["service"]["instance_id"], "demo");
    assert_eq!(summary["host"]["unavailable"], false);
    assert_eq!(summary["host"]["available"], true);
    assert_eq!(summary["host"]["pid"], 4242);
    assert!(summary["host"]["rss_bytes"].as_u64().unwrap() > 0);

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
        None,
        None,
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

    let addr = host.listen_addr().unwrap().to_string();
    let bootstrap = http_get_body(&addr, "/console-bootstrap.js").await;
    assert!(
        bootstrap.contains("mountConsole"),
        "config-enabled console must stay on the overview shell"
    );
    assert!(
        !bootstrap.contains("page === \"config\""),
        "config must not remount a separate shell via ?page=config"
    );
    let config_js = http_get_body(&addr, "/config/index.js").await;
    assert!(config_js.contains("export function mountConfigPanel"));

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
        None,
        None,
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
async fn embedded_console_manages_bot_agent_provider_over_web_rpc() {
    let root = tempfile::tempdir().unwrap();
    let product_path = root.path().join("product.toml");
    std::fs::write(
        &product_path,
        r#"
[service]
profile = "bot"
instance_id = "demo"

[[plugins.configured]]
id = "mutsuki.plugin.bot.agent"
config = { enabled = true, connection_id = "primary", default_profile_id = "from-web", streaming = "final_only", max_concurrency = 2, timeout_ms = 10000, max_message_bytes = 1200 }
"#,
    )
    .unwrap();

    let control = Arc::new(FixtureControlHandler::default());
    let bot_agent_config = BotAgentConfigHandle::default();
    let service = product_config_service_with_options(
        &product_path,
        ProductConfigOptions {
            lifecycle: Some(Arc::new(ControlPluginReloadLifecycle::new(
                control.clone(),
                "fixture",
            ))),
            bot_agent_config: Some(bot_agent_config.clone()),
            ..Default::default()
        },
    )
    .unwrap();
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
        control.clone(),
        "local-dev",
        Some(service),
        None,
        &WebConsolePaths::default(),
        None,
        None,
    )
    .unwrap();
    host.start().await.unwrap();
    let addr = host.listen_addr().unwrap().to_string();

    let providers = ws_rpc(&addr, "config", "providers.list").await.unwrap();
    assert!(
        providers
            .as_array()
            .unwrap()
            .iter()
            .any(|provider| provider == "mutsuki.plugin.bot.agent")
    );

    let schema = ws_rpc_params(
        &addr,
        "config",
        "schema.get",
        json!({"provider_id":"mutsuki.plugin.bot.agent"}),
    )
    .await
    .unwrap();
    assert_eq!(schema["provider_id"], "mutsuki.plugin.bot.agent");
    assert!(
        schema["root"]["children"]
            .as_array()
            .unwrap()
            .iter()
            .any(|field| field["key"] == "streaming")
    );

    let snapshot = ws_rpc_params(
        &addr,
        "config",
        "snapshot.read",
        json!({
            "provider_id":"mutsuki.plugin.bot.agent",
            "context":{"scope":"plugin_instance","plugin_instance_id":"default"}
        }),
    )
    .await
    .unwrap();
    assert_eq!(snapshot["revision"], 1);

    let mut invalid_candidate = snapshot["value"].clone();
    invalid_candidate["value"]["max_concurrency"]["value"] = json!(0);
    let invalid_apply = ws_rpc_params(
        &addr,
        "config",
        "apply",
        json!({
            "provider_id":"mutsuki.plugin.bot.agent",
            "context":{"scope":"plugin_instance","plugin_instance_id":"default"},
            "request":{
                "expected_revision":snapshot["revision"],
                "candidate":invalid_candidate
            }
        }),
    )
    .await;
    assert!(invalid_apply.is_err());
    assert_eq!(bot_agent_config.snapshot().max_concurrency, 2);
    let generation_before_apply = bot_agent_config.versioned_snapshot().generation;

    let mut candidate = snapshot["value"].clone();
    candidate["value"]["streaming"]["value"] = json!("segment_messages");
    candidate["value"]["max_concurrency"]["value"] = json!(3);
    candidate["value"]["timeout_ms"]["value"] = json!(30000);

    let applied = ws_rpc_params(
        &addr,
        "config",
        "apply",
        json!({
            "provider_id":"mutsuki.plugin.bot.agent",
            "context":{"scope":"plugin_instance","plugin_instance_id":"default"},
            "request":{
                "expected_revision":snapshot["revision"],
                "candidate":candidate
            }
        }),
    )
    .await
    .unwrap();
    assert_eq!(applied["applied"], true);
    assert_eq!(applied["restart_policy"], "plugin_reload");
    assert!(
        applied["actions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|action| action == "plugin_reloaded")
    );
    assert!(applied["pending_actions"].as_array().unwrap().is_empty());
    assert_eq!(bot_agent_config.snapshot().streaming, "segment_messages");
    assert_eq!(bot_agent_config.snapshot().max_concurrency, 3);
    assert_eq!(bot_agent_config.snapshot().timeout_ms, 30000);
    assert_eq!(
        bot_agent_config.versioned_snapshot().generation,
        generation_before_apply + 1
    );
    assert!(
        control
            .mutations
            .lock()
            .unwrap()
            .iter()
            .any(|mutation| mutation == "plugin_reload")
    );

    let persisted: toml::Value =
        toml::from_str(&std::fs::read_to_string(&product_path).unwrap()).unwrap();
    assert_eq!(
        persisted["plugins"]["configured"][0]["config"]["streaming"].as_str(),
        Some("segment_messages")
    );

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
        None,
        None,
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
        None,
        None,
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

#[tokio::test]
async fn embedded_console_mounts_qq_management_extension() {
    use mutsuki_bot_protocol::{
        BotConversationKind, QqBotCapabilityMatrix, QqMessageSegmentKind, QqPermissionRequirement,
        QqRateLimitPolicy, QqStreamingStrategy, QqUploadConstraints,
    };
    use mutsuki_plugin_bot_qq_web::{
        LocalQqManagementProvider, QqBotManagementApi, QqBotManagementService,
        account_view_from_config,
    };

    let local = LocalQqManagementProvider::new();
    local.upsert_account(account_view_from_config(
        "main",
        "QQBOT_CLIENT_SECRET",
        true,
        QqBotCapabilityMatrix {
            account_id: "main".into(),
            conversation_kinds: vec![BotConversationKind::Private],
            outbound_conversation_kinds: vec![BotConversationKind::Private],
            active_message_kinds: vec![BotConversationKind::Private],
            inbound_segments: vec![QqMessageSegmentKind::Text],
            outbound_segments: vec![QqMessageSegmentKind::Text],
            inbound_media: Vec::new(),
            outbound_media: Vec::new(),
            active_message: true,
            message_edit: false,
            message_recall: true,
            reply: true,
            quote: true,
            mention: true,
            upload: QqUploadConstraints::default(),
            rate_limit: QqRateLimitPolicy::default(),
            streaming: vec![QqStreamingStrategy::FinalOnly],
            configured_intents: 1,
            shard: [0, 1],
            required_intents: vec!["group_and_c2c_event".into()],
            required_permissions: vec![QqPermissionRequirement::ReadC2cMessages],
        },
        1,
        [0, 1],
        true,
        true,
        Some(1),
        None,
    ));
    let api: Arc<dyn QqBotManagementApi> = Arc::new(QqBotManagementService::local(local));
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
    let (mut host, dirs) = build_console_host(
        &config,
        &secrets,
        Arc::new(FixtureControlHandler::default()),
        "local-dev",
        None,
        None,
        &WebConsolePaths::default(),
        None,
        Some(api),
    )
    .unwrap();
    host.start().await.unwrap();
    let addr = host.listen_addr().unwrap().to_string();
    let options = http_get_body(&addr, "/console-options.json").await;
    assert!(options.contains("\"includeQq\":true"));
    let qq_js = http_get_body(&addr, "/qq-bot/index.js").await;
    assert!(qq_js.contains("mountQqBotPanel"));
    let snap = ws_rpc_params(
        &addr,
        "qq-bot",
        "snapshot",
        json!({ "capabilities": ["bot.read", "bot.secret.status"] }),
    )
    .await
    .unwrap();
    assert_eq!(snap["accounts"][0]["account_id"], "main");
    assert_eq!(snap["accounts"][0]["credential_status"], "configured");
    assert!(!dirs.qq_assets.as_os_str().is_empty());
    host.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

async fn http_get_body(addr: &str, path: &str) -> String {
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();
    let text = String::from_utf8_lossy(&buf);
    let (_, body) = text
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("HTTP response missing body separator: {text}"));
    assert!(
        text.starts_with("HTTP/1.1 200") || text.starts_with("HTTP/1.0 200"),
        "expected 200 for {path}, got: {}",
        text.lines().next().unwrap_or("")
    );
    body.to_string()
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
    ws.send(Message::Binary(
        WireMessage::Hello {
            protocol_version: WEB_PROTOCOL_VERSION.into(),
            capabilities: vec!["runtime.read".into(), "*".into()],
            auth_token: Some("local-dev".into()),
        }
        .encode()
        .unwrap()
        .into(),
    ))
    .await
    .unwrap();
    let Message::Binary(ack) = ws.next().await.unwrap().unwrap() else {
        panic!("ack");
    };
    assert!(matches!(
        WireMessage::decode(ack.as_ref()).unwrap(),
        WireMessage::HelloAck { .. }
    ));
    let id = Uuid::new_v4();
    let mut params = extra;
    if let Some(obj) = params.as_object_mut() {
        obj.entry("capabilities")
            .or_insert(json!(["runtime.read", "runtime.write", "*"]));
    }
    ws.send(Message::Binary(
        WireMessage::Rpc(RpcRequest {
            id,
            namespace: namespace.into(),
            method: method.into(),
            params,
        })
        .encode()
        .unwrap()
        .into(),
    ))
    .await
    .unwrap();
    let Message::Binary(bytes) = ws.next().await.unwrap().unwrap() else {
        panic!("rpc");
    };
    match WireMessage::decode(bytes.as_ref()).unwrap() {
        WireMessage::RpcResult(result) => match result.error {
            Some(error) => Err(error.message),
            None => Ok(result.result.unwrap_or(serde_json::Value::Null)),
        },
        other => panic!("unexpected {other:?}"),
    }
}
