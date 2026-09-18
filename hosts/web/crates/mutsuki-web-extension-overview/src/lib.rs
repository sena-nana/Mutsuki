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

use mutsuki_service_control::ControlErrorCode;
use mutsuki_web_extension_api::{
    BundledAssets, BundledManifest, ExtensionError, RpcRegistry, WebExtension,
    WebExtensionDescriptor, materialize_bundled_assets,
};
use mutsuki_web_extension_control::{CAPABILITY_RUNTIME_READ, ControlRpcCaller};
use mutsuki_web_protocol::{AssetEntry, ExtensionManifest, WebFrontendAssets};
use serde_json::{Value, json};

pub use mutsuki_web_extension_control::FixtureControlHandler;

pub const PLUGIN_ID: &str = "overview";
pub const PLUGIN_VERSION: &str = "0.1.0";

#[derive(Clone)]
pub struct OverviewWebExtension {
    control: ControlRpcCaller,
    assets_root: BundledAssets,
}

impl OverviewWebExtension {
    pub fn new(control: ControlRpcCaller) -> Self {
        Self {
            control,
            assets_root: BundledAssets::default(),
        }
    }

    pub fn with_frontend_assets(mut self, root: impl Into<PathBuf>) -> Self {
        self.assets_root.set(root);
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
        self.assets_root.resolve(manifest_for)
    }

    fn register_rpc(&self, ctx: &mut RpcRegistry) -> Result<(), ExtensionError> {
        let this = OverviewWebExtension {
            control: self.control.clone(),
            assets_root: BundledAssets::default(),
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
    BundledManifest {
        id: PLUGIN_ID,
        version: PLUGIN_VERSION,
        entry: "index.js",
        capabilities: vec![CAPABILITY_RUNTIME_READ.into()],
    }
    .build(assets)
}

pub fn materialize_frontend_assets(out_dir: &Path) -> Result<PathBuf, std::io::Error> {
    let js = include_str!("../assets/index.js");
    let css = include_str!("../assets/mutsuki-ui.css");
    materialize_bundled_assets(
        out_dir,
        manifest_for,
        &[
            ("index.js", js.as_bytes()),
            ("mutsuki-ui.css", css.as_bytes()),
        ],
    )
}
