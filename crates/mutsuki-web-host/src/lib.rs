//! MutsukiWebHost — Web application host at the same layer as MutsukiTauriHost.
//!
//! Owns HTTP/WebSocket lifecycle, static asset hosting, WebApplication assembly,
//! extension loading, management bridge, and recovery shell.
//! Does **not** own Bot admin pages, databases, logs, metrics UIs, or Axum ABI.

mod application;
mod builder;
mod config;
mod error;
mod link;
mod server;

pub use application::{MinimalWebApplication, WebApplication};
pub use builder::MutsukiWebHostBuilder;
pub use config::{ListenConfig, TlsConfig, WebHostConfig};
pub use error::{WebHostError, WebHostResult};
pub use link::{LinkBridgeTarget, parse_link_endpoint};
pub use mutsuki_web_protocol::{
    DeploymentMode, WebApplicationDescriptor, WebHostStatus, WebHostStatusReport, WebShellAssets,
};

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use mutsuki_web_bridge::{AuthPolicy, WebBridge};
use mutsuki_web_extension::{ExtensionRegistry, WebExtension};
use mutsuki_web_protocol::{
    DEFAULT_BUDGETS, ExtensionFailure, ResourceBudgets, WEB_PROTOCOL_VERSION,
};
use mutsuki_web_recovery::RecoveryShell;
use parking_lot::RwLock;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::server::HostServer;

/// Host lifecycle trait without exposing concrete web framework types.
pub trait WebHost {
    fn start(&mut self) -> impl std::future::Future<Output = WebHostResult<()>> + Send;
    fn stop(&mut self) -> impl std::future::Future<Output = WebHostResult<()>> + Send;
    fn status(&self) -> WebHostStatusReport;
}

/// Concrete WebHost implementation.
pub struct MutsukiWebHost {
    config: WebHostConfig,
    application: Arc<dyn WebApplication>,
    extensions: Vec<Arc<dyn WebExtension>>,
    bridge: Option<WebBridge>,
    status: RwLock<WebHostStatusReport>,
    cancel: CancellationToken,
    stop_tx: Option<oneshot::Sender<()>>,
    join: Option<tokio::task::JoinHandle<WebHostResult<()>>>,
    listen_addr: Option<SocketAddr>,
    shell_dir: PathBuf,
    started_at: Option<Instant>,
}

impl MutsukiWebHost {
    pub fn builder() -> MutsukiWebHostBuilder {
        MutsukiWebHostBuilder::new()
    }

    pub(crate) fn new(
        config: WebHostConfig,
        application: Arc<dyn WebApplication>,
        extensions: Vec<Arc<dyn WebExtension>>,
        shell_dir: PathBuf,
    ) -> Self {
        let report = WebHostStatusReport {
            status: WebHostStatus::Created,
            healthy: false,
            mode: config.mode,
            listen_addr: None,
            application_id: Some(application.descriptor().id),
            protocol_version: WEB_PROTOCOL_VERSION.to_string(),
            active_connections: 0,
            active_sessions: 0,
            extensions_loaded: 0,
            extensions_failed: 0,
            safe_mode: config.safe_mode,
            error: None,
        };
        Self {
            config,
            application,
            extensions,
            bridge: None,
            status: RwLock::new(report),
            cancel: CancellationToken::new(),
            stop_tx: None,
            join: None,
            listen_addr: None,
            shell_dir,
            started_at: None,
        }
    }

    pub fn listen_addr(&self) -> Option<SocketAddr> {
        self.listen_addr
    }

    pub fn bridge(&self) -> Option<&WebBridge> {
        self.bridge.as_ref()
    }

    pub fn config(&self) -> &WebHostConfig {
        &self.config
    }

    pub fn startup_duration(&self) -> Option<std::time::Duration> {
        self.started_at.map(|started| started.elapsed())
    }

    fn set_status(&self, mutate: impl FnOnce(&mut WebHostStatusReport)) {
        let mut status = self.status.write();
        mutate(&mut status);
    }

    fn assemble_bridge(&self) -> WebHostResult<(WebBridge, usize, usize)> {
        let budgets = self.config.budgets;
        let mut registry = ExtensionRegistry::new(budgets);
        let mut loaded = 0usize;
        let mut failed = 0usize;
        let mut extension_capabilities = Vec::new();

        for extension in &self.extensions {
            match registry.load_extension(extension.as_ref()) {
                Ok(record) => {
                    loaded += 1;
                    extension_capabilities.extend(record.manifest.capabilities);
                }
                Err(err) => {
                    failed += 1;
                    let extension_id = extension.descriptor().id;
                    registry.mark_failed(
                        &extension_id.clone(),
                        ExtensionFailure {
                            extension_id,
                            stage: "register".into(),
                            message: err.to_string(),
                            code: Some("extension_register_failed".into()),
                        },
                    );
                }
            }
        }

        let mut extension_ctx = mutsuki_web_extension::WebExtensionContext::default();
        let mut service_ctx = mutsuki_web_extension::WebServiceContext::default();
        self.application.register_services(&mut service_ctx);
        self.application.register_extensions(&mut extension_ctx);
        for record in extension_ctx.records() {
            // Application-provided extension records are already materialized.
            let _ = record;
        }
        let _ = service_ctx;

        let auth = self.config.auth_policy(&extension_capabilities);
        let bridge = WebBridge::new(budgets, registry, auth, self.config.safe_mode);
        let report = bridge.extensions().report();
        bridge.set_extension_counts(report.loaded.len() as u64, report.failed.len() as u64);
        Ok((bridge, loaded, failed))
    }

    fn prepare_shell(&self) -> WebHostResult<mutsuki_web_protocol::WebShellAssets> {
        std::fs::create_dir_all(&self.shell_dir)
            .map_err(|err| WebHostError::Io(err.to_string()))?;
        if self.config.safe_mode {
            let recovery = RecoveryShell::from_embedded_dir(&self.shell_dir);
            return recovery
                .materialize(&self.shell_dir)
                .map_err(|err| WebHostError::Io(err.to_string()));
        }
        let shell = self.application.shell();
        if shell.root_dir.exists() {
            copy_dir_recursive(&shell.root_dir, &self.shell_dir)
                .map_err(|err| WebHostError::Io(err.to_string()))?;
            return Ok(mutsuki_web_protocol::WebShellAssets {
                root_dir: self.shell_dir.clone(),
                index_file: shell.index_file,
                import_map: shell.import_map,
            });
        }
        // Fallback to recovery shell when application shell assets are absent.
        let recovery = RecoveryShell::from_embedded_dir(&self.shell_dir);
        recovery
            .materialize(&self.shell_dir)
            .map_err(|err| WebHostError::Io(err.to_string()))
    }
}

impl WebHost for MutsukiWebHost {
    async fn start(&mut self) -> WebHostResult<()> {
        if matches!(
            self.status().status,
            WebHostStatus::Running | WebHostStatus::Starting
        ) {
            return Err(WebHostError::InvalidState("already started".into()));
        }

        self.set_status(|status| {
            status.status = WebHostStatus::Starting;
            status.error = None;
        });
        let started = Instant::now();

        let (bridge, loaded, failed) = self.assemble_bridge()?;
        let shell = self.prepare_shell()?;
        let server = HostServer::new(
            self.config.clone(),
            bridge.clone(),
            shell,
            self.cancel.child_token(),
        );

        let (ready_tx, ready_rx) = oneshot::channel();
        let (stop_tx, stop_rx) = oneshot::channel();
        let cancel = self.cancel.clone();
        let join = tokio::spawn(async move { server.serve(ready_tx, stop_rx, cancel).await });

        let addr = ready_rx
            .await
            .map_err(|_| WebHostError::StartFailed("server dropped before ready".into()))??;

        self.bridge = Some(bridge);
        self.listen_addr = Some(addr);
        self.stop_tx = Some(stop_tx);
        self.join = Some(join);
        self.started_at = Some(started);

        self.set_status(|status| {
            status.status = WebHostStatus::Running;
            status.healthy = failed == 0 || self.config.safe_mode;
            status.listen_addr = Some(addr.to_string());
            status.extensions_loaded = loaded as u64;
            status.extensions_failed = failed as u64;
            status.safe_mode = self.config.safe_mode;
        });
        Ok(())
    }

    async fn stop(&mut self) -> WebHostResult<()> {
        if self.status().status == WebHostStatus::Stopped {
            return Ok(());
        }
        self.set_status(|status| status.status = WebHostStatus::Draining);
        self.cancel.cancel();
        if let Some(stop_tx) = self.stop_tx.take() {
            let _ = stop_tx.send(());
        }
        if let Some(join) = self.join.take() {
            match join.await {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    self.set_status(|status| {
                        status.status = WebHostStatus::Failed;
                        status.healthy = false;
                        status.error = Some(err.to_string());
                    });
                    return Err(err);
                }
                Err(err) => {
                    let message = err.to_string();
                    self.set_status(|status| {
                        status.status = WebHostStatus::Failed;
                        status.healthy = false;
                        status.error = Some(message.clone());
                    });
                    return Err(WebHostError::StopFailed(message));
                }
            }
        }
        self.bridge = None;
        self.listen_addr = None;
        self.set_status(|status| {
            status.status = WebHostStatus::Stopped;
            status.healthy = false;
            status.listen_addr = None;
            status.active_connections = 0;
            status.active_sessions = 0;
        });
        Ok(())
    }

    fn status(&self) -> WebHostStatusReport {
        let mut report = self.status.read().clone();
        if let Some(bridge) = &self.bridge {
            report.active_sessions = bridge.active_sessions();
            let metrics = bridge.metrics();
            report.active_connections = metrics.web_host_connections;
            report.extensions_loaded = metrics.web_extension_loaded;
            report.extensions_failed = metrics.web_extension_failed;
            report.safe_mode = bridge.safe_mode();
        }
        report
    }
}

fn copy_dir_recursive(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    if from == to {
        return Ok(());
    }
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else if entry.path() != target {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

pub fn default_budgets() -> ResourceBudgets {
    DEFAULT_BUDGETS
}

pub fn default_auth_policy() -> AuthPolicy {
    AuthPolicy::open_local()
}
