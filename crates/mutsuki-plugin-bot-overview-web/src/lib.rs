//! Default Web overview plugin.
//!
//! Registers typed RPC under namespace `overview`:
//! - summary
//! - structure
//! - tasks.summary
//! - health
//!
//! Frontend assets provide the multi-page Console shell (overview + config).
//!
//! # Product assembly
//!
//! With an in-process [`mutsuki_service_runtime::ServiceRuntime`]:
//!
//! ```ignore
//! let extension = OverviewWebExtension::new(
//!     runtime.control_handler(),
//!     runtime.control_token(),
//! ).with_frontend_assets(assets_root);
//! ```
//!
//! The control token stays inside the extension; browser clients only speak overview.* RPC.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mutsuki_service_control::{
    ControlError, ControlHandler, ControlMethod, ControlRequest, ControlResponse, HealthReport,
    PluginListResponse, RunnerStatus, RuntimeStatisticsView, ServiceStatus,
};
use mutsuki_web_extension::{
    ExtensionError, RpcRegistry, WebExtension, WebExtensionDescriptor, content_hash,
};
use mutsuki_web_protocol::{
    AssetEntry, EXTENSION_MANIFEST_VERSION, ExtensionManifest, WEB_PROTOCOL_VERSION,
    WebFrontendAssets,
};
use serde_json::{Value, json};

pub const PLUGIN_ID: &str = "overview";
pub const PLUGIN_VERSION: &str = "0.1.0";
pub const CAPABILITY_RUNTIME_READ: &str = "runtime.read";

/// Backend WebExtension that fronts a ServiceHost [`ControlHandler`].
pub struct OverviewWebExtension {
    control: Arc<dyn ControlHandler>,
    token: String,
    assets_root: Option<PathBuf>,
}

impl OverviewWebExtension {
    pub fn new(control: Arc<dyn ControlHandler>, token: impl Into<String>) -> Self {
        Self {
            control,
            token: token.into(),
            assets_root: None,
        }
    }

    pub fn with_frontend_assets(mut self, root: impl Into<PathBuf>) -> Self {
        self.assets_root = Some(root.into());
        self
    }
}

impl WebExtension for OverviewWebExtension {
    fn descriptor(&self) -> WebExtensionDescriptor {
        ExtensionManifest {
            manifest_version: EXTENSION_MANIFEST_VERSION,
            id: PLUGIN_ID.into(),
            version: PLUGIN_VERSION.into(),
            entry: "index.js".into(),
            capabilities: vec![CAPABILITY_RUNTIME_READ.into()],
            permissions: vec!["pages".into(), "navigation".into()],
            assets: self
                .frontend_assets()
                .map(|assets| assets.manifest.assets)
                .unwrap_or_default(),
            protocol_version: WEB_PROTOCOL_VERSION.into(),
        }
    }

    fn frontend_assets(&self) -> Option<WebFrontendAssets> {
        let root = self.assets_root.as_ref()?;
        let manifest = load_or_synthesize_manifest(root).ok()?;
        Some(WebFrontendAssets {
            manifest,
            root_dir: root.clone(),
        })
    }

    fn register_rpc(&self, ctx: &mut RpcRegistry) -> Result<(), ExtensionError> {
        let this = OverviewHandle {
            control: self.control.clone(),
            token: self.token.clone(),
        };

        ctx.register("summary", {
            let this = this.clone();
            move |_params| this.summary()
        });
        ctx.register("structure", {
            let this = this.clone();
            move |_params| this.structure()
        });
        ctx.register("tasks.summary", {
            let this = this.clone();
            move |_params| this.tasks_summary()
        });
        ctx.register("health", {
            let this = this.clone();
            move |_params| this.health()
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
struct OverviewHandle {
    control: Arc<dyn ControlHandler>,
    token: String,
}

impl OverviewHandle {
    fn call(&self, method: ControlMethod) -> Result<Value, ExtensionError> {
        let control = self.control.clone();
        let token = self.token.clone();
        let response = futures_executor::block_on(async move {
            control
                .handle(ControlRequest {
                    token,
                    method,
                    params: Value::Null,
                })
                .await
        });
        unwrap_control(response)
    }

    fn summary(&self) -> Result<Value, ExtensionError> {
        let service: ServiceStatus =
            serde_json::from_value(self.call(ControlMethod::ServiceStatus)?)
                .map_err(|e| ExtensionError::Registration(e.to_string()))?;
        let health: HealthReport = serde_json::from_value(self.call(ControlMethod::HealthCheck)?)
            .map_err(|e| ExtensionError::Registration(e.to_string()))?;
        let tasks = match self.call(ControlMethod::RuntimeStatistics) {
            Ok(value) => {
                let stats: RuntimeStatisticsView = serde_json::from_value(value)
                    .map_err(|e| ExtensionError::Registration(e.to_string()))?;
                Some(stats.tasks)
            }
            Err(err) => {
                let message = err.to_string();
                if message.contains("core is not running") {
                    None
                } else {
                    return Err(err);
                }
            }
        };
        let plugins: PluginListResponse =
            serde_json::from_value(self.call(ControlMethod::PluginList)?)
                .map_err(|e| ExtensionError::Registration(e.to_string()))?;
        let runners: Vec<RunnerStatus> =
            serde_json::from_value(self.call(ControlMethod::RunnerList)?)
                .map_err(|e| ExtensionError::Registration(e.to_string()))?;
        let event_sources = health.event_source_details.clone();

        Ok(json!({
            "service": service,
            "health": {
                "service": health.service,
                "core": health.core,
                "plugins": health.plugins,
                "runners": health.runners,
                "event_sources": health.event_sources,
                "recent_errors": health.recent_errors,
                "component_ids": health.components.keys().cloned().collect::<Vec<_>>(),
            },
            "counts": {
                "plugins": service.plugin_count,
                "runners": service.runner_count,
                "event_sources": event_sources.len(),
                "configured_plugins": plugins.plugins.len(),
                "tasks": tasks,
            },
            "uptime_ms": service.uptime_ms,
            "event_sources": event_sources,
            "runners": runners,
        }))
    }

    fn structure(&self) -> Result<Value, ExtensionError> {
        let plugins = self.call(ControlMethod::PluginList)?;
        let runners = self.call(ControlMethod::RunnerList)?;
        let event_sources = self.call(ControlMethod::EventSourceList)?;
        let health: HealthReport = serde_json::from_value(self.call(ControlMethod::HealthCheck)?)
            .map_err(|e| ExtensionError::Registration(e.to_string()))?;
        Ok(json!({
            "plugins": plugins,
            "runners": runners,
            "event_sources": event_sources,
            "component_ids": health.components.keys().cloned().collect::<Vec<_>>(),
            "components": health.components,
        }))
    }

    fn tasks_summary(&self) -> Result<Value, ExtensionError> {
        let stats: RuntimeStatisticsView =
            serde_json::from_value(self.call(ControlMethod::RuntimeStatistics)?)
                .map_err(|e| ExtensionError::Registration(e.to_string()))?;
        Ok(serde_json::to_value(stats.tasks).unwrap_or_default())
    }

    fn health(&self) -> Result<Value, ExtensionError> {
        self.call(ControlMethod::HealthCheck)
    }
}

fn unwrap_control(response: ControlResponse) -> Result<Value, ExtensionError> {
    if response.ok {
        Ok(response.result.unwrap_or(Value::Null))
    } else {
        let body = response
            .error
            .unwrap_or(mutsuki_service_control::ControlErrorBody {
                code: "failed".into(),
                message: "control request failed".into(),
            });
        Err(ExtensionError::Registration(format!(
            "{}: {}",
            body.code, body.message
        )))
    }
}

fn load_or_synthesize_manifest(root: &Path) -> Result<ExtensionManifest, ExtensionError> {
    let manifest_path = root.join("manifest.json");
    if manifest_path.exists() {
        let bytes =
            std::fs::read(&manifest_path).map_err(|e| ExtensionError::Manifest(e.to_string()))?;
        return serde_json::from_slice(&bytes).map_err(|e| ExtensionError::Manifest(e.to_string()));
    }
    let entry = root.join("index.js");
    let bytes = std::fs::read(&entry).map_err(|e| ExtensionError::Manifest(e.to_string()))?;
    Ok(ExtensionManifest {
        manifest_version: EXTENSION_MANIFEST_VERSION,
        id: PLUGIN_ID.into(),
        version: PLUGIN_VERSION.into(),
        entry: "index.js".into(),
        capabilities: vec![CAPABILITY_RUNTIME_READ.into()],
        permissions: vec!["pages".into(), "navigation".into()],
        assets: vec![AssetEntry {
            path: "index.js".into(),
            content_hash: content_hash(&bytes),
            bytes: bytes.len() as u64,
        }],
        protocol_version: WEB_PROTOCOL_VERSION.into(),
    })
}

/// Write bundled frontend assets for the overview console shell.
pub fn materialize_frontend_assets(out_dir: &Path) -> Result<PathBuf, std::io::Error> {
    std::fs::create_dir_all(out_dir)?;
    let js = include_str!("../assets/index.js");
    let css = include_str!("../assets/lilia-tokens.css");
    let shell = include_str!("../assets/shell.html");
    std::fs::write(out_dir.join("index.js"), js)?;
    std::fs::write(out_dir.join("lilia-tokens.css"), css)?;
    std::fs::write(out_dir.join("shell.html"), shell)?;
    std::fs::write(out_dir.join("index.html"), shell)?;
    let entry_bytes = js.as_bytes();
    let css_bytes = css.as_bytes();
    let shell_bytes = shell.as_bytes();
    let manifest = ExtensionManifest {
        manifest_version: EXTENSION_MANIFEST_VERSION,
        id: PLUGIN_ID.into(),
        version: PLUGIN_VERSION.into(),
        entry: "index.js".into(),
        capabilities: vec![CAPABILITY_RUNTIME_READ.into()],
        permissions: vec!["pages".into(), "navigation".into()],
        assets: vec![
            AssetEntry {
                path: "index.js".into(),
                content_hash: content_hash(entry_bytes),
                bytes: entry_bytes.len() as u64,
            },
            AssetEntry {
                path: "lilia-tokens.css".into(),
                content_hash: content_hash(css_bytes),
                bytes: css_bytes.len() as u64,
            },
            AssetEntry {
                path: "shell.html".into(),
                content_hash: content_hash(shell_bytes),
                bytes: shell_bytes.len() as u64,
            },
        ],
        protocol_version: WEB_PROTOCOL_VERSION.into(),
    };
    std::fs::write(
        out_dir.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).expect("manifest"),
    )?;
    Ok(out_dir.to_path_buf())
}

/// In-memory control fixture for demos and Web E2E without a full ServiceRuntime.
pub struct FixtureControlHandler {
    pub service: ServiceStatus,
    pub health: HealthReport,
    pub plugins: PluginListResponse,
    pub runners: Vec<RunnerStatus>,
    pub statistics: RuntimeStatisticsView,
    pub fail_statistics: bool,
}

impl Default for FixtureControlHandler {
    fn default() -> Self {
        Self {
            service: ServiceStatus {
                instance_id: "demo".into(),
                profile: "dev".into(),
                uptime_ms: 12_345,
                ipc_endpoint: "local://demo".into(),
                core_running: true,
                plugin_count: 2,
                runner_count: 1,
            },
            health: HealthReport {
                service: "ok".into(),
                core: "ok".into(),
                plugins: "ok".into(),
                runners: "ok".into(),
                event_sources: "ok".into(),
                event_source_details: vec![mutsuki_service_control::EventSourceStatus {
                    source_id: "demo.source".into(),
                    plugin_id: "demo.plugin".into(),
                    instance_id: "demo-instance".into(),
                    state: "running".into(),
                    health: "healthy".into(),
                    last_error: None,
                    reconnects: 0,
                    last_event_unix_ms: Some(1_700_000_000_000),
                    started_at_unix_ms: Some(1_700_000_000_000 - 60_000),
                }],
                recent_errors: Vec::new(),
                components: [(
                    "mutsuki.bot.qqbot.gateway:demo".into(),
                    json!({
                        "status": "ok",
                        "connected": true,
                        "identified": true,
                        "started_at_unix_ms": 1_700_000_000_000u64 - 120_000,
                        "connected_since_unix_ms": 1_700_000_000_000u64 - 90_000,
                    }),
                )]
                .into_iter()
                .collect(),
            },
            plugins: PluginListResponse {
                plugins: vec![mutsuki_service_control::PluginStatus {
                    plugin_id: "demo.plugin".into(),
                    configured: true,
                    active_deployment: Some("builtin".into()),
                    preferred_deployment: Some("builtin".into()),
                    candidates: vec![mutsuki_service_control::PluginCandidateStatus {
                        deployment: "builtin".into(),
                        version: "0.1.0".into(),
                        api_version: "1".into(),
                        sha256: "abc".into(),
                        path: "builtin".into(),
                        available: true,
                        runner_link: None,
                    }],
                }],
                diagnostics: Vec::new(),
            },
            runners: vec![RunnerStatus {
                runner_id: "demo.runner".into(),
                plugin_id: "demo.plugin".into(),
                state: "running".into(),
                pid: Some(4242),
                restarts: 0,
                last_error: None,
            }],
            statistics: RuntimeStatisticsView {
                tasks: mutsuki_service_control::TaskPoolStatisticsView {
                    ready: 1,
                    running: 2,
                    waiting: 0,
                    blocked: 0,
                    completed: 10,
                    failed: 1,
                    cancelled: 0,
                    expired: 0,
                    dead_letter: 0,
                    submitted_total: 14,
                    attempts_started: 14,
                    cumulative_queue_steps: 0,
                    cumulative_execution_steps: 0,
                    stale_results_rejected: 0,
                    terminal_records_evicted: 0,
                },
                retained_events: 0,
                dropped_events: 0,
                retained_traces: 0,
                dropped_traces: 0,
                scheduler_decisions: 0,
            },
            fail_statistics: false,
        }
    }
}

impl ControlHandler for FixtureControlHandler {
    fn handle(&self, request: ControlRequest) -> mutsuki_service_control::ControlFuture {
        let token_ok = request.token == "local-dev" || request.token == "fixture";
        let method = request.method;
        let service = self.service.clone();
        let health = self.health.clone();
        let plugins = self.plugins.clone();
        let runners = self.runners.clone();
        let statistics = self.statistics.clone();
        let fail_statistics = self.fail_statistics;
        Box::pin(async move {
            if !token_ok {
                return ControlResponse::err(ControlError::Unauthorized);
            }
            match method {
                ControlMethod::ServiceStatus => ControlResponse::ok(service),
                ControlMethod::HealthCheck => ControlResponse::ok(health.clone()),
                ControlMethod::PluginList => ControlResponse::ok(plugins),
                ControlMethod::RunnerList => ControlResponse::ok(runners),
                ControlMethod::EventSourceList => {
                    ControlResponse::ok(health.event_source_details.clone())
                }
                ControlMethod::RuntimeStatistics => {
                    if fail_statistics {
                        ControlResponse::err(ControlError::Failed("core is not running".into()))
                    } else {
                        ControlResponse::ok(statistics)
                    }
                }
                other => ControlResponse::err(ControlError::Unsupported(format!("{other:?}"))),
            }
        })
    }
}
