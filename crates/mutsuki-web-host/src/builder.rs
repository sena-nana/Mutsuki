use std::path::PathBuf;
use std::sync::Arc;

use mutsuki_web_extension::WebExtension;
use mutsuki_web_protocol::{DEFAULT_BUDGETS, DeploymentMode, ResourceBudgets};

use crate::MutsukiWebHost;
use crate::application::WebApplication;
use crate::config::{ListenConfig, TlsConfig, WebHostConfig};
use crate::error::{WebHostError, WebHostResult};

/// Builder that does not expose concrete web framework types.
pub struct MutsukiWebHostBuilder {
    application: Option<Arc<dyn WebApplication>>,
    listen: ListenConfig,
    mode: DeploymentMode,
    budgets: ResourceBudgets,
    safe_mode: bool,
    extensions: Vec<Arc<dyn WebExtension>>,
    shell_dir: Option<PathBuf>,
    auth_token: Option<String>,
    remote_tokens: Vec<String>,
    tls: Option<TlsConfig>,
    link_endpoint: Option<String>,
}

impl Default for MutsukiWebHostBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl MutsukiWebHostBuilder {
    pub fn new() -> Self {
        Self {
            application: None,
            listen: ListenConfig::loopback(0),
            mode: DeploymentMode::Embedded,
            budgets: DEFAULT_BUDGETS,
            safe_mode: false,
            extensions: Vec::new(),
            shell_dir: None,
            auth_token: None,
            remote_tokens: Vec::new(),
            tls: None,
            link_endpoint: None,
        }
    }

    pub fn application(mut self, application: impl WebApplication + 'static) -> Self {
        self.application = Some(Arc::new(application));
        self
    }

    pub fn application_arc(mut self, application: Arc<dyn WebApplication>) -> Self {
        self.application = Some(application);
        self
    }

    pub fn listen(mut self, addr: impl AsRef<str>) -> Self {
        self.listen = ListenConfig::parse(addr.as_ref());
        self
    }

    pub fn mode(mut self, mode: DeploymentMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn budgets(mut self, budgets: ResourceBudgets) -> Self {
        self.budgets = budgets;
        self
    }

    pub fn safe_mode(mut self, enabled: bool) -> Self {
        self.safe_mode = enabled;
        self
    }

    pub fn extension(mut self, extension: impl WebExtension + 'static) -> Self {
        self.extensions.push(Arc::new(extension));
        self
    }

    pub fn shell_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.shell_dir = Some(path.into());
        self
    }

    pub fn auth_token(mut self, token: impl Into<String>) -> Self {
        self.auth_token = Some(token.into());
        self
    }

    pub fn remote_tokens(mut self, tokens: Vec<String>) -> Self {
        self.remote_tokens = tokens;
        self
    }

    pub fn tls(mut self, tls: TlsConfig) -> Self {
        self.tls = Some(tls);
        self
    }

    /// Standalone mode MutsukiLink / local IPC endpoint descriptor.
    pub fn link_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.link_endpoint = Some(endpoint.into());
        self
    }

    pub fn build(self) -> WebHostResult<MutsukiWebHost> {
        let application = self
            .application
            .ok_or_else(|| WebHostError::InvalidConfig("application is required".into()))?;

        if self.mode == DeploymentMode::Standalone {
            let endpoint = self.link_endpoint.as_deref().ok_or_else(|| {
                WebHostError::InvalidConfig("standalone mode requires link_endpoint".into())
            })?;
            crate::link::parse_link_endpoint(endpoint)?;
        }

        if !self.listen.is_loopback() && self.tls.is_none() && self.remote_tokens.is_empty() {
            return Err(WebHostError::InvalidConfig(
                "non-loopback listen requires TLS or explicit remote auth tokens".into(),
            ));
        }

        let config = WebHostConfig {
            listen: self.listen,
            mode: self.mode,
            budgets: self.budgets,
            safe_mode: self.safe_mode,
            auth_token: self.auth_token,
            remote_tokens: self.remote_tokens,
            tls: self.tls,
            link_endpoint: self.link_endpoint,
        };

        let shell_dir = self.shell_dir.unwrap_or_else(|| {
            std::env::temp_dir().join(format!("mutsuki-webhost-shell-{}", uuid::Uuid::new_v4()))
        });

        Ok(MutsukiWebHost::new(
            config,
            application,
            self.extensions,
            shell_dir,
        ))
    }
}
