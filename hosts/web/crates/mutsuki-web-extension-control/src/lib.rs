//! Control WebExtension: exposes typed ServiceHost [`ControlCommand`] operations as `control.*` RPC.
//!
//! Read-only methods require the `runtime.read` capability; mutating ops require
//! `runtime.write` from the WebHost-authenticated RPC context.
// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::doc_markdown,
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::too_many_lines
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mutsuki_service_control::{
    ControlCommand, ControlErrorBody, ControlErrorCode, ControlHandler, ControlMethod,
    ControlRequest, ControlResponse, ControlResult, CoreDrainResponse, EventSourceStatus,
    HealthReport, HostMetrics, IdParam, LogTailParams, LogTailResponse, PluginDeploymentClearParam,
    PluginDeploymentParam, PluginListResponse, PluginReloadResponse, RunnerStatus,
    RuntimeStatisticsView, ServiceStatus, TaskEventPage, TaskEventsAfterParam, TaskSnapshot,
    TaskSubmitBatchParam, TaskSubmitBatchResponse,
};
use mutsuki_web_extension_api::{
    BundledAssets, BundledManifest, ExtensionError, RpcRegistry, WebExtension,
    WebExtensionDescriptor, materialize_bundled_assets,
};
use mutsuki_web_protocol::{AssetEntry, ExtensionManifest, WebFrontendAssets};
use serde::Deserialize;
use serde_json::Value;

pub const PLUGIN_ID: &str = "control";
pub const PLUGIN_VERSION: &str = "0.1.0";
pub const CAPABILITY_RUNTIME_READ: &str = "runtime.read";
pub const CAPABILITY_RUNTIME_WRITE: &str = "runtime.write";

pub type ControlRpcResult<T> = Result<T, ControlRpcError>;

#[derive(Debug)]
pub enum ControlRpcError {
    Remote(ControlErrorBody),
    Protocol(String),
}

impl ControlRpcError {
    pub fn code(&self) -> Option<ControlErrorCode> {
        match self {
            Self::Remote(body) => Some(body.code),
            Self::Protocol(_) => None,
        }
    }
}

impl std::fmt::Display for ControlRpcError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Remote(body) => write!(formatter, "{}: {}", body.code, body.message),
            Self::Protocol(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for ControlRpcError {}

impl From<ControlRpcError> for ExtensionError {
    fn from(error: ControlRpcError) -> Self {
        match error {
            ControlRpcError::Remote(body) => Self::Rpc {
                code: body.code.as_str().into(),
                message: body.message,
            },
            ControlRpcError::Protocol(message) => Self::Registration(message),
        }
    }
}

macro_rules! control_result {
    ($result:expr, $variant:ident) => {
        match $result {
            ControlResult::$variant(value) => Ok(value),
            other => Err(unexpected_control_result(stringify!($variant), &other)),
        }
    };
}

macro_rules! unit_control_result {
    ($result:expr, $variant:ident) => {
        match $result {
            ControlResult::$variant => Ok(()),
            other => Err(unexpected_control_result(stringify!($variant), &other)),
        }
    };
}

/// Shared in-process caller used by `control` RPC handlers and aggregating extensions.
#[derive(Clone)]
pub struct ControlRpcCaller {
    control: Arc<dyn ControlHandler>,
    token: String,
}

impl ControlRpcCaller {
    pub fn new(control: Arc<dyn ControlHandler>, token: impl Into<String>) -> Self {
        Self {
            control,
            token: token.into(),
        }
    }

    pub async fn invoke(&self, command: ControlCommand) -> ControlRpcResult<ControlResult> {
        unwrap_control(
            self.control
                .handle(ControlRequest::new(self.token.clone(), command))
                .await,
        )
    }

    pub async fn health(&self) -> ControlRpcResult<HealthReport> {
        control_result!(self.invoke(ControlCommand::HealthCheck).await?, HealthCheck)
    }

    pub async fn service_status(&self) -> ControlRpcResult<ServiceStatus> {
        control_result!(
            self.invoke(ControlCommand::ServiceStatus).await?,
            ServiceStatus
        )
    }

    pub async fn plugin_list(&self) -> ControlRpcResult<PluginListResponse> {
        control_result!(self.invoke(ControlCommand::PluginList).await?, PluginList)
    }

    pub async fn runner_list(&self) -> ControlRpcResult<Vec<RunnerStatus>> {
        control_result!(self.invoke(ControlCommand::RunnerList).await?, RunnerList)
    }

    pub async fn event_source_list(&self) -> ControlRpcResult<Vec<EventSourceStatus>> {
        control_result!(
            self.invoke(ControlCommand::EventSourceList).await?,
            EventSourceList
        )
    }

    pub async fn runtime_statistics(&self) -> ControlRpcResult<RuntimeStatisticsView> {
        control_result!(
            self.invoke(ControlCommand::RuntimeStatistics).await?,
            RuntimeStatistics
        )
    }

    pub async fn host_metrics(&self) -> ControlRpcResult<HostMetrics> {
        control_result!(self.invoke(ControlCommand::HostMetrics).await?, HostMetrics)
    }

    pub async fn log_tail(&self, params: LogTailParams) -> ControlRpcResult<LogTailResponse> {
        control_result!(self.invoke(ControlCommand::LogTail(params)).await?, LogTail)
    }

    pub async fn task_list(&self) -> ControlRpcResult<Vec<TaskSnapshot>> {
        control_result!(self.invoke(ControlCommand::TaskList).await?, TaskList)
    }

    pub async fn task_events_after(
        &self,
        params: TaskEventsAfterParam,
    ) -> ControlRpcResult<TaskEventPage> {
        control_result!(
            self.invoke(ControlCommand::TaskEventsAfter(params)).await?,
            TaskEventsAfter
        )
    }

    pub async fn task_submit_batch(
        &self,
        params: TaskSubmitBatchParam,
    ) -> ControlRpcResult<TaskSubmitBatchResponse> {
        control_result!(
            self.invoke(ControlCommand::TaskSubmitBatch(params)).await?,
            TaskSubmitBatch
        )
    }

    pub async fn task_cancel(&self, params: IdParam) -> ControlRpcResult<()> {
        unit_control_result!(
            self.invoke(ControlCommand::TaskCancel(params)).await?,
            TaskCancel
        )
    }

    pub async fn core_begin_drain(&self) -> ControlRpcResult<CoreDrainResponse> {
        control_result!(
            self.invoke(ControlCommand::CoreBeginDrain).await?,
            CoreBeginDrain
        )
    }

    pub async fn plugin_reload(&self) -> ControlRpcResult<PluginReloadResponse> {
        control_result!(
            self.invoke(ControlCommand::PluginReload).await?,
            PluginReload
        )
    }

    pub async fn plugin_deployment_set(
        &self,
        params: PluginDeploymentParam,
    ) -> ControlRpcResult<PluginReloadResponse> {
        control_result!(
            self.invoke(ControlCommand::PluginDeploymentSet(params))
                .await?,
            PluginDeploymentSet
        )
    }

    pub async fn plugin_deployment_clear(
        &self,
        params: PluginDeploymentClearParam,
    ) -> ControlRpcResult<PluginReloadResponse> {
        control_result!(
            self.invoke(ControlCommand::PluginDeploymentClear(params))
                .await?,
            PluginDeploymentClear
        )
    }

    pub async fn runner_restart(&self, params: IdParam) -> ControlRpcResult<()> {
        unit_control_result!(
            self.invoke(ControlCommand::RunnerRestart(params)).await?,
            RunnerRestart
        )
    }

    pub async fn runner_stop(&self, params: IdParam) -> ControlRpcResult<()> {
        unit_control_result!(
            self.invoke(ControlCommand::RunnerStop(params)).await?,
            RunnerStop
        )
    }

    pub async fn event_source_restart(&self, params: IdParam) -> ControlRpcResult<()> {
        unit_control_result!(
            self.invoke(ControlCommand::EventSourceRestart(params))
                .await?,
            EventSourceRestart
        )
    }

    pub async fn service_shutdown(&self) -> ControlRpcResult<()> {
        unit_control_result!(
            self.invoke(ControlCommand::ServiceShutdown).await?,
            ServiceShutdown
        )
    }
}

pub struct ControlWebExtension {
    caller: ControlRpcCaller,
    assets_root: BundledAssets,
}

impl ControlWebExtension {
    pub fn new(caller: ControlRpcCaller) -> Self {
        Self {
            caller,
            assets_root: BundledAssets::default(),
        }
    }

    pub fn from_handler(control: Arc<dyn ControlHandler>, token: impl Into<String>) -> Self {
        Self::new(ControlRpcCaller::new(control, token))
    }

    #[must_use]
    pub fn with_frontend_assets(mut self, root: impl Into<PathBuf>) -> Self {
        self.assets_root.set(root);
        self
    }
}

impl WebExtension for ControlWebExtension {
    fn descriptor(&self) -> WebExtensionDescriptor {
        manifest(
            self.frontend_assets()
                .map(|assets| assets.manifest.assets)
                .unwrap_or_default(),
        )
    }

    fn frontend_assets(&self) -> Option<WebFrontendAssets> {
        self.assets_root.resolve(manifest)
    }

    fn register_rpc(&self, ctx: &mut RpcRegistry) -> Result<(), ExtensionError> {
        // Every control command is exposed under its wire name; reads need `runtime.read` and
        // mutating commands `runtime.write`, exactly as `ControlMethod::is_mutating` classifies
        // them. `health` stays as the console's historical alias of `health_check`.
        let methods = ControlMethod::ALL
            .into_iter()
            .map(|method| (method_name(method), method))
            .chain(std::iter::once((
                "health".to_owned(),
                ControlMethod::HealthCheck,
            )));
        for (name, method) in methods {
            let caller = self.caller.clone();
            let capability = if method.is_mutating() {
                CAPABILITY_RUNTIME_WRITE
            } else {
                CAPABILITY_RUNTIME_READ
            };
            ctx.register_async_contextual(&name, move |context, params| {
                let caller = caller.clone();
                async move {
                    context.require(capability)?;
                    let command = decode_control_command(method, &params)?;
                    encode_control_result(caller.invoke(command).await?)
                }
            });
        }
        Ok(())
    }

    fn register_events(
        &self,
        ctx: &mut mutsuki_web_extension_api::EventRegistry,
    ) -> Result<(), ExtensionError> {
        ctx.register_topic("changed");
        Ok(())
    }
}

fn method_name(method: ControlMethod) -> String {
    match serde_json::to_value(method) {
        Ok(Value::String(name)) => name,
        _ => unreachable!("ControlMethod serializes as its snake_case name"),
    }
}

/// Builds the typed command for `method` from RPC params. Commands without parameters ignore
/// whatever the console sends, matching their unit variants.
fn decode_control_command(
    method: ControlMethod,
    params: &Value,
) -> Result<ControlCommand, ExtensionError> {
    let operation = Value::String(method_name(method));
    let with_params = serde_json::json!({ "operation": operation, "request": params });
    match ControlCommand::deserialize(&with_params) {
        Ok(command) => Ok(command),
        Err(error) => ControlCommand::deserialize(&serde_json::json!({ "operation": operation }))
            .map_err(|_| ExtensionError::Registration(format!("invalid control request: {error}"))),
    }
}

/// The console receives the result body itself; unit results become `null`.
fn encode_control_result(result: ControlResult) -> Result<Value, ExtensionError> {
    let mut envelope = serde_json::to_value(result).map_err(|error| {
        ExtensionError::Registration(format!("encode control response: {error}"))
    })?;
    Ok(envelope
        .get_mut("response")
        .map_or(Value::Null, Value::take))
}

fn manifest(assets: Vec<AssetEntry>) -> ExtensionManifest {
    BundledManifest {
        id: PLUGIN_ID,
        version: PLUGIN_VERSION,
        entry: "index.js",
        capabilities: vec![
            CAPABILITY_RUNTIME_READ.into(),
            CAPABILITY_RUNTIME_WRITE.into(),
        ],
    }
    .build(assets)
}

pub fn materialize_frontend_assets(out_dir: &Path) -> Result<PathBuf, std::io::Error> {
    let js = include_str!("../assets/index.js");
    materialize_bundled_assets(out_dir, manifest, &[("index.js", js.as_bytes())])
}

fn unwrap_control(response: ControlResponse) -> ControlRpcResult<ControlResult> {
    match response {
        ControlResponse::Ok(result) => Ok(result),
        ControlResponse::Error(body) => Err(ControlRpcError::Remote(body)),
    }
}

fn unexpected_control_result(expected: &str, actual: &ControlResult) -> ControlRpcError {
    ControlRpcError::Protocol(format!(
        "control response type mismatch: expected {expected}, received {:?}",
        actual.method()
    ))
}

/// Scripted ControlHandler with hard-coded tokens for demos and tests. It is
/// never part of a production build: enable the `testkit` feature from a
/// dev-dependency or example to use it.
#[cfg(feature = "testkit")]
mod fixture;
#[cfg(feature = "testkit")]
pub use fixture::FixtureControlHandler;
