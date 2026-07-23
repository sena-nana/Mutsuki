use mutsuki_web_bridge::AuthPolicy;
use mutsuki_web_protocol::{DeploymentMode, ResourceBudgets};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListenConfig {
    pub host: String,
    pub port: u16,
}

impl ListenConfig {
    pub fn loopback(port: u16) -> Self {
        Self {
            host: "127.0.0.1".into(),
            port,
        }
    }

    pub fn parse(addr: &str) -> Self {
        if let Some((host, port)) = addr.rsplit_once(':') {
            let port = port.parse().unwrap_or(0);
            return Self {
                host: host.to_string(),
                port,
            };
        }
        Self {
            host: addr.to_string(),
            port: 0,
        }
    }

    pub fn socket_addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    pub fn is_loopback(&self) -> bool {
        matches!(self.host.as_str(), "127.0.0.1" | "::1" | "localhost")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TlsConfig {
    pub cert_path: String,
    pub key_path: String,
}

#[derive(Debug, Clone)]
pub struct WebHostConfig {
    pub listen: ListenConfig,
    pub mode: DeploymentMode,
    pub budgets: ResourceBudgets,
    pub safe_mode: bool,
    pub auth_token: Option<String>,
    pub remote_tokens: Vec<String>,
    pub tls: Option<TlsConfig>,
    pub link_endpoint: Option<String>,
}

impl WebHostConfig {
    pub fn auth_policy(&self) -> AuthPolicy {
        if self.listen.is_loopback() {
            if let Some(token) = &self.auth_token {
                return AuthPolicy::Local {
                    accepted_tokens: vec![token.clone()],
                    default_capabilities: vec![
                        "host.read".into(),
                        "recovery.read".into(),
                        "recovery.write".into(),
                        "runtime.read".into(),
                        "runtime.write".into(),
                        "config.schema.read".into(),
                        "config.value.read".into(),
                        "config.value.write".into(),
                        "config.secret.write".into(),
                        "config.apply".into(),
                    ],
                    allow_unauthenticated: false,
                };
            }
            return AuthPolicy::open_local();
        }

        let tokens = if self.remote_tokens.is_empty() {
            self.auth_token.clone().into_iter().collect::<Vec<_>>()
        } else {
            self.remote_tokens.clone()
        };
        AuthPolicy::Remote {
            accepted_tokens: tokens,
            default_capabilities: vec!["host.read".into(), "recovery.read".into()],
            require_tls: true,
            tls_enabled: self.tls.is_some(),
        }
    }
}
