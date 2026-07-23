use std::path::{Path, PathBuf};
use std::sync::Arc;

use mutsuki_bot_web_console::{
    ConsoleAssetDirs, SecretKeyResolver, SecretMonitor, WebConsoleConfig, WebConsolePaths,
    WebConsoleSecrets, build_console_host, empty_config_service,
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
        product_config_path: &Path,
        service: &ServiceConfig,
        runtime: &ServiceRuntime,
    ) -> Result<Option<Self>, WebConsoleError> {
        let config = load_web_console_config(product_config_path)?;
        if !config.enabled {
            return Ok(None);
        }
        let secrets = resolve_secrets(service, &config)?;
        let secret_monitor = build_secret_monitor(service, &config);
        let config_service = if config.include_config {
            Some(empty_config_service())
        } else {
            None
        };
        let (host, assets) = build_console_host(
            &config,
            &secrets,
            runtime.control_handler(),
            runtime.control_token(),
            config_service,
            secret_monitor,
            &WebConsolePaths::resolve(&product_root(product_config_path), &config),
        )?;
        let mut host = host;
        host.start().await?;
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

pub fn load_web_console_config(
    product_config_path: &Path,
) -> Result<WebConsoleConfig, WebConsoleError> {
    let product = load_product_toml(product_config_path)?;
    Ok(product
        .get("web")
        .and_then(|web| web.get("console"))
        .cloned()
        .map(toml::Value::try_into)
        .transpose()
        .map_err(|error| WebConsoleError::Config {
            code: "web.console.invalid",
            message: error.to_string(),
        })?
        .unwrap_or_default())
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
    let key = config.auth_token_key.as_ref()?;
    let store = service.host_secret_store();
    Some(SecretMonitor::new(
        vec![key.clone()],
        Arc::new(HostSecretResolver { store }),
    ))
}

fn product_root(product_config_path: &Path) -> PathBuf {
    let parent = product_config_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    if parent.file_name().is_some_and(|name| name == "config") {
        parent.parent().map(Path::to_path_buf).unwrap_or(parent)
    } else {
        parent
    }
}

fn load_product_toml(product_config_path: &Path) -> Result<toml::Value, WebConsoleError> {
    let text =
        std::fs::read_to_string(product_config_path).map_err(|error| WebConsoleError::Config {
            code: "web.console.product_config_unreadable",
            message: error.to_string(),
        })?;
    toml::from_str(&text).map_err(|error| WebConsoleError::Config {
        code: "web.console.product_config_invalid",
        message: error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn disabled_console_is_default() {
        let root = tempdir().unwrap();
        let path = root.path().join("product.toml");
        std::fs::write(&path, "[service]\nprofile = \"test\"\n").unwrap();
        let config = load_web_console_config(&path).unwrap();
        assert!(!config.enabled);
    }

    #[test]
    fn enabled_console_requires_auth_token_key() {
        let root = tempdir().unwrap();
        let path = root.path().join("product.toml");
        std::fs::write(
            &path,
            r#"
[service]
profile = "test"

[web.console]
enabled = true
"#,
        )
        .unwrap();
        let service =
            mutsuki_service_config::ServiceConfig::load(mutsuki_service_config::ConfigOverrides {
                config_file: Some(path.clone()),
                ..Default::default()
            })
            .unwrap();
        let config = load_web_console_config(&path).unwrap();
        assert!(resolve_secrets(&service, &config).is_err());
    }
}
