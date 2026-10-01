use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use mutsuki_agent_service_host_integration::{
    AGENT_CONNECTIONS_PLUGIN_ID, LOCAL_AGENT_CONFIG_PROVIDER_ID,
};
use mutsuki_bot_flow::BOT_FLOW_CONFIG_PROVIDER_ID;
use mutsuki_bot_service_host_integration::{
    AgentConnectionConsoleBridge, BOT_COMMAND_PLUGIN_ID, BOT_INTERACTION_PLUGIN_ID,
    BilibiliConsoleBridge, BotDatabaseConsoleBridge, BotFlowConsoleBridge, LocalAgentConsoleBridge,
    QqConsoleBridge, SANDBOX_SERVICE_ID, SandboxConsoleBridge,
};
use mutsuki_bot_web_host_integration::{
    BotAgentConsoleServices, ConfigNavigationGroup, ConfigNavigationItem, ConsoleAssetDirs,
    ControlChangeBridge, ManagementChangeBridge, SecretKeyResolver, SecretMonitor,
    WebConsoleConfig, WebConsolePaths, WebConsoleSecrets, attach_control_changed_bridge,
    attach_management_changed_bridges, attach_revision_changed_bridge,
    build_console_host_with_agent,
};
use mutsuki_plugin_bot_adapter_qqbot::QQBOT_ADAPTER_PLUGIN_ID;
use mutsuki_plugin_bot_agent::BOT_AGENT_BRIDGE_PLUGIN_ID;
use mutsuki_plugin_bot_bilibili::PLUGIN_ID as BILIBILI_PLUGIN_ID;
use mutsuki_plugin_bot_bilibili_workshop::PLUGIN_ID as WORKSHOP_PLUGIN_ID;
use mutsuki_plugin_bot_event_router::BOT_FLOW_ROUTER_PLUGIN_ID;
use mutsuki_plugin_bot_mihuashi::PLUGIN_ID as MIHUASHI_PLUGIN_ID;
use mutsuki_service_config::ServiceConfig;
use mutsuki_service_runtime::ServiceRuntime;
use mutsuki_web_host::{MutsukiWebHost, WebHost, WebHostResult};

use crate::{LocalConsoleConfig, PRODUCT_CONFIG_PROVIDER_ID};

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
    _config_watch: mutsuki_config_service::ConfigWatchSubscription,
    _control_changes: ControlChangeBridge,
    _management_changes: ManagementChangeBridge,
    _assets: ConsoleAssetDirs,
}

impl WebConsoleGuard {
    pub async fn start(
        config: LocalConsoleConfig,
        product_root: &Path,
        service: &ServiceConfig,
        runtime: &ServiceRuntime,
        config_service: Arc<mutsuki_config_service::ConfigService>,
    ) -> Result<Option<Self>, WebConsoleError> {
        if !config.enabled {
            return Ok(None);
        }
        let workspace_enabled = config.extensions.iter().any(|extension| {
            matches!(
                extension.as_str(),
                "qq" | "agent" | "bot-flow-editor" | "sandbox"
            )
        });
        let mut config_provider_ids = vec![PRODUCT_CONFIG_PROVIDER_ID.into()];
        if workspace_enabled {
            config_provider_ids.extend([
                QQBOT_ADAPTER_PLUGIN_ID.into(),
                LOCAL_AGENT_CONFIG_PROVIDER_ID.into(),
                BOT_AGENT_BRIDGE_PLUGIN_ID.into(),
                BILIBILI_PLUGIN_ID.into(),
                WORKSHOP_PLUGIN_ID.into(),
                MIHUASHI_PLUGIN_ID.into(),
            ]);
        }
        let config = WebConsoleConfig {
            enabled: config.enabled,
            listen: config.listen,
            auth_token_key: config.auth_token_key,
            extensions: config.extensions,
            config_provider_ids,
            primary_config_provider_id: Some(PRODUCT_CONFIG_PROVIDER_ID.into()),
            config_navigation_groups: config_navigation_groups(),
            release_set: config.release_set,
        };
        let secrets = resolve_secrets(service, &config)?;
        let secret_monitor = build_secret_monitor(service, &config);
        let bilibili = BilibiliConsoleBridge::get(runtime);
        let qq = QqConsoleBridge::get(runtime);
        let sandbox = SandboxConsoleBridge::get(runtime);
        let (host, assets) = build_console_host_with_agent(
            &config,
            &secrets,
            runtime.control_handler(),
            runtime.control_token(),
            Some(config_service.clone()),
            secret_monitor,
            &WebConsolePaths::resolve(product_root, &config),
            bilibili.clone(),
            qq.clone(),
            sandbox.clone(),
            BotDatabaseConsoleBridge::get(runtime),
            BotAgentConsoleServices {
                connections: AgentConnectionConsoleBridge::get(runtime),
                sessions: LocalAgentConsoleBridge::get(runtime),
                flow: BotFlowConsoleBridge::get(runtime),
            },
        )?;
        let mut host = host;
        host.start().await?;
        let config_watch =
            attach_revision_changed_bridge(&host, &config_service).ok_or_else(|| {
                WebConsoleError::Config {
                    code: "web.console.bridge_unavailable",
                    message: "started Web Console has no event bridge".into(),
                }
            })?;
        let control_changes =
            attach_control_changed_bridge(&host, runtime.subscribe_control_changes()).ok_or_else(
                || WebConsoleError::Config {
                    code: "web.console.bridge_unavailable",
                    message: "started Web Console has no event bridge".into(),
                },
            )?;
        let management_changes = attach_management_changed_bridges(
            &host,
            qq.as_ref(),
            bilibili.as_ref(),
            sandbox.as_ref(),
        )
        .ok_or_else(|| WebConsoleError::Config {
            code: "web.console.bridge_unavailable",
            message: "started Web Console has no event bridge".into(),
        })?;
        Ok(Some(Self {
            host,
            _config_watch: config_watch,
            _control_changes: control_changes,
            _management_changes: management_changes,
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

fn config_navigation_groups() -> Vec<ConfigNavigationGroup> {
    let item = |provider_id: &str, label: &str| ConfigNavigationItem {
        provider_id: provider_id.into(),
        label: Some(label.into()),
    };
    vec![
        ConfigNavigationGroup {
            label: None,
            items: vec![item(PRODUCT_CONFIG_PROVIDER_ID, "工作区")],
        },
        ConfigNavigationGroup {
            label: Some("接入".into()),
            items: vec![
                item(QQBOT_ADAPTER_PLUGIN_ID, "QQ 登录"),
                item(MIHUASHI_PLUGIN_ID, "米画师"),
                item(BILIBILI_PLUGIN_ID, "B 站"),
                item(WORKSHOP_PLUGIN_ID, "B 站工房"),
            ],
        },
        ConfigNavigationGroup {
            label: Some("助手".into()),
            items: vec![
                item(LOCAL_AGENT_CONFIG_PROVIDER_ID, "模型"),
                item(AGENT_CONNECTIONS_PLUGIN_ID, "助手连接"),
                item(BOT_AGENT_BRIDGE_PLUGIN_ID, "回复"),
                item(BOT_FLOW_CONFIG_PROVIDER_ID, "流程编排"),
            ],
        },
        ConfigNavigationGroup {
            label: Some("服务".into()),
            items: vec![item(crate::SERVICE_CONFIG_PROVIDER_ID, "服务运行时")],
        },
        ConfigNavigationGroup {
            label: Some("扩展".into()),
            items: vec![
                item("overview", "总览"),
                item("qq-bot", "QQ 机器人"),
                item("bot-agent", "连接管理"),
                item("bilibili", "B 站管理"),
                item(BOT_FLOW_ROUTER_PLUGIN_ID, "流程编辑器"),
                item("control", "运行控制"),
                item("database", "数据库"),
                item(SANDBOX_SERVICE_ID, "沙盒"),
                item(BOT_COMMAND_PLUGIN_ID, "命令"),
                item(BOT_INTERACTION_PLUGIN_ID, "交互"),
                item("secret", "密钥"),
                item("upgrade", "升级"),
            ],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRODUCT_CONFIG_PROVIDER_IDS: &[&str] = &[
        PRODUCT_CONFIG_PROVIDER_ID,
        QQBOT_ADAPTER_PLUGIN_ID,
        MIHUASHI_PLUGIN_ID,
        BILIBILI_PLUGIN_ID,
        WORKSHOP_PLUGIN_ID,
        LOCAL_AGENT_CONFIG_PROVIDER_ID,
        AGENT_CONNECTIONS_PLUGIN_ID,
        BOT_AGENT_BRIDGE_PLUGIN_ID,
        BOT_FLOW_CONFIG_PROVIDER_ID,
        crate::SERVICE_CONFIG_PROVIDER_ID,
    ];
    const WEB_EXTENSION_PAGE_IDS: &[&str] = &[
        "overview",
        "qq-bot",
        "bot-agent",
        "bilibili",
        "control",
        "database",
        "secret",
        "upgrade",
    ];
    const SCHEMA_LESS_PLUGIN_IDS: &[&str] = &[
        BOT_FLOW_ROUTER_PLUGIN_ID,
        SANDBOX_SERVICE_ID,
        BOT_COMMAND_PLUGIN_ID,
        BOT_INTERACTION_PLUGIN_ID,
    ];

    #[test]
    fn non_extension_config_navigation_uses_product_providers() {
        for group in config_navigation_groups() {
            assert!(
                !group.items.is_empty(),
                "config navigation must not keep an empty group"
            );
            for item in group.items {
                if WEB_EXTENSION_PAGE_IDS.contains(&item.provider_id.as_str())
                    || SCHEMA_LESS_PLUGIN_IDS.contains(&item.provider_id.as_str())
                {
                    continue;
                }
                assert!(
                    PRODUCT_CONFIG_PROVIDER_IDS.contains(&item.provider_id.as_str()),
                    "config navigation provider `{}` is not a product ConfigProvider",
                    item.provider_id
                );
            }
        }
    }

    #[test]
    fn schema_less_plugin_navigation_uses_plugin_ids_and_labels() {
        let items: Vec<_> = config_navigation_groups()
            .into_iter()
            .flat_map(|group| group.items)
            .collect();
        let label = |id: &str| {
            items
                .iter()
                .find(|item| item.provider_id == id)
                .and_then(|item| item.label.as_deref())
        };
        assert_eq!(label(BOT_FLOW_ROUTER_PLUGIN_ID), Some("流程编辑器"));
        assert_eq!(label(SANDBOX_SERVICE_ID), Some("沙盒"));
        assert_eq!(label(BOT_COMMAND_PLUGIN_ID), Some("命令"));
        assert_eq!(label(BOT_INTERACTION_PLUGIN_ID), Some("交互"));
        assert!(items.iter().all(|item| item.provider_id != "sandbox"));
        assert!(
            items
                .iter()
                .all(|item| item.provider_id != "bot-flow-editor")
        );
    }
}
