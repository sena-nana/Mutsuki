use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use mutsuki_bot_service_host_integration::{
    AgentConnectionConsoleBridge, BilibiliConsoleBridge, BotFlowConsoleBridge, QqConsoleBridge,
};
use mutsuki_bot_web_console::{
    BotAgentConsoleServices, ConsoleAssetDirs, SecretKeyResolver, SecretMonitor, WebConsoleConfig,
    WebConsolePaths, WebConsoleSecrets, attach_revision_changed_bridge,
    build_console_host_with_agent,
};
use mutsuki_service_config::ServiceConfig;
use mutsuki_service_runtime::ServiceRuntime;
use mutsuki_web_host::{MutsukiWebHost, WebHost, WebHostResult};

#[derive(Debug, thiserror::Error)]
pub enum WebConsoleError {
    #[error("{code}: {message}")]
    Config { code: &'static str, message: String },
    #[error(transparent)]
    WebHost(#[from] mutsuki_web_host::WebHostError),
}

/// Keeps the embedded Web Console alive for the ServiceRuntime lifetime.
pub struct WebConsoleGuard {
    host: MutsukiWebHost,
    _assets: ConsoleAssetDirs,
}

impl WebConsoleGuard {
    pub async fn start(
        config: WebConsoleConfig,
        product_root: &Path,
        service: &ServiceConfig,
        runtime: &ServiceRuntime,
        config_service: Arc<mutsuki_config_service::ConfigService>,
    ) -> Result<Option<Self>, WebConsoleError> {
        if !config.enabled {
            return Ok(None);
        }
        let secrets = resolve_secrets(service, &config)?;
        let secret_monitor = build_secret_monitor(service, &config);
        let (host, assets) = build_console_host_with_agent(
            &config,
            &secrets,
            runtime.control_handler(),
            runtime.control_token(),
            Some(config_service.clone()),
            secret_monitor,
            &WebConsolePaths::resolve(product_root, &config),
            BilibiliConsoleBridge::get(runtime),
            QqConsoleBridge::get(runtime),
            BotAgentConsoleServices {
                connections: AgentConnectionConsoleBridge::get(runtime),
                flow: BotFlowConsoleBridge::get(runtime),
            },
        )?;
        let mut host = host;
        host.start().await?;
        attach_revision_changed_bridge(&host, &config_service);
        Ok(Some(Self {
            host,
            _assets: assets,
        }))
    }

    pub fn listen_addr(&self) -> Option<std::net::SocketAddr> {
        self.host.listen_addr()
    }

    pub async fn stop(mut self) -> WebHostResult<()> {
        self.host.stop().await
    }
}

fn resolve_secrets(
    service: &ServiceConfig,
    config: &WebConsoleConfig,
) -> Result<WebConsoleSecrets, WebConsoleError> {
    let key = config
        .auth_token_key
        .as_deref()
        .ok_or_else(|| WebConsoleError::Config {
            code: "web.console.auth_token_key_required",
            message: "enabled web console requires web.console.auth_token_key".into(),
        })?;
    let store = service.host_secret_store();
    let auth_token = store.resolve(key).ok_or_else(|| WebConsoleError::Config {
        code: "web.console.auth_token_missing",
        message: format!("secret key `{key}` is not configured"),
    })?;
    if auth_token.is_empty() {
        return Err(WebConsoleError::Config {
            code: "web.console.auth_token_empty",
            message: format!("secret key `{key}` must not be empty"),
        });
    }
    Ok(WebConsoleSecrets { auth_token })
}

struct HostSecretResolver {
    store: mutsuki_service_config::HostSecretStore,
}

impl SecretKeyResolver for HostSecretResolver {
    fn resolve(&self, key: &str) -> Option<String> {
        self.store.resolve(key)
    }
}

fn build_secret_monitor(
    service: &ServiceConfig,
    config: &WebConsoleConfig,
) -> Option<SecretMonitor> {
    let mut keys = BTreeSet::new();
    if let Some(key) = &config.auth_token_key {
        keys.insert(key.clone());
    }
    if keys.is_empty() {
        return None;
    }
    let store = service.host_secret_store();
    Some(SecretMonitor::new(
        keys.into_iter().collect(),
        Arc::new(HostSecretResolver { store }),
    ))
}
