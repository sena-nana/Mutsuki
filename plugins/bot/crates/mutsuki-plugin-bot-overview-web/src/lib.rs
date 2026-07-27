//! Overview WebExtension: `overview.summary` aggregated via control-web caller.

use std::path::{Path, PathBuf};

use mutsuki_plugin_bot_control_web::{CAPABILITY_RUNTIME_READ, ControlRpcCaller};
use mutsuki_web_extension::{
    ExtensionError, RpcRegistry, WebExtension, WebExtensionDescriptor, content_hash,
};
use mutsuki_web_protocol::{
    AssetEntry, EXTENSION_MANIFEST_VERSION, ExtensionManifest, WEB_PROTOCOL_VERSION,
    WebFrontendAssets,
};
use serde_json::{Value, json};

pub use mutsuki_plugin_bot_control_web::FixtureControlHandler;

pub const PLUGIN_ID: &str = "overview";
pub const PLUGIN_VERSION: &str = "0.1.0";

pub struct OverviewWebExtension {
    control: ControlRpcCaller,
    assets_root: Option<PathBuf>,
}

impl OverviewWebExtension {
    pub fn new(control: ControlRpcCaller) -> Self {
        Self {
            control,
            assets_root: None,
        }
    }

    pub fn with_frontend_assets(mut self, root: impl Into<PathBuf>) -> Self {
        self.assets_root = Some(root.into());
        self
    }

    fn summary(&self) -> Result<Value, ExtensionError> {
        let service = self.control.service_status()?;
        let health = self.control.health()?;
        let tasks = match self.control.runtime_statistics() {
            Ok(stats) => Some(stats.tasks),
            Err(err) if err.to_string().contains("core is not running") => None,
            Err(err) => return Err(err),
        };
        let plugins = self.control.plugin_list()?;
        let runners = self.control.runner_list()?;

        Ok(json!({
            "service": service,
            "health": {
                "service": health.service,
                "core": health.core,
                "plugins": health.plugins,
                "runners": health.runners,
                "event_sources": health.event_sources,
                "recent_errors": health.recent_errors,
            },
            "counts": {
                "plugins": service.plugin_count,
                "runners": service.runner_count,
                "event_sources": health.event_source_details.len(),
                "tasks": tasks,
            },
            "uptime_ms": service.uptime_ms,
            "host": match self.control.host_metrics() {
                Ok(metrics) => json!({
                    "pid": metrics.pid,
                    "uptime_ms": metrics.uptime_ms,
                    "rss_bytes": metrics.rss_bytes,
                    "cpu_time_ms": metrics.cpu_time_ms,
                    "available": true,
                    "unavailable": false,
                }),
                Err(err) => json!({
                    "pid": null,
                    "uptime_ms": service.uptime_ms,
                    "rss_bytes": null,
                    "cpu_time_ms": null,
                    "available": false,
                    "unavailable": true,
                    "reason": format!("host_metrics unavailable: {err}"),
                }),
            },
            "plugins": plugins,
            "runners": runners,
            "event_sources": health.event_source_details,
            "components": health.components,
        }))
    }
}

impl WebExtension for OverviewWebExtension {
    fn descriptor(&self) -> WebExtensionDescriptor {
        manifest_for(
            self.frontend_assets()
                .map(|a| a.manifest.assets)
                .unwrap_or_default(),
        )
    }

    fn frontend_assets(&self) -> Option<WebFrontendAssets> {
        let root = self.assets_root.as_ref()?;
        Some(WebFrontendAssets {
            manifest: load_manifest(root).ok()?,
            root_dir: root.clone(),
        })
    }

    fn register_rpc(&self, ctx: &mut RpcRegistry) -> Result<(), ExtensionError> {
        let this = OverviewWebExtension {
            control: self.control.clone(),
            assets_root: None,
        };
        ctx.register("summary", move |_params| this.summary());
        Ok(())
    }

    fn register_events(
        &self,
        _ctx: &mut mutsuki_web_extension::EventRegistry,
    ) -> Result<(), ExtensionError> {
        Ok(())
    }
}

fn manifest_for(assets: Vec<AssetEntry>) -> ExtensionManifest {
    ExtensionManifest {
        manifest_version: EXTENSION_MANIFEST_VERSION,
        id: PLUGIN_ID.into(),
        version: PLUGIN_VERSION.into(),
        entry: "index.js".into(),
        capabilities: vec![CAPABILITY_RUNTIME_READ.into()],
        permissions: vec!["pages".into(), "navigation".into()],
        assets,
        protocol_version: WEB_PROTOCOL_VERSION.into(),
    }
}

fn load_manifest(root: &Path) -> Result<ExtensionManifest, ExtensionError> {
    let path = root.join("manifest.json");
    if path.exists() {
        let bytes = std::fs::read(&path).map_err(|e| ExtensionError::Manifest(e.to_string()))?;
        return serde_json::from_slice(&bytes).map_err(|e| ExtensionError::Manifest(e.to_string()));
    }
    let bytes = std::fs::read(root.join("index.js"))
        .map_err(|e| ExtensionError::Manifest(e.to_string()))?;
    Ok(manifest_for(vec![AssetEntry {
        path: "index.js".into(),
        content_hash: content_hash(&bytes),
        bytes: bytes.len() as u64,
    }]))
}

pub fn materialize_frontend_assets(out_dir: &Path) -> Result<PathBuf, std::io::Error> {
    std::fs::create_dir_all(out_dir)?;
    let js = include_str!("../assets/index.js");
    let bootstrap = include_str!("../assets/bootstrap.js");
    let css = include_str!("../assets/mutsuki-ui.css");
    let shell = include_str!("../assets/shell.html");
    std::fs::write(out_dir.join("index.js"), js)?;
    std::fs::write(out_dir.join("bootstrap.js"), bootstrap)?;
    std::fs::write(out_dir.join("mutsuki-ui.css"), css)?;
    std::fs::write(out_dir.join("shell.html"), shell)?;
    std::fs::write(out_dir.join("index.html"), shell)?;
    let assets = [
        ("index.js", js.as_bytes()),
        ("bootstrap.js", bootstrap.as_bytes()),
        ("mutsuki-ui.css", css.as_bytes()),
        ("shell.html", shell.as_bytes()),
    ]
    .into_iter()
    .map(|(path, bytes)| AssetEntry {
        path: path.into(),
        content_hash: content_hash(bytes),
        bytes: bytes.len() as u64,
    })
    .collect();
    std::fs::write(
        out_dir.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest_for(assets)).expect("manifest"),
    )?;
    Ok(out_dir.to_path_buf())
}
