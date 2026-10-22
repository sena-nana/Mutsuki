//! Bot-package WebHost assembly helper (Embedded WebHost + admin WebExtensions).
//!
//! Products may opt in. Console = WebApplication shell + control/overview/(optional) config
//! WebExtensions. Does not own Host lifecycle and does not embed business pages into Recovery.
// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::default_trait_access,
    clippy::doc_markdown,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::pub_underscore_fields,
    clippy::question_mark,
    clippy::too_many_arguments,
    clippy::too_many_lines
)]

mod config_demo;
mod layout;
mod secret_status;
mod watch_bridge;

use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use config_demo::demo_config_service;
pub use layout::{ActivityPosition, ConsoleActivity, ConsoleLayout, SETTINGS_ACTIVITY};
pub use secret_status::{SecretKeyResolver, SecretMonitor, SecretStatusWebExtension};
pub use watch_bridge::{
    BilibiliResolver, ControlChangeBridge, ManagementChangeBridge, attach_control_changed_bridge,
    attach_management_changed_bridges, attach_revision_changed_bridge,
};

use mutsuki_agent_web_extension::{
    AgentConnectionManagementResolver, BotAgentWebExtension, LocalAgentManagementResolver,
    materialize_frontend_assets as materialize_bot_agent_assets,
};
use mutsuki_bot_flow::BotFlowRegistry;
use mutsuki_bot_management::BilibiliManagementApi;
use mutsuki_bot_sandbox::SandboxApi;
use mutsuki_bot_state_db::BotStateDbRepository;
use mutsuki_bot_web_extension_bilibili::{
    BilibiliWebExtension, materialize_frontend_assets as materialize_bilibili_assets,
};
use mutsuki_bot_web_extension_database::{
    DatabaseWebExtension, materialize_frontend_assets as materialize_database_assets,
};
use mutsuki_bot_web_extension_flow::{
    BotFlowEditorWebExtension, materialize_frontend_assets as materialize_bot_flow_assets,
};
use mutsuki_bot_web_extension_platforms::{PlatformProviderSource, PlatformsWebExtension};
use mutsuki_bot_web_extension_qq::{
    QqBotManagementApi, QqBotWebExtension, materialize_frontend_assets as materialize_qq_assets,
};
use mutsuki_bot_web_extension_sandbox::{
    SandboxWebExtension, materialize_frontend_assets as materialize_sandbox_assets,
};
use mutsuki_config_service::{ConfigProviderRegistry, ConfigService, InMemoryConfigRepository};
use mutsuki_service_control::ControlHandler;
use mutsuki_std_web_extension_config::{
    ConfigWebExtension, materialize_frontend_assets as materialize_config_assets,
};
use mutsuki_web_extension::content_hash;
use mutsuki_web_extension_control::{
    ControlRpcCaller, ControlWebExtension,
    materialize_frontend_assets as materialize_control_assets,
};
use mutsuki_web_extension_overview::{
    OverviewWebExtension, materialize_frontend_assets as materialize_overview_assets,
};
use mutsuki_web_extension_upgrade::{
    UpgradeWebExtension, materialize_frontend_assets as materialize_upgrade_assets,
};
use mutsuki_web_host::{
    MinimalWebApplication, MutsukiWebHost, MutsukiWebHostBuilder, WebHostResult,
};
use mutsuki_web_protocol::{DeploymentMode, WebApplicationDescriptor, WebShellAssets};
use mutsuki_web_shared_runtime::shared_runtime;
use serde_json::json;

pub use mutsuki_std_web_extension_config::{ConfigNavigationGroup, ConfigNavigationItem};

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
    /// Product-owned config navigation grouping and labels.
    #[serde(default, skip)]
    pub config_navigation_groups: Vec<ConfigNavigationGroup>,
    /// Relative path to active release set manifest (enables auto-upgrade page).
    pub release_set: Option<String>,
    /// Product-owned activities; extensions cannot add any.
    #[serde(default, skip)]
    pub layout: ConsoleLayout,
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

/// Owner services that make Agent connection management and Bot flow authoring real.
#[derive(Default)]
pub struct BotAgentConsoleServices {
    pub connections: Option<AgentConnectionManagementResolver>,
    pub sessions: Option<LocalAgentManagementResolver>,
    pub flow: Option<Arc<BotFlowRegistry>>,
}

/// Owner services the console exposes; every field is optional and pages follow their owner.
#[derive(Default)]
pub struct BotConsoleServices {
    pub config: Option<Arc<ConfigService>>,
    pub secret_monitor: Option<SecretMonitor>,
    pub bilibili: Option<Arc<dyn BilibiliManagementApi>>,
    pub qq: Option<Arc<dyn QqBotManagementApi>>,
    pub sandbox: Option<Arc<dyn SandboxApi>>,
    pub database: Option<Arc<BotStateDbRepository>>,
    pub agent: BotAgentConsoleServices,
    /// Live platform adapters for the generic `platforms` surface.
    pub platforms: Option<PlatformProviderSource>,
}

/// Builds the embedded Bot console; each page is mounted only when its owner service exists.
pub fn build_bot_console(
    config: &WebConsoleConfig,
    secrets: &WebConsoleSecrets,
    control: Arc<dyn ControlHandler>,
    control_token: &str,
    paths: &WebConsolePaths,
    services: BotConsoleServices,
) -> WebHostResult<(MutsukiWebHost, ConsoleAssetDirs)> {
    let BotConsoleServices {
        config: config_service,
        secret_monitor,
        bilibili,
        qq,
        sandbox,
        database,
        agent: bot_agent,
        platforms,
    } = services;
    if !config.enabled {
        return Err(mutsuki_web_host::WebHostError::InvalidConfig(
            "web.console.enabled is false".into(),
        ));
    }
    config
        .layout
        .validate()
        .map_err(mutsuki_web_host::WebHostError::InvalidConfig)?;

    // Unlike the other optional pages this one follows the service, not the id: the
    // console shows Bilibili exactly when the plugin is wired. Naming it in
    // `extensions` therefore cannot enable it, only assert it -- a missing service
    // becomes a startup error instead of a page that silently never appears.
    let include_bilibili = match &bilibili {
        Some(_) => true,
        None if config.has_extension("bilibili") => {
            return Err(mutsuki_web_host::WebHostError::InvalidConfig(
                "bilibili WebExtension requires its management service".into(),
            ));
        }
        None => false,
    };
    let selected = OPTIONAL_PAGES
        .iter()
        .filter(|page| match page.id {
            BILIBILI_PAGE => include_bilibili,
            // Like Bilibili, the accounts page follows its provider source, not a selection key.
            PLATFORMS_PAGE => platforms.is_some(),
            _ => config.has_extension(page.selection),
        })
        .map(|page| page.id)
        .collect::<Vec<_>>();
    let asset_dirs = ConsoleAssetDirs::materialize(&selected, &config.layout)?;
    let page = |id: &str| asset_dirs.page_assets(id).expect("selected page is staged");
    let caller = ControlRpcCaller::new(control, control_token);
    let mut builder = base_builder(config, secrets, &asset_dirs);
    builder = builder.extension(
        ControlWebExtension::new(caller.clone()).with_frontend_assets(page(DIAGNOSTICS_PAGE)),
    );
    builder = builder.extension(
        OverviewWebExtension::new(caller.clone()).with_frontend_assets(page(OVERVIEW_PAGE)),
    );
    builder = builder
        .extension(DatabaseWebExtension::new(database).with_frontend_assets(page(DATABASE_PAGE)));
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
                .with_frontend_assets(page(UPGRADE_PAGE)),
        );
    }
    if config.has_extension("config") {
        let service = config_service.clone().ok_or_else(|| {
            mutsuki_web_host::WebHostError::InvalidConfig(
                "web.console.include_config requires ConfigService".into(),
            )
        })?;
        let mut extension =
            ConfigWebExtension::new(service).with_frontend_assets(page(CONFIG_PAGE));
        if !config.config_provider_ids.is_empty() {
            extension = extension.with_visible_providers(config.config_provider_ids.clone());
        }
        if !config.config_navigation_groups.is_empty() {
            extension = extension.with_navigation_groups(config.config_navigation_groups.clone());
        }
        builder = builder.extension(extension);
    }
    if let Some(service) = bilibili {
        builder = builder.extension(
            BilibiliWebExtension::new(service).with_frontend_assets(page(BILIBILI_PAGE)),
        );
    }
    if config.has_extension("qq") {
        let api = qq.ok_or_else(|| {
            mutsuki_web_host::WebHostError::InvalidConfig(
                "qq WebExtension requires its management service".into(),
            )
        })?;
        builder =
            builder.extension(QqBotWebExtension::new(api).with_frontend_assets(page(QQ_PAGE)));
    }
    if config.has_extension("sandbox") {
        let api = sandbox.ok_or_else(|| {
            mutsuki_web_host::WebHostError::InvalidConfig(
                "sandbox WebExtension requires its management service".into(),
            )
        })?;
        builder = builder
            .extension(SandboxWebExtension::new(api).with_frontend_assets(page(SANDBOX_PAGE)));
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
                .with_frontend_assets(page(AGENT_PAGE)),
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
            BotFlowEditorWebExtension::new(service, flow).with_frontend_assets(page(FLOW_PAGE)),
        );
    }
    if let Some(providers) = platforms {
        builder = builder.extension(
            PlatformsWebExtension::new(providers).with_frontend_assets(page(PLATFORMS_PAGE)),
        );
    }
    let host = builder.build()?;
    Ok((host, asset_dirs))
}

pub(crate) fn base_builder(
    config: &WebConsoleConfig,
    secrets: &WebConsoleSecrets,
    asset_dirs: &ConsoleAssetDirs,
) -> MutsukiWebHostBuilder {
    let import_map = serde_json::from_str::<serde_json::Value>(shared_runtime().import_map_json())
        .ok()
        .and_then(|map| map.get("imports").and_then(|v| v.as_object()).cloned())
        .unwrap_or_default();
    // The host copies the document tree into `shell_dir`, so the two must differ.
    let shell = WebShellAssets {
        root_dir: asset_dirs.document_root().to_path_buf(),
        index_file: "index.html".into(),
        import_map,
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
        .shell_dir(asset_dirs.shell_root())
        .auth_token(secrets.auth_token.clone())
}

const OVERVIEW_PAGE: &str = mutsuki_web_extension_overview::PLUGIN_ID;
const DIAGNOSTICS_PAGE: &str = mutsuki_web_extension_control::PLUGIN_ID;
const DATABASE_PAGE: &str = mutsuki_bot_web_extension_database::PLUGIN_ID;
const UPGRADE_PAGE: &str = mutsuki_web_extension_upgrade::PLUGIN_ID;
const BILIBILI_PAGE: &str = mutsuki_bot_web_extension_bilibili::PLUGIN_ID;
const QQ_PAGE: &str = mutsuki_bot_web_extension_qq::PLUGIN_ID;
const PLATFORMS_PAGE: &str = mutsuki_bot_web_extension_platforms::PLUGIN_ID;
const SANDBOX_PAGE: &str = "sandbox";
const AGENT_PAGE: &str = mutsuki_agent_web_extension::PLUGIN_ID;
const FLOW_PAGE: &str = "bot-flow-editor";
const CONFIG_PAGE: &str = "config";

/// One console page module: how the product selects it and where the shell loads it from.
struct ConsolePage {
    /// Id in `console-options.json` and the key for [`ConsoleAssetDirs::page_assets`].
    id: &'static str,
    /// Name in `[web.console] extensions` that selects an optional page.
    selection: &'static str,
    /// Directory below `extensions/` the browser imports the module from.
    mount: &'static str,
    materialize: fn(&Path) -> std::io::Result<PathBuf>,
}

/// Always mounted, never gated.
const BASE_PAGES: [ConsolePage; 3] = [
    ConsolePage {
        id: OVERVIEW_PAGE,
        selection: "overview",
        mount: "overview",
        materialize: materialize_overview_assets,
    },
    ConsolePage {
        id: DIAGNOSTICS_PAGE,
        selection: "diagnostics",
        mount: "diagnostics",
        materialize: materialize_control_assets,
    },
    ConsolePage {
        id: DATABASE_PAGE,
        selection: DATABASE_PAGE,
        mount: "database",
        materialize: materialize_database_assets,
    },
];

/// Optional pages in load order. Config runs last so its plugin hubs can see the pages and
/// overview cards the others registered when deciding route availability.
const OPTIONAL_PAGES: [ConsolePage; 8] = [
    ConsolePage {
        id: PLATFORMS_PAGE,
        selection: "platforms",
        mount: "platforms",
        materialize: materialize_platforms_assets,
    },
    ConsolePage {
        id: UPGRADE_PAGE,
        selection: "upgrade",
        mount: "upgrade",
        materialize: materialize_upgrade_assets,
    },
    ConsolePage {
        id: BILIBILI_PAGE,
        selection: "bilibili",
        mount: "bilibili",
        materialize: materialize_bilibili_assets,
    },
    ConsolePage {
        id: QQ_PAGE,
        selection: "qq",
        mount: "qq-bot",
        materialize: materialize_qq_assets,
    },
    ConsolePage {
        id: SANDBOX_PAGE,
        selection: "sandbox",
        mount: "sandbox",
        materialize: materialize_sandbox_assets,
    },
    ConsolePage {
        id: AGENT_PAGE,
        selection: "agent",
        mount: "bot-agent",
        materialize: materialize_bot_agent_assets,
    },
    ConsolePage {
        id: FLOW_PAGE,
        selection: "bot-flow-editor",
        mount: "bot-flow",
        materialize: materialize_bot_flow_assets,
    },
    ConsolePage {
        id: CONFIG_PAGE,
        selection: "config",
        mount: "config",
        materialize: materialize_config_assets,
    },
];

/// Stages the platforms accounts page built in that crate's `web/`. The extension derives its
/// manifest from the staged entry, so it is written here from its own descriptor.
fn materialize_platforms_assets(out_dir: &Path) -> std::io::Result<PathBuf> {
    use mutsuki_web_extension::WebExtension as _;
    std::fs::create_dir_all(out_dir)?;
    std::fs::write(
        out_dir.join("index.js"),
        include_str!("../../mutsuki-bot-web-extension-platforms/web/browser/index.js"),
    )?;
    let manifest = PlatformsWebExtension::new(Arc::new(Vec::new))
        .with_frontend_assets(out_dir)
        .descriptor();
    std::fs::write(
        out_dir.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).map_err(std::io::Error::other)?,
    )?;
    Ok(out_dir.to_path_buf())
}

/// One page's materialized assets. The temp dir must outlive the host.
struct StagedPage {
    id: &'static str,
    assets: PathBuf,
    _dir: tempfile::TempDir,
}

/// Temp directories holding the console document and every staged page. Keep alive while the
/// host runs.
pub struct ConsoleAssetDirs {
    document: tempfile::TempDir,
    shell: tempfile::TempDir,
    pages: Vec<StagedPage>,
}

/// Every `std::io` failure in console assembly surfaces the same way.
fn io_error(error: impl std::fmt::Display) -> mutsuki_web_host::WebHostError {
    mutsuki_web_host::WebHostError::Io(error.to_string())
}

impl ConsoleAssetDirs {
    /// Stages the base pages plus the `optional` page ids, then writes the console document.
    fn materialize(optional: &[&str], layout: &ConsoleLayout) -> WebHostResult<Self> {
        let document = tempfile::tempdir().map_err(io_error)?;
        let mut pages = Vec::new();
        let selected = BASE_PAGES.iter().chain(
            OPTIONAL_PAGES
                .iter()
                .filter(|page| optional.contains(&page.id)),
        );
        for page in selected {
            let dir = tempfile::tempdir().map_err(io_error)?;
            let assets = (page.materialize)(dir.path()).map_err(io_error)?;
            copy_dir(
                &assets,
                &document.path().join("extensions").join(page.mount),
            )
            .map_err(io_error)?;
            pages.push(StagedPage {
                id: page.id,
                assets,
                _dir: dir,
            });
        }
        materialize_console_shell(document.path(), optional, layout).map_err(io_error)?;
        Ok(Self {
            document,
            shell: tempfile::tempdir().map_err(io_error)?,
            pages,
        })
    }

    /// The console document tree (`index.html`, `shared/`, `extensions/`).
    #[must_use]
    pub fn document_root(&self) -> &Path {
        self.document.path()
    }

    /// Directory the host serves from (a copy of the document tree).
    #[must_use]
    pub fn shell_root(&self) -> &Path {
        self.shell.path()
    }

    /// Materialized assets of one staged page, by console page id.
    #[must_use]
    pub fn page_assets(&self, id: &str) -> Option<&Path> {
        self.pages
            .iter()
            .find(|page| page.id == id)
            .map(|page| page.assets.as_path())
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

fn materialize_lilia_fonts(out_dir: &Path) -> std::io::Result<()> {
    let dir = out_dir.join("fonts");
    std::fs::create_dir_all(&dir)?;
    macro_rules! write_font {
        ($weight:literal) => {
            std::fs::write(
                dir.join(concat!(
                    "noto-sans-sc-chinese-simplified-",
                    $weight,
                    "-normal.woff2"
                )),
                include_bytes!(concat!(
                    "../../../../../products/bot/assets/fonts/noto-sans-sc-chinese-simplified-",
                    $weight,
                    "-normal.woff2"
                )),
            )?;
        };
    }
    write_font!("400");
    write_font!("500");
    write_font!("600");
    write_font!("700");
    Ok(())
}

/// Document CSP. `script-src` admits only same-origin modules and the inline import map.
fn document_csp(optional: &[&str]) -> String {
    let extra = if optional.contains(&QQ_PAGE) || optional.contains(&SANDBOX_PAGE) {
        format!(" {}", mutsuki_bot_web_extension_qq::EXTRA_IMG_SRC.join(" "))
    } else {
        String::new()
    };
    let import_map = shared_runtime().csp_script_hash();
    format!(
        "default-src 'self'; connect-src 'self' ws: wss:; img-src 'self' data: blob:{extra}; media-src 'self' blob:{extra}; script-src 'self' {import_map}; style-src 'self' 'unsafe-inline'; object-src 'none'; base-uri 'self'"
    )
}

/// Module URL plus the manifest-declared stylesheets of one staged page.
fn console_extension_entry(
    out_dir: &Path,
    page: &ConsolePage,
) -> std::io::Result<serde_json::Value> {
    let root = out_dir.join("extensions").join(page.mount);
    let manifest: mutsuki_web_protocol::ExtensionManifest =
        serde_json::from_slice(&std::fs::read(root.join("manifest.json"))?)
            .map_err(std::io::Error::other)?;
    let versioned = |path: &str| -> std::io::Result<String> {
        let bytes = std::fs::read(root.join(path))?;
        Ok(format!(
            "./extensions/{}/{path}?v={}",
            page.mount,
            asset_version_stamp(&bytes)
        ))
    };
    let styles = manifest
        .styles
        .iter()
        .map(|style| versioned(style))
        .collect::<std::io::Result<Vec<_>>>()?;
    Ok(json!({
        "id": page.id,
        "url": versioned(&manifest.entry)?,
        "styles": styles,
    }))
}

/// Shell page shown for `#/` and unknown routes.
const HOME_PAGE_ID: &str = "overview.page";

pub(crate) fn materialize_console_shell(
    out_dir: &Path,
    optional: &[&str],
    layout: &ConsoleLayout,
) -> std::io::Result<()> {
    let runtime = shared_runtime();
    runtime.write_to(out_dir)?;
    materialize_lilia_fonts(out_dir)?;

    // Built by the `web/` package (`@mutsuki/bot-console`); it imports the shell through the
    // import map and starts it with `console-options.json`.
    let entry_name = "console.js";
    let entry = include_str!("../web/browser/console.js");
    std::fs::write(out_dir.join(entry_name), entry)?;
    let index = include_str!("../assets/console-shell-overview.html")
        .replace("__CONSOLE_CSP__", &document_csp(optional))
        .replace("__SHARED_STYLESHEET__", runtime.stylesheet_path())
        .replace("__IMPORT_MAP__", &runtime.import_map_script())
        .replace(
            &format!("./{entry_name}"),
            &format!("./{entry_name}?v={}", asset_version_stamp(entry.as_bytes())),
        );
    std::fs::write(out_dir.join("index.html"), index)?;

    let extensions = BASE_PAGES
        .iter()
        .chain(
            OPTIONAL_PAGES
                .iter()
                .filter(|page| optional.contains(&page.id)),
        )
        .map(|page| console_extension_entry(out_dir, page))
        .collect::<std::io::Result<Vec<_>>>()?;
    let options = json!({
        "brand": "Mutsuki",
        "homePageId": HOME_PAGE_ID,
        "layout": layout,
        "extensions": extensions,
    });
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

    fn extension_ids(dirs: &ConsoleAssetDirs) -> Vec<String> {
        let options: serde_json::Value = serde_json::from_slice(
            &std::fs::read(dirs.document_root().join("console-options.json")).unwrap(),
        )
        .unwrap();
        options["extensions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["id"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn owner_pages_are_materialized_only_for_registered_services() {
        let dirs =
            ConsoleAssetDirs::materialize(&[AGENT_PAGE, FLOW_PAGE], &ConsoleLayout::default())
                .unwrap();
        let root = dirs.document_root();
        for staged in [
            "extensions/bot-agent/index.js",
            "extensions/bot-flow/index.js",
            "extensions/bot-flow/lilia-node-editor.js",
        ] {
            assert!(root.join(staged).is_file(), "{staged} not staged");
        }
        assert_eq!(
            extension_ids(&dirs),
            [
                "mutsuki.web.overview",
                "mutsuki.web.diagnostics",
                "mutsuki.bot.database",
                "mutsuki.agent.console",
                "bot-flow-editor"
            ]
        );
        assert!(dirs.page_assets(AGENT_PAGE).is_some());

        let dirs = ConsoleAssetDirs::materialize(&[], &ConsoleLayout::default()).unwrap();
        let root = dirs.document_root();
        assert!(!root.join("extensions/bot-agent").exists());
        assert!(!root.join("extensions/bot-flow").exists());
        // The RuntimeEvent timeline is bundled into the diagnostics page; nothing is staged
        // at the document root for it any more.
        assert!(!root.join("trajectory-model.js").exists());
        assert!(root.join("extensions/diagnostics/index.js").is_file());
        assert!(root.join("extensions/database/index.js").is_file());
        assert_eq!(
            extension_ids(&dirs),
            [
                "mutsuki.web.overview",
                "mutsuki.web.diagnostics",
                "mutsuki.bot.database"
            ]
        );
        assert!(dirs.page_assets(AGENT_PAGE).is_none());
    }

    #[test]
    fn enabled_panel_modules_use_their_materialized_content_versions() {
        let all = OPTIONAL_PAGES
            .iter()
            .map(|page| page.id)
            .collect::<Vec<_>>();
        let dirs = ConsoleAssetDirs::materialize(&all, &ConsoleLayout::default()).unwrap();
        let options =
            std::fs::read_to_string(dirs.document_root().join("console-options.json")).unwrap();
        for page in BASE_PAGES.iter().chain(&OPTIONAL_PAGES) {
            let module_path = format!("extensions/{}/index.js", page.mount);
            let bytes = std::fs::read(dirs.document_root().join(&module_path)).unwrap();
            let expected = format!("./{module_path}?v={}", asset_version_stamp(&bytes));
            assert!(
                options.contains(&expected),
                "missing versioned {module_path}"
            );
        }
        // Config loads last so its hubs see every other registration.
        assert_eq!(
            extension_ids(&dirs).last().map(String::as_str),
            Some(CONFIG_PAGE)
        );
    }

    #[test]
    fn enabled_panel_with_missing_assets_fails_materialization() {
        let root = tempfile::tempdir().unwrap();
        let error =
            materialize_console_shell(root.path(), &[CONFIG_PAGE], &ConsoleLayout::default())
                .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    }
}
