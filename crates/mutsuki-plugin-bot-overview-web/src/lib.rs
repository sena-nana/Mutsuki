//! Overview WebExtension: `overview.summary` over an in-process ControlHandler.

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

    fn call(&self, method: ControlMethod) -> Result<Value, ExtensionError> {
        let control = self.control.clone();
        let token = self.token.clone();
        unwrap_control(futures_executor::block_on(async move {
            control
                .handle(ControlRequest {
                    token,
                    method,
                    params: Value::Null,
                })
                .await
        }))
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
            Err(err) if err.to_string().contains("core is not running") => None,
            Err(err) => return Err(err),
        };
        let plugins: PluginListResponse =
            serde_json::from_value(self.call(ControlMethod::PluginList)?)
                .map_err(|e| ExtensionError::Registration(e.to_string()))?;
        let runners: Vec<RunnerStatus> =
            serde_json::from_value(self.call(ControlMethod::RunnerList)?)
                .map_err(|e| ExtensionError::Registration(e.to_string()))?;

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
            token: self.token.clone(),
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
    let css = include_str!("../assets/lilia-tokens.css");
    let shell = include_str!("../assets/shell.html");
    std::fs::write(out_dir.join("index.js"), js)?;
    std::fs::write(out_dir.join("lilia-tokens.css"), css)?;
    std::fs::write(out_dir.join("shell.html"), shell)?;
    std::fs::write(out_dir.join("index.html"), shell)?;
    let assets = [
        ("index.js", js.as_bytes()),
        ("lilia-tokens.css", css.as_bytes()),
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

/// Fixture ControlHandler for demos/tests.
pub struct FixtureControlHandler {
    pub fail_statistics: bool,
}

impl Default for FixtureControlHandler {
    fn default() -> Self {
        Self {
            fail_statistics: false,
        }
    }
}

impl ControlHandler for FixtureControlHandler {
    fn handle(&self, request: ControlRequest) -> mutsuki_service_control::ControlFuture {
        let ok = request.token == "local-dev" || request.token == "fixture";
        let method = request.method;
        let fail_statistics = self.fail_statistics;
        Box::pin(async move {
            if !ok {
                return ControlResponse::err(ControlError::Unauthorized);
            }
            match method {
                ControlMethod::ServiceStatus => ControlResponse::ok(ServiceStatus {
                    instance_id: "demo".into(),
                    profile: "dev".into(),
                    uptime_ms: 12_345,
                    ipc_endpoint: "local://demo".into(),
                    core_running: true,
                    plugin_count: 2,
                    runner_count: 1,
                }),
                ControlMethod::HealthCheck => ControlResponse::ok(HealthReport {
                    service: "ok".into(),
                    core: "ok".into(),
                    plugins: "ok".into(),
                    runners: "ok".into(),
                    event_sources: "ok".into(),
                    event_source_details: vec![mutsuki_service_control::EventSourceStatus {
                        source_id: "demo.source".into(),
                        plugin_id: "demo.plugin".into(),
                        instance_id: "demo".into(),
                        state: "running".into(),
                        health: "healthy".into(),
                        last_error: None,
                        reconnects: 0,
                        last_event_unix_ms: Some(1_700_000_000_000),
                        started_at_unix_ms: Some(1_699_999_940_000),
                    }],
                    recent_errors: Vec::new(),
                    components: [(
                        "mutsuki.bot.qqbot.gateway:demo".into(),
                        json!({
                            "status": "ok",
                            "started_at_unix_ms": 1_699_999_880_000u64,
                            "connected_since_unix_ms": 1_699_999_910_000u64,
                        }),
                    )]
                    .into_iter()
                    .collect(),
                }),
                ControlMethod::PluginList => ControlResponse::ok(PluginListResponse {
                    plugins: vec![mutsuki_service_control::PluginStatus {
                        plugin_id: "demo.plugin".into(),
                        configured: true,
                        active_deployment: Some("builtin".into()),
                        preferred_deployment: Some("builtin".into()),
                        candidates: vec![],
                    }],
                    diagnostics: Vec::new(),
                }),
                ControlMethod::RunnerList => ControlResponse::ok(vec![RunnerStatus {
                    runner_id: "demo.runner".into(),
                    plugin_id: "demo.plugin".into(),
                    state: "running".into(),
                    pid: Some(4242),
                    restarts: 0,
                    last_error: None,
                }]),
                ControlMethod::EventSourceList => {
                    ControlResponse::ok(Vec::<mutsuki_service_control::EventSourceStatus>::new())
                }
                ControlMethod::RuntimeStatistics => {
                    if fail_statistics {
                        ControlResponse::err(ControlError::Failed("core is not running".into()))
                    } else {
                        ControlResponse::ok(RuntimeStatisticsView {
                            tasks: mutsuki_service_control::TaskPoolStatisticsView {
                                ready: 1,
                                running: 2,
                                submitted_total: 14,
                                ..Default::default()
                            },
                            ..Default::default()
                        })
                    }
                }
                other => ControlResponse::err(ControlError::Unsupported(format!("{other:?}"))),
            }
        })
    }
}
