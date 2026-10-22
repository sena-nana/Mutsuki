//! Embedded console assembly smoke (control + overview over fixture control plane).
// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(clippy::too_many_lines)]

use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use mutsuki_bot_sandbox::{SandboxApi, SandboxService};
use mutsuki_bot_state_db::BotStateDbRepository;
use mutsuki_bot_web_host_integration::{
    BotAgentConsoleServices, SecretKeyResolver, SecretMonitor, WebConsoleConfig, WebConsolePaths,
    WebConsoleSecrets, build_console_host, build_console_host_with_agent, demo_config_service,
    empty_config_service,
};
use mutsuki_web_extension_control::FixtureControlHandler;
use mutsuki_web_host::WebHost;
use mutsuki_web_protocol::{RpcRequest, WEB_PROTOCOL_VERSION, WireMessage};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

#[tokio::test]
async fn embedded_console_document_resolves_through_its_import_map() {
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

    let document = ConsoleDocument::fetch(&addr).await;
    document.assert_complete(&addr).await;
    assert!(
        document.csp.contains("img-src 'self' data: blob:") && !document.csp.contains("qlogo"),
        "console document CSP must stay at baseline when QQ/sandbox WebExtensions are not selected"
    );
    let (headers, _) = http_get_parts(&addr, "/index.html").await;
    let header_csp = headers.to_ascii_lowercase();
    assert!(
        header_csp.contains("img-src 'self' data: blob:") && !header_csp.contains("qlogo"),
        "Host CSP header must stay at baseline when QQ/sandbox WebExtensions are not selected"
    );

    let activities = document.options["activities"].as_array().unwrap();
    let activity_ids = activities
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(activity_ids, ["home", "system", "settings"]);
    assert!(
        activities
            .iter()
            .any(|item| item["id"] == "settings" && item["position"] == "bottom")
    );
    assert_eq!(
        document.extension_ids(),
        ["overview", "control", "database"]
    );

    // Agent trajectory modules stay importable from the document root and the control page.
    for path in ["/trajectory-model.js", "/trajectory-view.js"] {
        assert!(!http_get_body(&addr, path).await.is_empty());
    }

    // Fonts are host-served next to the document.
    for weight in ["400", "500", "600", "700"] {
        let path = format!("/fonts/noto-sans-sc-chinese-simplified-{weight}-normal.woff2");
        let (headers, body) = http_get_bytes(&addr, &path).await;
        assert!(headers.to_ascii_lowercase().contains("woff2"));
        assert!(body.starts_with(b"wOF2"));
    }

    host.stop().await.unwrap();
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

    // No repository is bound in this assembly, so the database page has nothing to show. The RPC
    // says so by returning null, and reading rows fails rather than inventing an empty table.
    let database = ws_rpc(&addr, "database", "snapshot").await.unwrap();
    assert!(database.is_null());
    assert!(
        ws_rpc_params(
            &addr,
            "database",
            "rows",
            json!({"table": "bot_management_meta"})
        )
        .await
        .is_err()
    );

    let logs = ws_rpc_params(&addr, "control", "log_tail", json!({"lines": 5}))
        .await
        .unwrap();
    assert!(logs["entries"].is_array());

    host.stop().await.unwrap();
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
    let document = ConsoleDocument::fetch(&addr).await;
    document.assert_complete(&addr).await;
    assert_eq!(
        document.extension_ids().last().map(String::as_str),
        Some("config")
    );
    let parsed = &document.options;
    assert!(
        parsed["activities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| { item["id"] == "plugins" && item["position"] == "top" })
    );
    assert!(
        !parsed["activities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == "config")
    );
    assert!(
        !parsed["activities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == "settings")
    );
    host.stop().await.unwrap();
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
}

#[tokio::test]
async fn embedded_console_starts_upgrade_extension_when_release_set_configured() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../../crates/mutsuki-module-catalog")
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
}

#[tokio::test]
async fn embedded_console_mounts_qq_management_extension() {
    use mutsuki_bot_protocol::{
        BotConversationKind, BotRateLimitPolicy, BotSegmentKind, BotStreamingStrategy,
        BotUploadConstraints,
    };
    use mutsuki_bot_qq_protocol::{QqBotCapabilityMatrix, QqPermissionRequirement};
    use mutsuki_bot_web_extension_qq::{
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
            configured_intents: 1,
            shard: [0, 1],
            required_intents: vec!["group_and_c2c_event".into()],
            required_permissions: vec![QqPermissionRequirement::ReadC2cMessages],
            capabilities: mutsuki_bot_protocol::BotAdapterCapabilities {
                conversation_kinds: vec![BotConversationKind::Private],
                outbound_conversation_kinds: vec![BotConversationKind::Private],
                active_message_kinds: vec![BotConversationKind::Private],
                inbound_segments: vec![BotSegmentKind::Text],
                outbound_segments: vec![BotSegmentKind::Text],
                inbound_media: Vec::new(),
                outbound_media: Vec::new(),
                active_message: true,
                message_edit: false,
                message_recall: true,
                reply: true,
                quote: true,
                mention: true,
                upload: BotUploadConstraints::default(),
                rate_limit: BotRateLimitPolicy::default(),
                streaming: vec![BotStreamingStrategy::FinalOnly],
            },
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
    let document = ConsoleDocument::fetch(&addr).await;
    document.assert_complete(&addr).await;
    assert!(document.extension_ids().contains(&"qq-bot".to_string()));
    let snap = ws_rpc_params(&addr, "qq-bot", "snapshot", json!({}))
        .await
        .unwrap();
    assert_eq!(snap["accounts"][0]["account_id"], "main");
    assert_eq!(snap["accounts"][0]["credential_status"], "configured");
    assert!(dirs.page_assets("qq-bot").is_some());
    host.stop().await.unwrap();
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
    let (mut host, dirs) = build_console_host_with_agent(
        &config,
        &secrets,
        Arc::new(FixtureControlHandler::default()),
        "local-dev",
        None,
        None,
        &WebConsolePaths::default(),
        None,
        None,
        Some(Arc::new(SandboxService::new()) as Arc<dyn SandboxApi>),
        None,
        BotAgentConsoleServices::default(),
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
    let document = ConsoleDocument::fetch(&addr).await;
    document.assert_complete(&addr).await;
    assert!(
        !document.options["activities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == "sandbox")
    );
    assert!(document.extension_ids().contains(&"sandbox".to_string()));
    let snap = ws_rpc_params(&addr, "sandbox", "snapshot", json!({}))
        .await
        .unwrap();
    assert_eq!(snap["mode"], "simulate");
    assert!(!snap["conversations"].as_array().unwrap().is_empty());
    assert!(dirs.page_assets("sandbox").is_some());
    host.stop().await.unwrap();
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
    let document = ConsoleDocument::fetch(&addr).await;
    // Crawling the flow module also proves its relative editor/context-menu imports are staged.
    document.assert_complete(&addr).await;
    assert!(
        document
            .extension_ids()
            .contains(&"bot-flow-editor".to_string())
    );
    assert!(dirs.page_assets("bot-flow-editor").is_some());
    host.stop().await.unwrap();
}

#[test]
fn embedded_console_rejects_explicit_bilibili_extension_without_service() {
    let error = build_console_host(
        &WebConsoleConfig {
            enabled: true,
            listen: "127.0.0.1:0".into(),
            extensions: vec!["bilibili".into()],
            ..Default::default()
        },
        &WebConsoleSecrets {
            auth_token: "local-dev".into(),
        },
        Arc::new(FixtureControlHandler::default()),
        "local-dev",
        None,
        None,
        &WebConsolePaths::default(),
        None,
        None,
    )
    .err()
    .expect("missing management service");
    assert!(
        error
            .to_string()
            .contains("bilibili WebExtension requires its management service")
    );
}

#[test]
fn embedded_console_rejects_explicit_sandbox_extension_without_service() {
    let error = build_console_host(
        &WebConsoleConfig {
            enabled: true,
            listen: "127.0.0.1:0".into(),
            extensions: vec!["sandbox".into()],
            ..Default::default()
        },
        &WebConsoleSecrets {
            auth_token: "local-dev".into(),
        },
        Arc::new(FixtureControlHandler::default()),
        "local-dev",
        None,
        None,
        &WebConsolePaths::default(),
        None,
        None,
    )
    .err()
    .expect("missing management service");
    assert!(
        error
            .to_string()
            .contains("sandbox WebExtension requires its management service")
    );
}

/// The console document as a browser sees it: CSP, import map, stylesheet and module list.
struct ConsoleDocument {
    csp: String,
    import_map_body: String,
    imports: serde_json::Map<String, serde_json::Value>,
    stylesheets: Vec<String>,
    module_scripts: Vec<String>,
    options: serde_json::Value,
}

impl ConsoleDocument {
    async fn fetch(addr: &str) -> Self {
        let html = http_get_body(addr, "/").await;
        let csp = between(
            &html,
            "content=\"",
            "\"",
            "http-equiv=\"Content-Security-Policy\"",
        );
        let import_map_body = between(&html, ">", "</script>", "<script type=\"importmap\"");
        let map: serde_json::Value = serde_json::from_str(&import_map_body).unwrap();
        let imports = map["imports"].as_object().unwrap().clone();
        let stylesheets = attribute_values(&html, "<link rel=\"stylesheet\" href=\"");
        let module_scripts = attribute_values(&html, "<script type=\"module\" src=\"");
        let options =
            serde_json::from_str(&http_get_body(addr, "/console-options.json").await).unwrap();
        Self {
            csp,
            import_map_body,
            imports,
            stylesheets,
            module_scripts,
            options,
        }
    }

    fn extension_ids(&self) -> Vec<String> {
        self.options["extensions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["id"].as_str().unwrap().to_string())
            .collect()
    }

    /// Every module the console can import resolves: bare specifiers through the import map,
    /// relative ones to served files. Every extension's manifest hashes match what is served,
    /// and the CSP admits exactly the inline import map.
    async fn assert_complete(&self, addr: &str) {
        assert!(
            self.csp.contains(&mutsuki_web_shared_runtime::csp_hash(
                self.import_map_body.as_bytes()
            )),
            "document CSP must carry the hash of the inline import map"
        );
        let script_src = self
            .csp
            .split(';')
            .map(str::trim)
            .find(|directive| directive.starts_with("script-src"))
            .unwrap();
        assert!(!script_src.contains("'unsafe-inline'"), "{script_src}");
        assert!(!self.stylesheets.is_empty());
        for href in &self.stylesheets {
            let (headers, _) = http_get_parts(addr, &absolute("/", href)).await;
            assert!(headers.to_ascii_lowercase().contains("text/css"), "{href}");
        }

        let mut pending = self
            .module_scripts
            .iter()
            .map(|src| absolute("/", src))
            .collect::<Vec<_>>();
        for target in self.imports.values() {
            pending.push(absolute("/", target.as_str().unwrap()));
        }
        for extension in self.options["extensions"].as_array().unwrap() {
            pending.push(absolute("/", extension["url"].as_str().unwrap()));
            for style in extension["styles"].as_array().unwrap() {
                let style = absolute("/", style.as_str().unwrap());
                assert!(!http_get_body(addr, &style).await.is_empty());
            }
            self.assert_manifest_matches(addr, extension).await;
        }
        let mut seen = std::collections::BTreeSet::new();
        while let Some(path) = pending.pop() {
            let path = path.split('?').next().unwrap().to_string();
            if !seen.insert(path.clone()) {
                continue;
            }
            let source = http_get_body(addr, &path).await;
            for specifier in import_specifiers(&source) {
                if specifier.starts_with("./")
                    || specifier.starts_with("../")
                    || specifier.starts_with('/')
                {
                    pending.push(absolute(&path, &specifier));
                } else {
                    assert!(
                        self.imports.contains_key(&specifier),
                        "{path} imports bare `{specifier}`, which the import map does not resolve"
                    );
                }
            }
        }
    }

    async fn assert_manifest_matches(&self, addr: &str, extension: &serde_json::Value) {
        let url = absolute("/", extension["url"].as_str().unwrap());
        let (module, stamp) = url.split_once("?v=").expect("extension URL is versioned");
        let base = &module[..=module.rfind('/').unwrap()];
        let manifest: mutsuki_web_protocol::ExtensionManifest =
            serde_json::from_str(&http_get_body(addr, &format!("{base}manifest.json")).await)
                .unwrap();
        assert_eq!(format!("{base}{}", manifest.entry), module);
        for asset in &manifest.assets {
            let (_, bytes) = http_get_bytes(addr, &format!("{base}{}", asset.path)).await;
            assert_eq!(
                mutsuki_web_extension::content_hash(&bytes),
                asset.content_hash,
                "{base}{} differs from its manifest",
                asset.path
            );
            if asset.path == manifest.entry {
                assert!(asset.content_hash["sha256:".len()..].starts_with(stamp));
            }
        }
    }
}

fn between(text: &str, open: &str, close: &str, anchor: &str) -> String {
    let start = text
        .find(anchor)
        .unwrap_or_else(|| panic!("missing {anchor}"))
        + anchor.len();
    let rest = &text[start..];
    let rest = &rest[rest.find(open).unwrap() + open.len()..];
    rest[..rest.find(close).unwrap()].to_string()
}

fn attribute_values(text: &str, prefix: &str) -> Vec<String> {
    text.match_indices(prefix)
        .map(|(index, _)| {
            let rest = &text[index + prefix.len()..];
            rest[..rest.find('"').unwrap()].to_string()
        })
        .collect()
}

/// Resolves `reference` against the served path `base` the way a browser would.
fn absolute(base: &str, reference: &str) -> String {
    if let Some(path) = reference.strip_prefix('/') {
        return format!("/{path}");
    }
    let mut parts = base.split('/').collect::<Vec<_>>();
    parts.pop();
    for part in reference.split('/') {
        match part {
            "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    let joined = parts.join("/");
    if joined.starts_with('/') {
        joined
    } else {
        format!("/{joined}")
    }
}

/// Literal static and dynamic import specifiers of an ES module.
fn import_specifiers(source: &str) -> Vec<String> {
    let mut specifiers = Vec::new();
    for keyword in ["from", "import", "import("] {
        for (index, _) in source.match_indices(keyword) {
            let before = source[..index].chars().next_back();
            if before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '$' || c == '.') {
                continue;
            }
            let rest = source[index + keyword.len()..].trim_start();
            let Some(quote) = rest.chars().next().filter(|c| *c == '"' || *c == '\'') else {
                continue;
            };
            let body = &rest[1..];
            if let Some(end) = body.find(quote) {
                let specifier = &body[..end];
                if !specifier.is_empty() && !specifier.contains(char::is_whitespace) {
                    specifiers.push(specifier.to_string());
                }
            }
        }
    }
    specifiers
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
