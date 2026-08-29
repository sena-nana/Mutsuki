//! Overview WebExtension: `overview.summary` aggregated via control-web caller.
// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::doc_markdown,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::return_self_not_must_use
)]

use std::path::{Path, PathBuf};

use mutsuki_plugin_bot_control_web::{CAPABILITY_RUNTIME_READ, ControlRpcCaller};
use mutsuki_service_control::ControlErrorCode;
use mutsuki_web_extension_api::{
    ExtensionError, RpcRegistry, WebExtension, WebExtensionDescriptor, content_hash,
    load_bundled_manifest,
};
use mutsuki_web_protocol::{
    AssetEntry, EXTENSION_MANIFEST_VERSION, ExtensionManifest, WEB_PROTOCOL_VERSION,
    WebFrontendAssets,
};
use serde_json::{Value, json};

pub use mutsuki_plugin_bot_control_web::FixtureControlHandler;

pub const PLUGIN_ID: &str = "overview";
pub const PLUGIN_VERSION: &str = "0.1.0";

#[derive(Clone)]
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

    async fn summary(&self) -> Result<Value, ExtensionError> {
        let service = self.control.service_status().await?;
        let health = self.control.health().await?;
        let tasks = match self.control.runtime_statistics().await {
            Ok(stats) => Some(stats.tasks),
            Err(err) if err.code() == Some(ControlErrorCode::CoreUnavailable) => None,
            Err(err) => return Err(err.into()),
        };
        let plugins = self.control.plugin_list().await?;
        let runners = self.control.runner_list().await?;

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
            "host": match self.control.host_metrics().await {
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
            manifest: load_bundled_manifest(root, manifest_for).ok()?,
            root_dir: root.clone(),
        })
    }

    fn register_rpc(&self, ctx: &mut RpcRegistry) -> Result<(), ExtensionError> {
        let this = OverviewWebExtension {
            control: self.control.clone(),
            assets_root: None,
        };
        ctx.register_async_contextual("summary", move |context, _params| {
            let this = this.clone();
            async move {
                context.require(CAPABILITY_RUNTIME_READ)?;
                this.summary().await
            }
        });
        Ok(())
    }

    fn register_events(
        &self,
        _ctx: &mut mutsuki_web_extension_api::EventRegistry,
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

pub fn materialize_frontend_assets(out_dir: &Path) -> Result<PathBuf, std::io::Error> {
    std::fs::create_dir_all(out_dir)?;
    let js = include_str!("../assets/index.js");
    let css = include_str!("../assets/mutsuki-ui.css");
    std::fs::write(out_dir.join("index.js"), js)?;
    std::fs::write(out_dir.join("mutsuki-ui.css"), css)?;
    let assets = [
        ("index.js", js.as_bytes()),
        ("mutsuki-ui.css", css.as_bytes()),
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
