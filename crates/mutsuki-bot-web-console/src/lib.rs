//! Product-level Bot Web Console assembly (Embedded WebHost + extensions).
//!
//! Console = WebApplication shell + control/overview/(optional) config WebExtensions.
//! Does not embed business pages into WebHost Recovery.

mod secret_status;

use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use secret_status::{SecretKeyResolver, SecretMonitor, SecretStatusWebExtension};

use mutsuki_bot_config::{ConfigProviderRegistry, ConfigService};
use mutsuki_plugin_bot_config_web::{
    ConfigWebExtension, materialize_frontend_assets as materialize_config_assets,
};
use mutsuki_plugin_bot_control_web::{ControlRpcCaller, ControlWebExtension};
use mutsuki_plugin_bot_overview_web::{
    OverviewWebExtension, materialize_frontend_assets as materialize_overview_assets,
};
use mutsuki_plugin_bot_upgrade_web::UpgradeWebExtension;
use mutsuki_service_control::ControlHandler;
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
    #[serde(default)]
    pub include_config: bool,
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
}

/// Resolved console auth token from Host secret store.
pub struct WebConsoleSecrets {
    pub auth_token: String,
}

/// Empty ConfigService for products that enable `include_config` before registering providers.
pub fn empty_config_service() -> Arc<ConfigService> {
    let registry = Arc::new(ConfigProviderRegistry::default());
    Arc::new(ConfigService::new(registry))
}

/// Resolved filesystem paths for optional console features.
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

impl Default for WebConsolePaths {
    fn default() -> Self {
        Self { release_set: None }
    }
}

/// Build an embedded WebHost pre-wired with control + overview (+ optional config/upgrade) extensions.
pub fn build_console_host(
    config: &WebConsoleConfig,
    secrets: &WebConsoleSecrets,
    control: Arc<dyn ControlHandler>,
    control_token: &str,
    config_service: Option<Arc<ConfigService>>,
    secret_monitor: Option<SecretMonitor>,
    paths: &WebConsolePaths,
) -> WebHostResult<(MutsukiWebHost, ConsoleAssetDirs)> {
    if !config.enabled {
        return Err(mutsuki_web_host::WebHostError::InvalidConfig(
            "web.console.enabled is false".into(),
        ));
    }

    let asset_dirs = ConsoleAssetDirs::materialize(
        config.include_config && config_service.is_some(),
        paths.release_set.is_some(),
    )?;
    let caller = ControlRpcCaller::new(control, control_token);
    let mut builder = base_builder(config, secrets, &asset_dirs);
    builder = builder.extension(ControlWebExtension::new(caller.clone()));
    builder = builder.extension(
        OverviewWebExtension::new(caller.clone()).with_frontend_assets(&asset_dirs.overview_assets),
    );
    if let Some(monitor) = secret_monitor {
        builder = builder.extension(SecretStatusWebExtension::new(monitor));
    }
    if let Some(release_set_path) = &paths.release_set {
        builder = builder.extension(
            UpgradeWebExtension::new(release_set_path)
                .map_err(|err| mutsuki_web_host::WebHostError::InvalidConfig(err.to_string()))?,
        );
    }
    if config.include_config {
        let service = config_service.ok_or_else(|| {
            mutsuki_web_host::WebHostError::InvalidConfig(
                "web.console.include_config requires ConfigService".into(),
            )
        })?;
        builder = builder.extension(
            ConfigWebExtension::new(service).with_frontend_assets(&asset_dirs.config_assets),
        );
    }
    Ok((builder.build()?, asset_dirs))
}

fn base_builder(
    config: &WebConsoleConfig,
    secrets: &WebConsoleSecrets,
    asset_dirs: &ConsoleAssetDirs,
) -> MutsukiWebHostBuilder {
    let shell = WebShellAssets {
        root_dir: asset_dirs.shell_root.clone(),
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
    pub _config_dir: Option<tempfile::TempDir>,
    pub _shell_dir: tempfile::TempDir,
    pub overview_assets: PathBuf,
    pub config_assets: PathBuf,
    pub shell_root: PathBuf,
}

impl ConsoleAssetDirs {
    fn materialize(include_config: bool, include_upgrade: bool) -> WebHostResult<Self> {
        let overview_dir = tempfile::tempdir()
            .map_err(|err| mutsuki_web_host::WebHostError::Io(err.to_string()))?;
        let overview_assets = materialize_overview_assets(overview_dir.path())
            .map_err(|err| mutsuki_web_host::WebHostError::Io(err.to_string()))?;
        materialize_console_shell(&overview_assets, include_config, include_upgrade)
            .map_err(|err| mutsuki_web_host::WebHostError::Io(err.to_string()))?;

        let (config_dir, config_assets) = if include_config {
            let dir = tempfile::tempdir()
                .map_err(|err| mutsuki_web_host::WebHostError::Io(err.to_string()))?;
            let assets = materialize_config_assets(dir.path())
                .map_err(|err| mutsuki_web_host::WebHostError::Io(err.to_string()))?;
            copy_dir(&assets, &overview_assets.join("config"))
                .map_err(|err| mutsuki_web_host::WebHostError::Io(err.to_string()))?;
            (Some(dir), assets)
        } else {
            (None, PathBuf::new())
        };

        Ok(Self {
            overview_assets: overview_assets.clone(),
            config_assets,
            shell_root: overview_assets,
            _overview_dir: overview_dir,
            _config_dir: config_dir,
            _shell_dir: tempfile::tempdir()
                .map_err(|err| mutsuki_web_host::WebHostError::Io(err.to_string()))?,
        })
    }
}

fn materialize_console_shell(
    out_dir: &Path,
    include_config: bool,
    include_upgrade: bool,
) -> std::io::Result<()> {
    let index = if include_config {
        include_str!("../assets/console-shell-config.html")
    } else {
        include_str!("../assets/console-shell-overview.html")
    };
    std::fs::write(out_dir.join("index.html"), index)?;
    std::fs::write(
        out_dir.join("console-options.json"),
        serde_json::to_string(&json!({
            "includeConfig": include_config,
            "includeUpgrade": include_upgrade,
        }))?,
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
