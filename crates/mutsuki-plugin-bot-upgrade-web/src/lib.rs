//! Auto-upgrade WebExtension: release set module checks + upgrade plan (Git fetch / build / ABI / pin).
//!
//! Does not run git/build in the WebHost process; Console surfaces checks and CLI-oriented plans.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mutsuki_plugin_bot_control_web::CAPABILITY_RUNTIME_READ;
use mutsuki_plugin_catalog::{
    ReleaseSetInfo, RemoteHeadProvider, ReqwestRemoteHeadProvider, UpgradeExecuteOptions,
    check_module_updates, execute_module_upgrade, format_execute_cli_command, load_release_set,
    plan_module_upgrade, upgrade_check_json,
};
use mutsuki_web_extension::{ExtensionError, RpcRegistry, WebExtension, WebExtensionDescriptor};
use mutsuki_web_protocol::{
    EXTENSION_MANIFEST_VERSION, ExtensionManifest, JsonValue, WEB_PROTOCOL_VERSION,
};
use serde_json::{Value, json};

pub const PLUGIN_ID: &str = "upgrade";
pub const PLUGIN_VERSION: &str = "0.1.0";

pub struct UpgradeWebExtension {
    inner: UpgradeRpc,
}

impl UpgradeWebExtension {
    pub fn new(release_set_path: impl AsRef<Path>) -> Result<Self, ExtensionError> {
        let release_set_path = release_set_path.as_ref().to_path_buf();
        let release_set = load_release_set(&release_set_path).map_err(map_catalog_error)?;
        if release_set.repositories.is_empty() {
            return Err(ExtensionError::Registration(
                "release set must declare [[repositories]] for auto-upgrade".into(),
            ));
        }
        Ok(Self {
            inner: UpgradeRpc {
                release_set,
                release_set_path,
                remote: Arc::new(ReqwestRemoteHeadProvider::default()),
            },
        })
    }

    pub fn with_remote_provider(mut self, remote: Arc<dyn RemoteHeadProvider>) -> Self {
        self.inner.remote = remote;
        self
    }
}

impl WebExtension for UpgradeWebExtension {
    fn descriptor(&self) -> WebExtensionDescriptor {
        ExtensionManifest {
            manifest_version: EXTENSION_MANIFEST_VERSION,
            id: PLUGIN_ID.into(),
            version: PLUGIN_VERSION.into(),
            entry: String::new(),
            capabilities: vec![CAPABILITY_RUNTIME_READ.into()],
            permissions: vec!["pages".into(), "navigation".into()],
            assets: vec![],
            protocol_version: WEB_PROTOCOL_VERSION.into(),
        }
    }

    fn frontend_assets(&self) -> Option<mutsuki_web_protocol::WebFrontendAssets> {
        None
    }

    fn register_rpc(&self, ctx: &mut RpcRegistry) -> Result<(), ExtensionError> {
        let inner = self.inner.clone();
        ctx.register("check", {
            let inner = inner.clone();
            move |params| inner.check(&params)
        });
        ctx.register("plan", {
            let inner = inner.clone();
            move |params| inner.plan(&params)
        });
        ctx.register("execute", {
            let inner = inner.clone();
            move |params| inner.execute(&params)
        });
        Ok(())
    }

    fn register_events(
        &self,
        _ctx: &mut mutsuki_web_extension::EventRegistry,
    ) -> Result<(), ExtensionError> {
        Ok(())
    }
}

#[derive(Clone)]
struct UpgradeRpc {
    release_set: ReleaseSetInfo,
    release_set_path: PathBuf,
    remote: Arc<dyn RemoteHeadProvider>,
}

impl UpgradeRpc {
    fn check(&self, params: &JsonValue) -> Result<Value, ExtensionError> {
        let query = params.get("query").and_then(|v| v.as_str());
        let release_set = self.release_set.clone();
        let remote = self.remote.clone();
        let modules = run_remote_check(release_set, remote)?;
        let filtered = filter_modules(modules, query);
        Ok(upgrade_check_json(&self.release_set, &filtered))
    }

    fn plan(&self, params: &JsonValue) -> Result<Value, ExtensionError> {
        let module_id = required_str(params, "module_id")?;
        let target_revision = params
            .get("target_revision")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| {
                let release_set = self.release_set.clone();
                let remote = self.remote.clone();
                run_remote_check(release_set, remote)
                    .ok()
                    .and_then(|modules| {
                        modules
                            .into_iter()
                            .find(|module| module.id == module_id)
                            .and_then(|module| module.remote_revision)
                    })
            });
        let plan = plan_module_upgrade(&self.release_set, &module_id, target_revision.as_deref())
            .map_err(map_catalog_error)?;
        Ok(json!({
            "module_id": module_id,
            "plan": plan,
            "cli_command": format_execute_cli_command(
                &self.release_set_path,
                &module_id,
                Some(plan.target_revision.as_str()),
                &UpgradeExecuteOptions::default(),
            ),
            "reload": {
                "namespace": "control",
                "method": "plugin_reload",
                "note": "Bot 插件 ABI 更新并完成 pin 同步后，在 Console 插件页执行重载；核心模块需重启 Runtime。"
            }
        }))
    }

    fn execute(&self, params: &JsonValue) -> Result<Value, ExtensionError> {
        let module_id = required_str(params, "module_id")?;
        let target_revision = params
            .get("target_revision")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let dry_run = params
            .get("dry_run")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        if !dry_run {
            return Err(ExtensionError::Registration(
                "upgrade.execute requires dry_run=true in WebHost; run mutsuki-plugin execute in CLI"
                    .into(),
            ));
        }
        let options = UpgradeExecuteOptions {
            dry_run: true,
            ..Default::default()
        };
        let report = execute_module_upgrade(
            &self.release_set,
            &self.release_set_path,
            &module_id,
            target_revision.as_deref(),
            &options,
        )
        .map_err(map_catalog_error)?;
        Ok(json!({
            "module_id": module_id,
            "dry_run": true,
            "report": report,
            "cli_command": report.cli_command,
            "note": "Web Console 仅预览步骤；复制 CLI 命令在终端执行真实升级。"
        }))
    }
}

fn filter_modules(
    modules: Vec<mutsuki_plugin_catalog::ModuleUpgradeSummary>,
    query: Option<&str>,
) -> Vec<mutsuki_plugin_catalog::ModuleUpgradeSummary> {
    match query.map(|needle| needle.to_ascii_lowercase()) {
        None => modules,
        Some(needle) => modules
            .into_iter()
            .filter(|module| {
                module.id.to_ascii_lowercase().contains(&needle)
                    || module.url.to_ascii_lowercase().contains(&needle)
            })
            .collect(),
    }
}

fn run_remote_check(
    release_set: ReleaseSetInfo,
    remote: Arc<dyn RemoteHeadProvider>,
) -> Result<Vec<mutsuki_plugin_catalog::ModuleUpgradeSummary>, ExtensionError> {
    std::thread::spawn(move || check_module_updates(&release_set, remote.as_ref()))
        .join()
        .map_err(|_| ExtensionError::Registration("remote check thread panicked".into()))?
        .map_err(map_catalog_error)
}

fn required_str(params: &JsonValue, key: &str) -> Result<String, ExtensionError> {
    params
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| ExtensionError::Registration(format!("missing {key}")))
}

fn map_catalog_error(err: mutsuki_plugin_catalog::CatalogError) -> ExtensionError {
    ExtensionError::Registration(err.to_string())
}

pub fn default_release_set_path(repo_root: &Path) -> PathBuf {
    repo_root.join("releases").join("mutsuki-0.1-alpha-3.toml")
}
