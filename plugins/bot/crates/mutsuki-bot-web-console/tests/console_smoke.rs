//! Embedded console assembly smoke (control + overview over fixture control plane).

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use mutsuki_bot_state_db::BotStateDbRepository;
use mutsuki_bot_web_console::{
    BotAgentConsoleServices, SecretKeyResolver, SecretMonitor, WebConsoleConfig, WebConsolePaths,
    WebConsoleSecrets, build_console_host, build_console_host_with_agent, demo_config_service,
    empty_config_service,
};
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
    assert!(css.contains(".mutsuki-console .trajectory"));
    assert!(css.contains(".mutsuki-console .trajectory-row"));
    assert!(css.contains(".lilia-node-editor"));
    assert!(css.contains(".lilia-node-editor__viewport"));
    assert!(css.contains(".lilia-node-editor__wire"));
    assert!(css.contains(".console-page-header__actions"));
    assert!(css.contains(".mutsuki-console .overview-cards"));
    assert!(css.contains(".is-context-hidden"));
}

#[tokio::test]
async fn embedded_console_serves_workspace_css_and_shell_markup() {
    let config = WebConsoleConfig {
        enabled: true,
        listen: "127.0.0.1:0".into(),
        auth_token_key: None,
        extensions: Vec::new(),
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
    assert!(css.contains(".mutsuki-console--activity-shell"));
    assert!(css.contains(".console-activity__item.is-active"));
    assert!(!css.contains("@import"));

    let shell_js = http_get_body(&addr, "/shared/web-shell.js").await;
    assert!(shell_js.contains("console-activity"));
    assert!(shell_js.contains("console-context"));
    assert!(shell_js.contains("console-page-header__actions"));
    assert!(shell_js.contains("is-context-hidden"));
    assert!(shell_js.contains("sandbox:"));
    let options: serde_json::Value =
        serde_json::from_str(&http_get_body(&addr, "/console-options.json").await).unwrap();
    let activities = options["activities"].as_array().unwrap();
    assert!(!activities.iter().any(|item| item["id"] == "config"));
    assert!(
        activities
            .iter()
            .any(|item| item["id"] == "settings" && item["position"] == "bottom")
    );
    let extensions = options["extensions"].as_array().unwrap();
    for id in ["overview", "control", "database"] {
        let url = extensions
            .iter()
            .find(|item| item["id"] == id)
            .and_then(|item| item["url"].as_str())
            .expect("enabled extension must have a module URL");
        let path = url.trim_start_matches('.');
        assert!(!http_get_body(&addr, path).await.is_empty());
    }
    let overview_js = http_get_body(&addr, "/extensions/overview/index.js").await;
    assert!(overview_js.contains("overview-dashboard"));
    assert!(overview_js.contains("metric-grid"));
    assert!(overview_js.contains("overview.cards"));
    assert!(overview_js.contains("overview-cards"));
    assert!(overview_js.contains(r#"aria-label", "刷新""#));
    assert!(!overview_js.contains("mountQqAccountCards"));
    assert!(!overview_js.contains("qq-bot/index.js"));
    let control_js = http_get_body(&addr, "/extensions/control/index.js").await;
    assert!(control_js.contains("mountTrajectoryView"));
    assert!(control_js.contains("./trajectory-view.js"));
    assert!(!control_js.contains("id=\"task-event-seq\""));
    assert!(css.contains(".mutsuki-console .trajectory"));
    assert!(css.contains(".mutsuki-console .trajectory-row"));
    assert!(css.contains(".mutsuki-console .overview-dashboard"));
    assert!(css.contains(".mutsuki-console .metric-grid"));
    assert!(css.contains(".mutsuki-console .overview-cards"));
    assert!(css.contains(".console-page-header__actions"));
    assert!(
        http_get_body(&addr, "/trajectory-model.js")
            .await
            .contains("projectAgentEvents")
    );
    assert!(
        http_get_body(&addr, "/trajectory-view.js")
            .await
            .contains("mountTrajectoryView")
    );

    let html = http_get_body(&addr, "/").await;
    assert!(html.contains("mutsuki-ui.css?v="));
    assert!(html.contains("console-bootstrap.js?v="));
    let (headers, shell) = http_get_parts(&addr, "/index.html").await;
    let header_csp = headers.to_ascii_lowercase();
    assert!(
        header_csp.contains("img-src 'self' data: blob:") && !header_csp.contains("qlogo"),
        "Host CSP header must stay at baseline when QQ/sandbox WebExtensions are not selected"
    );
    assert!(
        shell.contains("img-src 'self' data: blob:") && !shell.contains("qlogo"),
        "console document CSP must stay at baseline when QQ/sandbox WebExtensions are not selected"
    );

    let bootstrap = http_get_body(&addr, "/console-bootstrap.js").await;
    assert!(bootstrap.contains("createWebShellRuntime"));
    assert!(bootstrap.contains("createWebUiThemeController"));
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

    for weight in ["400", "500", "600", "700"] {
        let path = format!("/fonts/noto-sans-sc-chinese-simplified-{weight}-normal.woff2");
        let (headers, body) = http_get_bytes(&addr, &path).await;
        assert!(headers.to_ascii_lowercase().contains("woff2"));
        assert!(body.starts_with(b"wOF2"));
    }

    host.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test]
async fn embedded_console_reads_overview_and_control() {
    let config = WebConsoleConfig {
        enabled: true,
        listen: "127.0.0.1:0".into(),
        auth_token_key: None,
        extensions: Vec::new(),
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

    let database = ws_rpc(&addr, "database", "snapshot").await.unwrap();
    assert!(database.is_null());
    let database_js = http_get_body(&addr, "/extensions/database/index.js").await;
    assert!(database_js.contains("在左侧选择要访问的数据表"));

    let logs = ws_rpc_params(&addr, "control", "log_tail", json!({"lines": 5}))
        .await
        .unwrap();
    assert!(logs["entries"].is_array());

    host.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test]
async fn embedded_console_reads_live_bot_state_database() {
    let root = tempfile::tempdir().unwrap();
    let repository =
        Arc::new(BotStateDbRepository::open(root.path().join("state.sqlite3")).unwrap());
    let config = WebConsoleConfig {
        enabled: true,
        listen: "127.0.0.1:0".into(),
        auth_token_key: None,
        extensions: Vec::new(),
        ..Default::default()
    };
    let secrets = WebConsoleSecrets {
        auth_token: "local-dev".into(),
    };
    let (mut host, _dirs) = build_console_host_with_agent(
        &config,
        &secrets,
        Arc::new(FixtureControlHandler::default()),
        "local-dev",
        None,
        None,
        &WebConsolePaths::default(),
        None,
        None,
        None,
        Some(repository),
        BotAgentConsoleServices::default(),
    )
    .unwrap();
    host.start().await.unwrap();
    let addr = host.listen_addr().unwrap().to_string();
    let snapshot = ws_rpc(&addr, "database", "snapshot").await.unwrap();
    assert!(
        snapshot["tables"]
            .as_array()
            .unwrap()
            .iter()
            .any(|table| table["name"] == "bot_management_meta")
    );
    let page = ws_rpc_params(
        &addr,
        "database",
        "rows",
        json!({ "table": "bot_management_meta" }),
    )
    .await
    .unwrap();
    assert_eq!(page["rows"].as_array().unwrap().len(), 1);
    host.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test]
async fn embedded_console_with_config_shell() {
    let config = WebConsoleConfig {
        enabled: true,
        listen: "127.0.0.1:0".into(),
        auth_token_key: None,
        extensions: vec!["config".into()],
        config_provider_ids: vec!["product".into()],
        primary_config_provider_id: Some("product".into()),
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
    assert!(bootstrap.contains("mountWebShell"));
    let options = http_get_body(&addr, "/console-options.json").await;
    let parsed: serde_json::Value = serde_json::from_str(&options).unwrap();
    assert!(
        parsed["activities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| { item["id"] == "config" && item["position"] == "top" })
    );
    assert!(
        !parsed["activities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == "settings")
    );
    let config_path = versioned_module_path(&options, "./extensions/config/index.js");
    let config_js = http_get_body(&addr, &format!("/{config_path}")).await;
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
        extensions: vec!["config".into()],
        config_provider_ids: vec!["product".into()],
        primary_config_provider_id: Some("product".into()),
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
    let navigation = ws_rpc(
        &host.listen_addr().unwrap().to_string(),
        "config",
        "navigation.list",
    )
    .await
    .unwrap();
    assert_eq!(navigation[0]["items"][0]["provider_id"], "product");
    assert!(navigation[0]["items"][0]["label"].is_null());
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
        extensions: vec!["upgrade".into()],
        config_provider_ids: Vec::new(),
        primary_config_provider_id: None,
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
        extensions: Vec::new(),
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
        LocalQqManagementProvider, QqAccountViewInput, QqBotManagementApi, QqBotManagementService,
        account_view_from_config,
    };

    let local = LocalQqManagementProvider::new();
    local.upsert_account(account_view_from_config(QqAccountViewInput {
        account_id: "main".into(),
        app_id: "app".into(),
        credential_reference: "QQBOT_CLIENT_SECRET".into(),
        credential_present: true,
        capability: QqBotCapabilityMatrix {
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
        intents: 1,
        shard: [0, 1],
        connected: true,
        identified: true,
        last_heartbeat_unix_ms: Some(1),
        last_error: None,
        last_error_code: None,
        reconnect_count: 0,
        self_user: None,
    }));
    let api: Arc<dyn QqBotManagementApi> = Arc::new(QqBotManagementService::local(local));
    let config = WebConsoleConfig {
        enabled: true,
        listen: "127.0.0.1:0".into(),
        auth_token_key: None,
        extensions: vec!["qq".into()],
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
    let (headers, shell) = http_get_parts(&addr, "/index.html").await;
    let header_csp = headers.to_ascii_lowercase();
    assert!(
        header_csp.contains("https://*.qlogo.cn") && header_csp.contains("https://*.nt.qq.com.cn"),
        "qq WebExtension must declare QQ image hosts in the Host CSP header"
    );
    assert!(
        shell.contains("https://*.qlogo.cn") && shell.contains("https://*.nt.qq.com.cn"),
        "qq WebExtension must declare QQ image hosts in the document CSP"
    );
    let options = http_get_body(&addr, "/console-options.json").await;
    let qq_path = versioned_module_path(&options, "./extensions/qq-bot/index.js");
    let qq_js = http_get_body(&addr, &format!("/{qq_path}")).await;
    assert!(qq_js.contains("mountQqAccountCards"));
    assert!(qq_js.contains("overview.cards"));
    assert!(qq_js.contains("config.editor"));
    assert!(qq_js.contains("请到配置里填写账号"));
    assert!(qq_js.contains("self_user"));
    assert!(qq_js.contains("qq-account-avatar"));
    assert!(qq_js.contains("在线时长"));
    assert!(!qq_js.contains("qq-bot.page"));
    assert!(!qq_js.contains("主动投递"));
    assert!(!qq_js.contains("搜索账号"));
    assert!(!qq_js.contains("保存登录配置"));
    let snap = ws_rpc_params(&addr, "qq-bot", "snapshot", json!({}))
        .await
        .unwrap();
    assert_eq!(snap["accounts"][0]["account_id"], "main");
    assert_eq!(snap["accounts"][0]["credential_status"], "configured");
    assert!(!dirs.qq_assets.as_os_str().is_empty());
    host.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test]
async fn embedded_console_serves_sandbox_panel() {
    let config = WebConsoleConfig {
        enabled: true,
        listen: "127.0.0.1:0".into(),
        auth_token_key: None,
        extensions: vec!["sandbox".into()],
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
        None,
    )
    .unwrap();
    host.start().await.unwrap();
    let addr = host.listen_addr().unwrap().to_string();
    let (headers, shell) = http_get_parts(&addr, "/index.html").await;
    assert!(
        headers.to_ascii_lowercase().contains("https://*.qlogo.cn"),
        "sandbox WebExtension must declare QQ image hosts in the Host CSP header"
    );
    assert!(
        shell.contains("https://*.qlogo.cn"),
        "sandbox WebExtension must declare QQ image hosts in the document CSP"
    );
    let options = http_get_body(&addr, "/console-options.json").await;
    let parsed: serde_json::Value = serde_json::from_str(&options).unwrap();
    assert!(
        parsed["activities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == "sandbox")
    );
    let path = versioned_module_path(&options, "./extensions/sandbox/index.js");
    let js = http_get_body(&addr, &format!("/{path}")).await;
    assert!(js.contains("mountSandboxPanel"));
    assert!(js.contains("activityId: \"sandbox\""));
    assert!(!js.contains("ctx.activities.register"));
    assert!(js.contains("添加用户"));
    assert!(js.contains("真实数据"));
    assert!(!js.contains("inject_into_flow"));
    let snap = ws_rpc_params(&addr, "sandbox", "snapshot", json!({}))
        .await
        .unwrap();
    assert_eq!(snap["mode"], "simulate");
    assert!(!snap["conversations"].as_array().unwrap().is_empty());
    assert!(!dirs.sandbox_assets.as_os_str().is_empty());
    host.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test]
async fn embedded_console_serves_lilia_flow_node_editor() {
    use mutsuki_bot_flow::{BotFlowConfigProvider, BotFlowRegistry, BotNodeCatalog};
    use mutsuki_config_service::{ConfigProviderRegistry, ConfigService, InMemoryConfigRepository};

    let flow = Arc::new(BotFlowRegistry::new(BotNodeCatalog::default()));
    let providers = Arc::new(ConfigProviderRegistry::default());
    providers
        .register(Arc::new(BotFlowConfigProvider::new(flow.clone())))
        .unwrap();
    let service = Arc::new(
        ConfigService::new(providers, Arc::new(InMemoryConfigRepository::default())).unwrap(),
    );
    let config = WebConsoleConfig {
        enabled: true,
        listen: "127.0.0.1:0".into(),
        auth_token_key: None,
        extensions: vec!["bot-flow-editor".into()],
        ..Default::default()
    };
    let secrets = WebConsoleSecrets {
        auth_token: "local-dev".into(),
    };
    let (mut host, dirs) = build_console_host_with_agent(
        &config,
        &secrets,
        Arc::new(FixtureControlHandler::default()),
        "local-dev",
        Some(service),
        None,
        &WebConsolePaths::default(),
        None,
        None,
        None,
        None,
        BotAgentConsoleServices {
            flow: Some(flow),
            ..BotAgentConsoleServices::default()
        },
    )
    .unwrap();
    host.start().await.unwrap();
    let addr = host.listen_addr().unwrap().to_string();
    let options = http_get_body(&addr, "/console-options.json").await;
    let flow_path = versioned_module_path(&options, "./extensions/bot-flow/index.js");
    let flow_js = http_get_body(&addr, &format!("/{flow_path}")).await;
    assert!(flow_js.contains("lilia-node-editor"));
    assert!(flow_js.contains("mountLiliaNodeEditor"));
    let editor_js = http_get_body(&addr, "/extensions/bot-flow/lilia-node-editor.js").await;
    assert!(editor_js.contains("export function mountLiliaNodeEditor"));
    let css = http_get_body(&addr, "/mutsuki-ui.css").await;
    assert!(css.contains(".lilia-node-editor__viewport"));
    assert!(css.contains(".lilia-node-editor__wire"));
    assert!(!dirs.bot_flow_assets.as_os_str().is_empty());
    host.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

fn versioned_module_path(options: &str, module_path: &str) -> String {
    let start = options
        .find(module_path)
        .unwrap_or_else(|| panic!("console options do not reference {module_path}"));
    let path = &options[start + 2..];
    let end = path
        .find('"')
        .unwrap_or_else(|| panic!("unterminated module specifier for {module_path}"));
    let path = &path[..end];
    assert!(path.contains("?v="), "module path is not versioned: {path}");
    path.to_string()
}

async fn http_get_body(addr: &str, path: &str) -> String {
    String::from_utf8_lossy(&http_get_bytes(addr, path).await.1).into_owned()
}

async fn http_get_parts(addr: &str, path: &str) -> (String, String) {
    let (headers, body) = http_get_bytes(addr, path).await;
    (headers, String::from_utf8_lossy(&body).into_owned())
}

async fn http_get_bytes(addr: &str, path: &str) -> (String, Vec<u8>) {
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();
    let split = buf
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap_or_else(|| panic!("HTTP response missing body separator for {path}"));
    let headers = String::from_utf8_lossy(&buf[..split]).into_owned();
    assert!(
        headers.starts_with("HTTP/1.1 200") || headers.starts_with("HTTP/1.0 200"),
        "expected 200 for {path}, got: {}",
        headers.lines().next().unwrap_or("")
    );
    (headers, buf[split + 4..].to_vec())
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
            capabilities: Vec::new(),
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
    let params = extra;
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
