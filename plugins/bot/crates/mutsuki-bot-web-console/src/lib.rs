//! Product-level Bot Web Console assembly (Embedded WebHost + extensions).
//!
//! Console = WebApplication shell + control/overview/(optional) config WebExtensions.
//! Does not embed business pages into WebHost Recovery.

mod config_demo;
mod secret_status;
mod watch_bridge;

use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use config_demo::demo_config_service;
pub use secret_status::{SecretKeyResolver, SecretMonitor, SecretStatusWebExtension};
pub use watch_bridge::{
    ControlChangeBridge, ManagementChangeBridge, attach_control_changed_bridge,
    attach_management_changed_bridges, attach_revision_changed_bridge,
};

use mutsuki_bot_flow::BotFlowRegistry;
use mutsuki_bot_management::BilibiliManagementApi;
use mutsuki_config_service::{ConfigProviderRegistry, ConfigService, InMemoryConfigRepository};
use mutsuki_plugin_bot_agent_web::{
    AgentConnectionManagementResolver, BotAgentWebExtension, LocalAgentManagementResolver,
    materialize_frontend_assets as materialize_bot_agent_assets, materialize_trajectory_assets,
};
use mutsuki_plugin_bot_bilibili_web::{
    BilibiliWebExtension, materialize_frontend_assets as materialize_bilibili_assets,
};
use mutsuki_plugin_bot_control_web::{
    ControlRpcCaller, ControlWebExtension,
    materialize_frontend_assets as materialize_control_assets,
};
use mutsuki_plugin_bot_flow_web::{
    BotFlowEditorWebExtension, materialize_frontend_assets as materialize_bot_flow_assets,
};
use mutsuki_plugin_bot_overview_web::{
    OverviewWebExtension, materialize_frontend_assets as materialize_overview_assets,
};
use mutsuki_plugin_bot_qq_web::{
    QqBotManagementApi, QqBotWebExtension, materialize_frontend_assets as materialize_qq_assets,
};
use mutsuki_plugin_bot_upgrade_web::{
    UpgradeWebExtension, materialize_frontend_assets as materialize_upgrade_assets,
};
use mutsuki_plugin_config_web::{
    ConfigNavigationGroup, ConfigNavigationItem, ConfigWebExtension,
    materialize_frontend_assets as materialize_config_assets,
};
use mutsuki_service_control::ControlHandler;
use mutsuki_web_extension::content_hash;
use mutsuki_web_host::{
    MinimalWebApplication, MutsukiWebHost, MutsukiWebHostBuilder, WebHostResult,
};
use mutsuki_web_protocol::{DeploymentMode, WebApplicationDescriptor, WebShellAssets};
use serde_json::json;

pub const CONSOLE_APPLICATION_ID: &str = "mutsuki.bot.console";

/// Console enablement parsed from product config (`[web.console]`).
#[derive(Clone, Debug, Default, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WebConsoleConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_listen")]
    pub listen: String,
    /// Host secret key reference for the Web auth token (no literal secrets in product config).
    pub auth_token_key: Option<String>,
    /// Explicit WebExtension selection. Runtime plugins never imply pages.
    #[serde(default)]
    pub extensions: Vec<String>,
    /// Product-selected Config providers. Empty preserves the generic Config Web default.
    #[serde(default)]
    pub config_provider_ids: Vec<String>,
    /// Product-owned primary provider shown before plugin configuration.
    pub primary_config_provider_id: Option<String>,
    /// Relative path to active release set manifest (enables auto-upgrade page).
    pub release_set: Option<String>,
}

fn default_listen() -> String {
    "127.0.0.1:0".into()
}

impl WebConsoleConfig {
    pub fn disabled() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn has_extension(&self, id: &str) -> bool {
        self.extensions.iter().any(|candidate| candidate == id)
    }
}

/// Resolved console auth token from Host secret store.
pub struct WebConsoleSecrets {
    pub auth_token: String,
}

/// Empty ConfigService for products that enable `include_config` before registering providers.
pub fn empty_config_service() -> Arc<ConfigService> {
    let registry = Arc::new(ConfigProviderRegistry::default());
    Arc::new(
        ConfigService::new(registry, Arc::new(InMemoryConfigRepository::default()))
            .expect("memory ConfigRepository recovers"),
    )
}

/// Resolved filesystem paths for optional console features.
#[derive(Default)]
pub struct WebConsolePaths {
    pub release_set: Option<PathBuf>,
}

impl WebConsolePaths {
    pub fn resolve(product_root: &Path, config: &WebConsoleConfig) -> Self {
        Self {
            release_set: config
                .release_set
                .as_deref()
                .map(|relative| product_root.join(relative)),
        }
    }
}

/// Build an embedded WebHost pre-wired with control + overview (+ optional config/upgrade/bilibili/qq) extensions.
pub fn build_console_host(
    config: &WebConsoleConfig,
    secrets: &WebConsoleSecrets,
    control: Arc<dyn ControlHandler>,
    control_token: &str,
    config_service: Option<Arc<ConfigService>>,
    secret_monitor: Option<SecretMonitor>,
    paths: &WebConsolePaths,
    bilibili: Option<Arc<dyn BilibiliManagementApi>>,
    qq: Option<Arc<dyn QqBotManagementApi>>,
) -> WebHostResult<(MutsukiWebHost, ConsoleAssetDirs)> {
    build_console_host_with_agent(
        config,
        secrets,
        control,
        control_token,
        config_service,
        secret_monitor,
        paths,
        bilibili,
        qq,
        BotAgentConsoleServices::default(),
    )
}

/// Owner services that make Agent connection management and Bot flow authoring real.
#[derive(Default)]
pub struct BotAgentConsoleServices {
    pub connections: Option<AgentConnectionManagementResolver>,
    pub sessions: Option<LocalAgentManagementResolver>,
    pub flow: Option<Arc<BotFlowRegistry>>,
}

/// Builds the console and exposes each Agent page only when its owner service is registered.
#[allow(clippy::too_many_arguments)]
pub fn build_console_host_with_agent(
    config: &WebConsoleConfig,
    secrets: &WebConsoleSecrets,
    control: Arc<dyn ControlHandler>,
    control_token: &str,
    config_service: Option<Arc<ConfigService>>,
    secret_monitor: Option<SecretMonitor>,
    paths: &WebConsolePaths,
    bilibili: Option<Arc<dyn BilibiliManagementApi>>,
    qq: Option<Arc<dyn QqBotManagementApi>>,
    bot_agent: BotAgentConsoleServices,
) -> WebHostResult<(MutsukiWebHost, ConsoleAssetDirs)> {
    if !config.enabled {
        return Err(mutsuki_web_host::WebHostError::InvalidConfig(
            "web.console.enabled is false".into(),
        ));
    }

    let asset_dirs = ConsoleAssetDirs::materialize(
        config.has_extension("config"),
        config.has_extension("upgrade"),
        config.has_extension("bilibili"),
        config.has_extension("qq"),
        config.has_extension("agent"),
        config.has_extension("bot-flow-editor"),
    )?;
    let caller = ControlRpcCaller::new(control, control_token);
    let mut builder = base_builder(config, secrets, &asset_dirs);
    builder = builder.extension(
        ControlWebExtension::new(caller.clone()).with_frontend_assets(&asset_dirs.control_assets),
    );
    builder = builder.extension(
        OverviewWebExtension::new(caller.clone()).with_frontend_assets(&asset_dirs.overview_assets),
    );
    if let Some(monitor) = secret_monitor {
        builder = builder.extension(SecretStatusWebExtension::new(monitor));
    }
    if config.has_extension("upgrade") {
        let release_set_path = paths.release_set.as_ref().ok_or_else(|| {
            mutsuki_web_host::WebHostError::InvalidConfig(
                "upgrade WebExtension requires a release set".into(),
            )
        })?;
        builder = builder.extension(
            UpgradeWebExtension::new(release_set_path)
                .map_err(|err| mutsuki_web_host::WebHostError::InvalidConfig(err.to_string()))?
                .with_frontend_assets(&asset_dirs.upgrade_assets),
        );
    }
    if config.has_extension("config") {
        let service = config_service.clone().ok_or_else(|| {
            mutsuki_web_host::WebHostError::InvalidConfig(
                "web.console.include_config requires ConfigService".into(),
            )
        })?;
        let mut extension =
            ConfigWebExtension::new(service).with_frontend_assets(&asset_dirs.config_assets);
        if !config.config_provider_ids.is_empty() {
            extension = extension.with_visible_providers(config.config_provider_ids.clone());
        }
        if let Some(primary) = &config.primary_config_provider_id {
            let plugin_items = config
                .config_provider_ids
                .iter()
                .filter(|provider| *provider != primary)
                .map(|provider| ConfigNavigationItem {
                    provider_id: provider.clone(),
                    label: None,
                })
                .collect();
            extension = extension.with_navigation_groups([
                ConfigNavigationGroup {
                    label: None,
                    items: vec![ConfigNavigationItem {
                        provider_id: primary.clone(),
                        label: Some("Mutsuki".into()),
                    }],
                },
                ConfigNavigationGroup {
                    label: Some("插件".into()),
                    items: plugin_items,
                },
            ]);
        }
        builder = builder.extension(extension);
    }
    if config.has_extension("bilibili") {
        let service = bilibili.ok_or_else(|| {
            mutsuki_web_host::WebHostError::InvalidConfig(
                "bilibili WebExtension requires its management service".into(),
            )
        })?;
        builder = builder.extension(
            BilibiliWebExtension::new(service).with_frontend_assets(&asset_dirs.bilibili_assets),
        );
    }
    if config.has_extension("qq") {
        let api = qq.ok_or_else(|| {
            mutsuki_web_host::WebHostError::InvalidConfig(
                "qq WebExtension requires its management service".into(),
            )
        })?;
        builder = builder
            .extension(QqBotWebExtension::new(api).with_frontend_assets(&asset_dirs.qq_assets));
    }
    if config.has_extension("agent") {
        if bot_agent.connections.is_none() && bot_agent.sessions.is_none() {
            return Err(mutsuki_web_host::WebHostError::InvalidConfig(
                "agent WebExtension requires an Agent management service".into(),
            ));
        }
        builder = builder.extension(
            BotAgentWebExtension::new(None)
                .with_connection_resolver(bot_agent.connections)
                .with_sessions(bot_agent.sessions)
                .with_frontend_assets(&asset_dirs.bot_agent_assets),
        );
    }
    if config.has_extension("bot-flow-editor") {
        let flow = bot_agent.flow.ok_or_else(|| {
            mutsuki_web_host::WebHostError::InvalidConfig(
                "bot-flow-editor requires the Flow provider and catalog service".into(),
            )
        })?;
        let service = config_service.ok_or_else(|| {
            mutsuki_web_host::WebHostError::InvalidConfig(
                "bot-flow-editor requires ConfigService".into(),
            )
        })?;
        builder = builder.extension(
            BotFlowEditorWebExtension::new(service, flow)
                .with_frontend_assets(&asset_dirs.bot_flow_assets),
        );
    }
    Ok((builder.build()?, asset_dirs))
}

pub(crate) fn base_builder(
    config: &WebConsoleConfig,
    secrets: &WebConsoleSecrets,
    asset_dirs: &ConsoleAssetDirs,
) -> MutsukiWebHostBuilder {
    // shell_dir must differ from overview_assets (self-copy truncates files).
    let shell = WebShellAssets {
        root_dir: asset_dirs.overview_assets.clone(),
        index_file: "index.html".into(),
        import_map: Default::default(),
    };
    MutsukiWebHost::builder()
        .application(MinimalWebApplication::new(
            WebApplicationDescriptor {
                id: CONSOLE_APPLICATION_ID.into(),
                name: "Mutsuki Console".into(),
                version: "0.1.0".into(),
                brand: Some("Mutsuki".into()),
                theme: Some("lilia".into()),
            },
            shell,
        ))
        .listen(&config.listen)
        .mode(DeploymentMode::Embedded)
        .shell_dir(&asset_dirs.shell_root)
        .auth_token(secrets.auth_token.clone())
}

/// Temp directories holding materialized frontend assets. Keep alive while host runs.
pub struct ConsoleAssetDirs {
    pub _overview_dir: tempfile::TempDir,
    pub _control_dir: tempfile::TempDir,
    pub _config_dir: Option<tempfile::TempDir>,
    pub _upgrade_dir: Option<tempfile::TempDir>,
    pub _bilibili_dir: Option<tempfile::TempDir>,
    pub _qq_dir: Option<tempfile::TempDir>,
    pub _bot_agent_dir: Option<tempfile::TempDir>,
    pub _bot_flow_dir: Option<tempfile::TempDir>,
    pub _shell_dir: tempfile::TempDir,
    pub overview_assets: PathBuf,
    pub control_assets: PathBuf,
    pub config_assets: PathBuf,
    pub upgrade_assets: PathBuf,
    pub bilibili_assets: PathBuf,
    pub qq_assets: PathBuf,
    pub bot_agent_assets: PathBuf,
    pub bot_flow_assets: PathBuf,
    pub shell_root: PathBuf,
}

impl ConsoleAssetDirs {
    fn materialize(
        include_config: bool,
        include_upgrade: bool,
        include_bilibili: bool,
        include_qq: bool,
        include_agent_connections: bool,
        include_bot_flow: bool,
    ) -> WebHostResult<Self> {
        let overview_dir = tempfile::tempdir()
            .map_err(|err| mutsuki_web_host::WebHostError::Io(err.to_string()))?;
        let overview_assets = materialize_overview_assets(overview_dir.path())
            .map_err(|err| mutsuki_web_host::WebHostError::Io(err.to_string()))?;
        materialize_trajectory_assets(&overview_assets)
            .map_err(|err| mutsuki_web_host::WebHostError::Io(err.to_string()))?;
        let control_dir = tempfile::tempdir()
            .map_err(|err| mutsuki_web_host::WebHostError::Io(err.to_string()))?;
        let control_assets = materialize_control_assets(control_dir.path())
            .map_err(|err| mutsuki_web_host::WebHostError::Io(err.to_string()))?;
        materialize_trajectory_assets(&control_assets)
            .map_err(|err| mutsuki_web_host::WebHostError::Io(err.to_string()))?;
        copy_dir(&control_assets, &overview_assets.join("extensions/control"))
            .map_err(|err| mutsuki_web_host::WebHostError::Io(err.to_string()))?;

        // Config assets first so shell ?v= stamps match real config/index.js bytes.
        let (config_dir, config_assets) = if include_config {
            let dir = tempfile::tempdir()
                .map_err(|err| mutsuki_web_host::WebHostError::Io(err.to_string()))?;
            let assets = materialize_config_assets(dir.path())
                .map_err(|err| mutsuki_web_host::WebHostError::Io(err.to_string()))?;
            copy_dir(&assets, &overview_assets.join("extensions/config"))
                .map_err(|err| mutsuki_web_host::WebHostError::Io(err.to_string()))?;
            (Some(dir), assets)
        } else {
            (None, PathBuf::new())
        };

        let (upgrade_dir, upgrade_assets) = if include_upgrade {
            let dir = tempfile::tempdir()
                .map_err(|err| mutsuki_web_host::WebHostError::Io(err.to_string()))?;
            let assets = materialize_upgrade_assets(dir.path())
                .map_err(|err| mutsuki_web_host::WebHostError::Io(err.to_string()))?;
            copy_dir(&assets, &overview_assets.join("extensions/upgrade"))
                .map_err(|err| mutsuki_web_host::WebHostError::Io(err.to_string()))?;
            (Some(dir), assets)
        } else {
            (None, PathBuf::new())
        };

        let (bilibili_dir, bilibili_assets) = if include_bilibili {
            let dir = tempfile::tempdir()
                .map_err(|err| mutsuki_web_host::WebHostError::Io(err.to_string()))?;
            let assets = materialize_bilibili_assets(dir.path())
                .map_err(|err| mutsuki_web_host::WebHostError::Io(err.to_string()))?;
            copy_dir(&assets, &overview_assets.join("extensions/bilibili"))
                .map_err(|err| mutsuki_web_host::WebHostError::Io(err.to_string()))?;
            (Some(dir), assets)
        } else {
            (None, PathBuf::new())
        };

        let (qq_dir, qq_assets) = if include_qq {
            let dir = tempfile::tempdir()
                .map_err(|err| mutsuki_web_host::WebHostError::Io(err.to_string()))?;
            let assets = materialize_qq_assets(dir.path())
                .map_err(|err| mutsuki_web_host::WebHostError::Io(err.to_string()))?;
            copy_dir(&assets, &overview_assets.join("extensions/qq-bot"))
                .map_err(|err| mutsuki_web_host::WebHostError::Io(err.to_string()))?;
            (Some(dir), assets)
        } else {
            (None, PathBuf::new())
        };

        let (bot_agent_dir, bot_agent_assets) = if include_agent_connections {
            let dir = tempfile::tempdir()
                .map_err(|err| mutsuki_web_host::WebHostError::Io(err.to_string()))?;
            let assets = materialize_bot_agent_assets(dir.path())
                .map_err(|err| mutsuki_web_host::WebHostError::Io(err.to_string()))?;
            copy_dir(&assets, &overview_assets.join("extensions/bot-agent"))
                .map_err(|err| mutsuki_web_host::WebHostError::Io(err.to_string()))?;
            (Some(dir), assets)
        } else {
            (None, PathBuf::new())
        };

        let (bot_flow_dir, bot_flow_assets) = if include_bot_flow {
            let dir = tempfile::tempdir()
                .map_err(|err| mutsuki_web_host::WebHostError::Io(err.to_string()))?;
            let assets = materialize_bot_flow_assets(dir.path())
                .map_err(|err| mutsuki_web_host::WebHostError::Io(err.to_string()))?;
            copy_dir(&assets, &overview_assets.join("extensions/bot-flow"))
                .map_err(|err| mutsuki_web_host::WebHostError::Io(err.to_string()))?;
            (Some(dir), assets)
        } else {
            (None, PathBuf::new())
        };

        materialize_console_shell(
            &overview_assets,
            include_config,
            include_upgrade,
            include_bilibili,
            include_qq,
            include_agent_connections,
            include_bot_flow,
        )
        .map_err(|err| mutsuki_web_host::WebHostError::Io(err.to_string()))?;

        let shell_dir = tempfile::tempdir()
            .map_err(|err| mutsuki_web_host::WebHostError::Io(err.to_string()))?;
        Ok(Self {
            overview_assets: overview_assets.clone(),
            control_assets,
            config_assets,
            upgrade_assets,
            bilibili_assets,
            qq_assets,
            bot_agent_assets,
            bot_flow_assets,
            shell_root: shell_dir.path().to_path_buf(),
            _overview_dir: overview_dir,
            _control_dir: control_dir,
            _config_dir: config_dir,
            _upgrade_dir: upgrade_dir,
            _bilibili_dir: bilibili_dir,
            _qq_dir: qq_dir,
            _bot_agent_dir: bot_agent_dir,
            _bot_flow_dir: bot_flow_dir,
            _shell_dir: shell_dir,
        })
    }
}

fn asset_version_stamp(bytes: &[u8]) -> String {
    content_hash(bytes)
        .strip_prefix("sha256:")
        .unwrap_or_default()
        .chars()
        .take(12)
        .collect()
}

pub(crate) fn materialize_console_shell(
    out_dir: &Path,
    include_config: bool,
    include_upgrade: bool,
    include_bilibili: bool,
    include_qq: bool,
    include_agent_connections: bool,
    include_bot_flow: bool,
) -> std::io::Result<()> {
    let index_template = include_str!("../assets/console-shell-overview.html");
    let bootstrap_name = "console-bootstrap.js";
    let bootstrap_template = include_str!("../assets/console-bootstrap.js");

    let css = include_str!("../assets/mutsuki-ui.css");
    let css_v = asset_version_stamp(css.as_bytes());

    let overview_js = std::fs::read(out_dir.join("index.js"))?;
    let overview_dir = out_dir.join("extensions/overview");
    std::fs::create_dir_all(&overview_dir)?;
    std::fs::write(overview_dir.join("index.js"), &overview_js)?;
    let overview_v = asset_version_stamp(&overview_js);
    let bootstrap = bootstrap_template.to_owned();
    let bootstrap_v = asset_version_stamp(bootstrap.as_bytes());
    let web_sdk = include_bytes!("../../../../../hosts/web/packages/web-sdk/browser/web-sdk.js");
    let web_shell =
        include_bytes!("../../../../../hosts/web/packages/web-shell/browser/web-shell.js");

    let index = index_template
        .replace("./mutsuki-ui.css", &format!("./mutsuki-ui.css?v={css_v}"))
        .replace(
            &format!("./{bootstrap_name}"),
            &format!("./{bootstrap_name}?v={bootstrap_v}"),
        );

    std::fs::write(out_dir.join("index.html"), index)?;
    std::fs::write(out_dir.join(bootstrap_name), bootstrap)?;
    std::fs::write(out_dir.join("mutsuki-ui.css"), css)?;
    std::fs::create_dir_all(out_dir.join("shared"))?;
    std::fs::write(out_dir.join("shared/web-sdk.js"), web_sdk)?;
    std::fs::write(out_dir.join("shared/web-shell.js"), web_shell)?;
    let control_path = "extensions/control/index.js";
    let control_v = asset_version_stamp(&std::fs::read(out_dir.join(control_path))?);
    let mut extensions = vec![
        json!({
            "id": "overview",
            "url": format!("./extensions/overview/index.js?v={overview_v}"),
        }),
        json!({
            "id": "control",
            "url": format!("./{control_path}?v={control_v}"),
        }),
    ];
    for (id, path, enabled) in [
        ("config", "extensions/config/index.js", include_config),
        ("upgrade", "extensions/upgrade/index.js", include_upgrade),
        ("bilibili", "extensions/bilibili/index.js", include_bilibili),
        ("qq-bot", "extensions/qq-bot/index.js", include_qq),
        (
            "bot-agent",
            "extensions/bot-agent/index.js",
            include_agent_connections,
        ),
        (
            "bot-flow-editor",
            "extensions/bot-flow/index.js",
            include_bot_flow,
        ),
    ] {
        if enabled {
            let bytes = std::fs::read(out_dir.join(path))?;
            extensions.push(json!({
                "id": id,
                "url": format!("./{path}?v={}", asset_version_stamp(&bytes)),
            }));
        }
    }
    let mut activities = vec![
        json!({"id": "home", "label": "概览", "icon": "home", "order": 0, "position": "top"}),
        json!({"id": "bot", "label": "Bot", "icon": "bot", "order": 10, "position": "top"}),
        json!({"id": "automation", "label": "自动化", "icon": "flow", "order": 20, "position": "top"}),
        json!({"id": "system", "label": "系统", "icon": "system", "order": 30, "position": "top"}),
    ];
    if include_config {
        activities.push(json!({
            "id": "config",
            "label": "配置",
            "icon": "config",
            "order": 40,
            "position": "top"
        }));
    }
    activities.push(json!({
        "id": "settings",
        "label": "设置",
        "icon": "settings",
        "order": 100,
        "position": "bottom"
    }));
    let options = json!({ "activities": activities, "extensions": extensions });
    std::fs::write(
        out_dir.join("console-options.json"),
        serde_json::to_string(&options)?,
    )?;
    Ok(())
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod owner_console_tests {
    use super::*;

    #[test]
    fn owner_pages_are_materialized_only_for_registered_services() {
        let dirs = ConsoleAssetDirs::materialize(false, false, false, false, true, true).unwrap();
        assert!(
            dirs.overview_assets
                .join("extensions/bot-agent/index.js")
                .is_file()
        );
        assert!(
            dirs.overview_assets
                .join("extensions/bot-agent/trajectory-view.js")
                .is_file()
        );
        assert!(
            dirs.overview_assets
                .join("extensions/bot-flow/index.js")
                .is_file()
        );
        let options: serde_json::Value = serde_json::from_slice(
            &std::fs::read(dirs.overview_assets.join("console-options.json")).unwrap(),
        )
        .unwrap();
        let ids = options["extensions"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|item| item["id"].as_str())
            .collect::<Vec<_>>();
        assert!(ids.contains(&"bot-agent"));
        assert!(ids.contains(&"bot-flow-editor"));

        let dirs = ConsoleAssetDirs::materialize(false, false, false, false, false, false).unwrap();
        assert!(!dirs.overview_assets.join("extensions/bot-agent").exists());
        assert!(!dirs.overview_assets.join("extensions/bot-flow").exists());
        assert!(dirs.overview_assets.join("trajectory-model.js").is_file());
        assert!(dirs.overview_assets.join("trajectory-view.js").is_file());
    }

    #[test]
    fn enabled_panel_modules_use_their_materialized_content_versions() {
        let dirs = ConsoleAssetDirs::materialize(true, false, true, true, true, true).unwrap();
        let options =
            std::fs::read_to_string(dirs.overview_assets.join("console-options.json")).unwrap();
        for module_path in [
            "extensions/config/index.js",
            "extensions/bilibili/index.js",
            "extensions/qq-bot/index.js",
            "extensions/bot-agent/index.js",
            "extensions/bot-flow/index.js",
        ] {
            let bytes = std::fs::read(dirs.overview_assets.join(module_path)).unwrap();
            let expected = format!("./{module_path}?v={}", asset_version_stamp(&bytes));
            assert!(
                options.contains(&expected),
                "missing versioned {module_path}"
            );
        }
    }

    #[test]
    fn enabled_panel_with_missing_assets_fails_materialization() {
        let root = tempfile::tempdir().unwrap();
        materialize_overview_assets(root.path()).unwrap();
        let error = materialize_console_shell(root.path(), true, false, false, false, false, false)
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    }
}
