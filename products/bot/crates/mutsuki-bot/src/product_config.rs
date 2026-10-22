//! Product configuration and owner-provider assembly for the local Bot application.

use std::collections::BTreeMap;
use std::sync::Arc;

use mutsuki_agent_service_host_integration::{
    AGENT_CONNECTIONS_PLUGIN_ID, AgentConnectionsConfig, LOCAL_AGENT_CONFIG_PROVIDER_ID,
    LOCAL_AGENT_PLUGIN_ID, local_agent_config_from_value,
};
use mutsuki_bot_service_host_integration::{
    BOT_COMMAND_PLUGIN_ID, BOT_INTERACTION_PLUGIN_ID, BOT_SANDBOX_PLUGIN_ID,
};
use mutsuki_config_service::document::{bool_field, document_object};
use mutsuki_config_service::{
    ConfigApplyMode, ConfigConstraints, ConfigDescriptor, ConfigError, ConfigExpr, ConfigLifecycle,
    ConfigMutability, ConfigNode, ConfigPersistSink, ConfigPersistTransaction, ConfigPresentation,
    ConfigProviderId, ConfigProviderRegistry, ConfigRepository, ConfigScope, ConfigSecretMutation,
    ConfigService, ConfigValue, ConfigValueType, LocalizedText, MapKeyStrategy,
    MemoryConfigProvider, RestartPolicy, capability,
};
use mutsuki_plugin_bot_adapter_qqbot::{QQBOT_ADAPTER_PLUGIN_ID, qq_config_from_value};
use mutsuki_plugin_bot_agent::{BOT_AGENT_BRIDGE_PLUGIN_ID, BotAgentConfig};
use mutsuki_plugin_bot_bilibili::{PLUGIN_ID as BILIBILI_PLUGIN_ID, bilibili_config_from_value};
use mutsuki_plugin_bot_bilibili_workshop::PLUGIN_ID as WORKSHOP_PLUGIN_ID;
use mutsuki_plugin_bot_event_router::BOT_FLOW_ROUTER_PLUGIN_ID;
use mutsuki_plugin_bot_mihuashi::PLUGIN_ID as MIHUASHI_PLUGIN_ID;
use mutsuki_plugin_io_http_client::{HttpClientConfig, PLUGIN_ID as HTTP_CLIENT_PLUGIN_ID};
use mutsuki_service_config::{
    ConfiguredPluginSelection, HostSecretStore, PreparedHostSecretTransaction,
};

pub const PRODUCT_CONFIG_PROVIDER_ID: &str = "mutsuki.product";

/// Schema and value version of the `mutsuki.product` document (v4: unified plugin ids).
pub const PRODUCT_CONFIG_VERSION: u32 = 4;

/// Resource provider the product binds to every media-consuming plugin. The
/// binding is owned by this assembly; owner documents no longer carry it.
pub const MEDIA_RESOURCE_PROVIDER_ID: &str = mutsuki_plugin_resource_sqlite::PROVIDER_ID;

#[derive(Debug, thiserror::Error)]
pub enum ProductConfigError {
    #[error("product config provider registration failed: {0}")]
    Register(String),
}

struct HostSecretPersist {
    store: HostSecretStore,
    key_by_field: BTreeMap<String, String>,
}

struct PreparedHostSecretPersist(PreparedHostSecretTransaction);

impl ConfigPersistTransaction for PreparedHostSecretPersist {
    fn activate(&mut self) -> Result<(), ConfigError> {
        self.0.activate().map_err(persist_error)
    }

    fn commit_marker(&self) -> Option<&std::path::Path> {
        self.0.commit_marker()
    }

    fn commit(&mut self) -> Result<(), ConfigError> {
        self.0.commit().map_err(persist_error)
    }

    fn finish(&mut self) -> Result<(), ConfigError> {
        self.0.finish().map_err(persist_error)
    }

    fn rollback(&mut self) -> Result<(), ConfigError> {
        self.0.rollback().map_err(persist_error)
    }
}

impl ConfigPersistSink for HostSecretPersist {
    fn prepare(
        &self,
        _persisted_value: &ConfigValue,
        secret_mutations: BTreeMap<String, ConfigSecretMutation>,
    ) -> Result<Box<dyn ConfigPersistTransaction>, ConfigError> {
        let updates = secret_mutations
            .into_iter()
            .filter_map(|(field, mutation)| {
                self.key_by_field.get(&field).cloned().map(|key| {
                    let value = match mutation {
                        ConfigSecretMutation::Set(value) => Some(value),
                        ConfigSecretMutation::Clear => None,
                    };
                    (key, value)
                })
            })
            .collect();
        self.store
            .prepare_mutations(updates)
            .and_then(|mut transaction| {
                transaction.enable_coordinated_commit()?;
                Ok(transaction)
            })
            .map(|transaction| {
                Box::new(PreparedHostSecretPersist(transaction))
                    as Box<dyn ConfigPersistTransaction>
            })
            .map_err(persist_error)
    }
}

pub struct ProductConfigOptions {
    pub repository: Arc<dyn ConfigRepository>,
    pub lifecycle: Option<Arc<dyn ConfigLifecycle>>,
}

impl ProductConfigOptions {
    #[must_use]
    pub fn new(repository: Arc<dyn ConfigRepository>) -> Self {
        Self {
            repository,
            lifecycle: None,
        }
    }

    #[must_use]
    pub fn with_lifecycle(mut self, lifecycle: Arc<dyn ConfigLifecycle>) -> Self {
        self.lifecycle = Some(lifecycle);
        self
    }
}

pub fn product_config_service_with_options(
    options: ProductConfigOptions,
) -> Result<Arc<ConfigService>, ProductConfigError> {
    let registry = Arc::new(ConfigProviderRegistry::default());
    registry
        .register(Arc::new(MemoryConfigProvider::new(
            product_descriptor(),
            product_seed_defaults(),
            ConfigApplyMode::HotReload,
        )))
        .map_err(register_error)?;
    let service = ConfigService::new(registry, options.repository)
        .map_err(|error| ProductConfigError::Register(error.to_string()))?;
    if let Some(lifecycle) = options.lifecycle {
        service.set_lifecycle(lifecycle);
    }
    Ok(Arc::new(service))
}

pub fn product_descriptor() -> ConfigDescriptor {
    ConfigDescriptor {
        provider_id: ConfigProviderId::new(PRODUCT_CONFIG_PROVIDER_ID),
        schema_version: PRODUCT_CONFIG_VERSION,
        value_version: PRODUCT_CONFIG_VERSION,
        title: LocalizedText::new("工作区"),
        description: None,
        scopes: vec![ConfigScope::global()],
        root: ConfigNode {
            key: "product".into(),
            value_type: ConfigValueType::Object,
            title: LocalizedText::new("工作区"),
            description: None,
            default_value: None,
            constraints: ConfigConstraints::default(),
            presentation: ConfigPresentation::default(),
            visibility: None,
            enabled_if: None,
            mutability: ConfigMutability::ReadWrite,
            restart_policy: RestartPolicy::None,
            children: vec![
                bool_node(
                    "workspace_enabled",
                    "启用",
                    Some("关闭后，助手连接和流程路由会一并停用。"),
                    RestartPolicy::ApplicationRestart,
                ),
                hidden(ConfigNode {
                    key: "runtime_plugins".into(),
                    value_type: ConfigValueType::Map {
                        key_strategy: MapKeyStrategy::FreeString,
                        value: Box::new(ConfigValueType::Object),
                    },
                    title: LocalizedText::new("运行插件"),
                    description: None,
                    default_value: None,
                    constraints: ConfigConstraints::default(),
                    presentation: ConfigPresentation::default(),
                    visibility: None,
                    enabled_if: None,
                    mutability: ConfigMutability::ReadWrite,
                    restart_policy: RestartPolicy::PluginReload,
                    children: Vec::new(),
                }),
            ],
        },
        groups: Vec::new(),
    }
}

fn bool_node(
    key: &str,
    title: &str,
    description: Option<&str>,
    restart_policy: RestartPolicy,
) -> ConfigNode {
    ConfigNode {
        key: key.into(),
        value_type: ConfigValueType::Bool,
        title: LocalizedText::new(title),
        description: description.map(LocalizedText::new),
        default_value: None,
        constraints: ConfigConstraints::default(),
        presentation: ConfigPresentation::default(),
        visibility: None,
        enabled_if: None,
        mutability: ConfigMutability::ReadWrite,
        restart_policy,
        children: Vec::new(),
    }
}

fn hidden(mut node: ConfigNode) -> ConfigNode {
    node.visibility = Some(ConfigExpr::Literal {
        value: ConfigValue::Bool(false),
    });
    node
}

pub fn product_seed_defaults() -> ConfigValue {
    ConfigValue::Object(BTreeMap::from([
        ("workspace_enabled".into(), ConfigValue::Bool(true)),
        (
            "runtime_plugins".into(),
            ConfigValue::Object(BTreeMap::new()),
        ),
    ]))
}

/// Builds an owner provider whose secret fields resolve through the Host
/// secret store under the given `(document field, store key)` pairs.
pub(crate) fn secret_backed_provider(
    descriptor: ConfigDescriptor,
    value: ConfigValue,
    secrets: &HostSecretStore,
    secret_keys: &[(&str, &str)],
) -> MemoryConfigProvider {
    let mut provider = MemoryConfigProvider::new(descriptor, value, ConfigApplyMode::HotReload)
        .with_persist(Arc::new(HostSecretPersist {
            store: secrets.clone(),
            key_by_field: secret_keys
                .iter()
                .map(|(field, key)| ((*field).to_owned(), (*key).to_owned()))
                .collect(),
        }));
    for (field, key) in secret_keys {
        if secrets.resolve(key).is_some() {
            provider = provider.with_initial_secret(*field, "configured".into());
        }
    }
    provider
}

/// Registers the product-facing owner providers after Host secrets have been loaded.
pub(crate) async fn register_configured_product_providers(
    service: &Arc<ConfigService>,
    secrets: HostSecretStore,
) -> Result<(), ProductConfigError> {
    let owners = crate::owners::product_owners();
    for owner in &owners {
        service
            .registry()
            .register((owner.provider)(&secrets))
            .map_err(register_error)?;
    }
    for owner in &owners {
        service
            .restore(owner.provider_id, (owner.context)())
            .await
            .map_err(register_error)?;
    }
    Ok(())
}

/// Resolves the owner documents that are authoritative for configurable product plugins.
pub(crate) async fn configured_product_owner_selections(
    service: &Arc<ConfigService>,
) -> Result<Vec<ConfiguredPluginSelection>, ProductConfigError> {
    let mut selections = Vec::new();
    for owner in crate::owners::product_owners() {
        let provider_id = owner.provider_id;
        let snapshot = service
            .read(
                provider_id,
                (owner.context)(),
                &[capability::VALUE_READ.into()],
            )
            .await
            .map_err(register_error)?;
        let restored = configured_plugin_selection_from_value(provider_id, &snapshot.value, None)
            .map_err(register_error)?;
        selections.push(restored);
    }
    Ok(selections)
}

pub(crate) fn configured_product_selections(
    product: &serde_json::Value,
    owner_selections: Vec<ConfiguredPluginSelection>,
) -> Result<Vec<ConfiguredPluginSelection>, ConfigError> {
    let workspace_enabled = product
        .get("workspace_enabled")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let mut selections = runtime_plugin_selections(product)?;
    selections.extend([
        ConfiguredPluginSelection {
            id: AGENT_CONNECTIONS_PLUGIN_ID.into(),
            enabled: workspace_enabled,
            config: serde_json::to_value(AgentConnectionsConfig::default())
                .expect("Agent connection defaults serialize"),
        },
        ConfiguredPluginSelection {
            id: BOT_FLOW_ROUTER_PLUGIN_ID.into(),
            enabled: workspace_enabled,
            config: serde_json::json!({}),
        },
        ConfiguredPluginSelection {
            id: BOT_SANDBOX_PLUGIN_ID.into(),
            enabled: workspace_enabled,
            config: serde_json::json!({}),
        },
        ConfiguredPluginSelection {
            id: BOT_COMMAND_PLUGIN_ID.into(),
            enabled: workspace_enabled,
            config: serde_json::json!({}),
        },
        ConfiguredPluginSelection {
            id: BOT_INTERACTION_PLUGIN_ID.into(),
            enabled: workspace_enabled,
            config: serde_json::json!({}),
        },
    ]);
    selections.push(http_client_selection());
    selections.extend(owner_selections);
    // The local engine rides on the workspace-provided Agent connections and
    // flow router, so it cannot outlive `workspace_enabled` even when its own
    // owner document says enabled.
    if !workspace_enabled {
        for selection in &mut selections {
            if selection.id == LOCAL_AGENT_PLUGIN_ID {
                selection.enabled = false;
            }
        }
    }
    Ok(selections)
}

/// The std HTTP client the built-in platform plugins fetch through. Like the media
/// provider it is a product binding: responses land in the media provider, the allowlist
/// is the union of the platform hosts, and only Bilibili may send and capture its login
/// cookie secret.
fn http_client_selection() -> ConfiguredPluginSelection {
    let owners = crate::owners::product_owners();
    let mut domain_allowlist = owners
        .iter()
        .flat_map(|owner| owner.http_domains.iter())
        .map(|&domain| domain.to_owned())
        .collect::<Vec<_>>();
    domain_allowlist.sort();
    domain_allowlist.dedup();
    let config = HttpClientConfig {
        response_provider_id: MEDIA_RESOURCE_PROVIDER_ID.into(),
        domain_allowlist,
        secret_grants: owners
            .iter()
            .flat_map(|owner| (owner.http_secret_grants)())
            .collect(),
        ..HttpClientConfig::default()
    };
    ConfiguredPluginSelection {
        id: HTTP_CLIENT_PLUGIN_ID.into(),
        enabled: true,
        config: serde_json::to_value(config).expect("HTTP client config serializes"),
    }
}

pub(crate) fn runtime_plugin_selections(
    product: &serde_json::Value,
) -> Result<Vec<ConfiguredPluginSelection>, ConfigError> {
    product
        .get("runtime_plugins")
        .and_then(serde_json::Value::as_object)
        .into_iter()
        .flatten()
        .map(|(id, value)| {
            if is_product_owner_plugin(id) {
                return Err(ConfigError::ApplyRejected {
                    reason: format!("`runtime_plugins` 不得配置 owner 插件 `{id}`"),
                });
            }
            Ok(ConfiguredPluginSelection {
                id: id.clone(),
                enabled: value
                    .get("enabled")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(true),
                config: value
                    .get("config")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({})),
            })
        })
        .collect()
}

pub(crate) use crate::owners::is_product_owner_plugin;

pub(crate) fn configured_plugin_selection_from_value(
    provider_id: &str,
    value: &ConfigValue,
    base: Option<&ConfiguredPluginSelection>,
) -> Result<ConfiguredPluginSelection, ConfigError> {
    let json = value.to_json();
    let object = document_object(&json)?;
    let current = base.map(|selection| &selection.config);
    match provider_id {
        AGENT_CONNECTIONS_PLUGIN_ID => {
            let config: AgentConnectionsConfig =
                serde_json::from_value(json).map_err(|error| ConfigError::ApplyRejected {
                    reason: error.to_string(),
                })?;
            config
                .validate()
                .map_err(|error| ConfigError::ApplyRejected {
                    reason: error.to_string(),
                })?;
            Ok(ConfiguredPluginSelection {
                id: provider_id.into(),
                enabled: base.is_some_and(|selection| selection.enabled),
                config: serde_json::to_value(config).expect("Agent connections serialize"),
            })
        }
        QQBOT_ADAPTER_PLUGIN_ID => {
            let (enabled, config) = qq_config_from_value(object, current)?;
            Ok(ConfiguredPluginSelection {
                id: provider_id.into(),
                enabled,
                config: serde_json::to_value(config).expect("QQ config serializes"),
            })
        }
        LOCAL_AGENT_CONFIG_PROVIDER_ID => {
            let (enabled, config) = local_agent_config_from_value(object, current)?;
            Ok(ConfiguredPluginSelection {
                id: provider_id.into(),
                enabled,
                config: serde_json::to_value(config).expect("Local Agent config serializes"),
            })
        }
        BOT_AGENT_BRIDGE_PLUGIN_ID => {
            let config: BotAgentConfig =
                serde_json::from_value(json).map_err(|error| ConfigError::ApplyRejected {
                    reason: error.to_string(),
                })?;
            config
                .validate()
                .map_err(|error| ConfigError::ApplyRejected {
                    reason: error.to_string(),
                })?;
            let enabled = config.enabled;
            Ok(ConfiguredPluginSelection {
                id: provider_id.into(),
                enabled,
                config: serde_json::to_value(config).expect("Bot Agent config serializes"),
            })
        }
        BILIBILI_PLUGIN_ID => {
            // The product assembly owns the media resource provider binding.
            let (enabled, config) =
                bilibili_config_from_value(object, current, MEDIA_RESOURCE_PROVIDER_ID)?;
            Ok(ConfiguredPluginSelection {
                id: provider_id.into(),
                enabled,
                config: serde_json::to_value(config).expect("Bilibili config serializes"),
            })
        }
        WORKSHOP_PLUGIN_ID | MIHUASHI_PLUGIN_ID => {
            let enabled = bool_field(object, "enabled")?;
            Ok(ConfiguredPluginSelection {
                id: provider_id.into(),
                enabled,
                // `enabled` already lives on the selection; the link-card
                // plugins take no configuration of their own.
                config: serde_json::Value::Null,
            })
        }
        _ => Err(ConfigError::ApplyRejected {
            reason: format!("未知产品配置 `{provider_id}`"),
        }),
    }
}

fn persist_error(error: mutsuki_service_config::ConfigError) -> ConfigError {
    ConfigError::ApplyRejected {
        reason: error.to_string(),
    }
}

fn register_error(error: ConfigError) -> ProductConfigError {
    ProductConfigError::Register(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_bot_service_host_integration::link_card_config_value;
    use mutsuki_plugin_bot_adapter_qqbot::QQ_RECEIVE_GUILD_FIELD;
    use mutsuki_plugin_bot_adapter_qqbot::{
        DEFAULT_QQBOT_INTENTS, QQ_INTENT_GROUP_AND_C2C, QQ_INTENT_PUBLIC_GUILD,
    };
    use mutsuki_plugin_bot_adapter_qqbot::{QqBotConfig, qq_config_value};
    use mutsuki_plugin_bot_bilibili::BilibiliBackendConfig;
    use mutsuki_plugin_bot_bilibili::{BILIBILI_COOKIE_KEY, BilibiliConfig, bilibili_config_value};
    use mutsuki_plugin_io_http_client::HttpSecretGrant;

    fn select(value: ConfigValue) -> ConfiguredPluginSelection {
        configured_plugin_selection_from_value(QQBOT_ADAPTER_PLUGIN_ID, &value, None).unwrap()
    }

    #[test]
    fn http_client_covers_platform_hosts_and_scopes_the_cookie_grant_to_bilibili() {
        let selections = configured_product_selections(
            &serde_json::json!({"workspace_enabled": false, "runtime_plugins": {}}),
            Vec::new(),
        )
        .unwrap();
        let selection = selections
            .iter()
            .find(|selection| selection.id == HTTP_CLIENT_PLUGIN_ID)
            .expect("the product always selects the HTTP client");
        assert!(selection.enabled);
        let config: HttpClientConfig = serde_json::from_value(selection.config.clone()).unwrap();
        config.validate().unwrap();
        assert_eq!(config.response_provider_id, MEDIA_RESOURCE_PROVIDER_ID);
        for host in [
            "b23.tv",
            "bilibili.com",
            "hdslb.com",
            "mall.bilibili.com",
            "mihuashi.com",
        ] {
            assert!(
                config.domain_allowlist.iter().any(|domain| domain == host),
                "{host} must be reachable"
            );
        }
        assert_eq!(
            config.secret_grants,
            vec![HttpSecretGrant {
                secret_key: "BILIBILI_COOKIE".into(),
                domains: vec!["bilibili.com".into()],
                capture: true,
            }]
        );
        assert!(is_product_owner_plugin(HTTP_CLIENT_PLUGIN_ID));
    }

    #[test]
    fn disabled_login_allows_empty_app_id() {
        let selected = select(qq_config_value(false, &QqBotConfig::default()));
        assert!(!selected.enabled);
        let config: QqBotConfig = serde_json::from_value(selected.config).unwrap();
        assert!(config.app_id.is_empty());
    }

    #[test]
    fn malformed_runtime_config_is_rejected_instead_of_reset() {
        let mut document = qq_config_value(false, &QqBotConfig::default()).to_json();
        document["runtime_config"] = serde_json::json!("not an owner config");
        let error = configured_plugin_selection_from_value(
            QQBOT_ADAPTER_PLUGIN_ID,
            &ConfigValue::from_json(&document),
            None,
        )
        .unwrap_err();
        assert!(matches!(error, ConfigError::ApplyRejected { .. }));
    }

    #[test]
    fn enabled_login_requires_app_id() {
        let error = configured_plugin_selection_from_value(
            QQBOT_ADAPTER_PLUGIN_ID,
            &qq_config_value(true, &QqBotConfig::default()),
            None,
        )
        .unwrap_err();
        assert!(error.to_string().contains("app_id"));
    }

    #[test]
    fn receive_switches_update_only_known_intent_bits() {
        let extra = 1 << 1;
        let mut config = QqBotConfig::new("local", "app");
        config.gateway_intents = DEFAULT_QQBOT_INTENTS | extra;
        let ConfigValue::Object(mut object) = qq_config_value(true, &config) else {
            panic!("QQ login document must be an object");
        };
        object.insert(QQ_RECEIVE_GUILD_FIELD.into(), ConfigValue::Bool(false));
        let selected = select(ConfigValue::Object(object));
        let restored: QqBotConfig = serde_json::from_value(selected.config).unwrap();
        assert_eq!(
            restored.gateway_intents & QQ_INTENT_PUBLIC_GUILD,
            0,
            "guild receive switch must clear only the guild bit"
        );
        assert_eq!(restored.gateway_intents & extra, extra);
        assert_eq!(
            restored.gateway_intents & QQ_INTENT_GROUP_AND_C2C,
            QQ_INTENT_GROUP_AND_C2C
        );
    }

    #[test]
    fn legacy_gateway_intents_are_still_accepted() {
        let mut config = QqBotConfig::new("local", "app");
        config.gateway_intents = QQ_INTENT_GROUP_AND_C2C;
        let value = ConfigValue::Object(
            [
                ("enabled".into(), ConfigValue::Bool(true)),
                ("app_id".into(), ConfigValue::String("app".into())),
                (
                    "gateway_intents".into(),
                    ConfigValue::Integer(i64::try_from(config.gateway_intents).unwrap()),
                ),
                (
                    "runtime_config".into(),
                    ConfigValue::from_json(&serde_json::to_value(&config).unwrap()),
                ),
            ]
            .into_iter()
            .collect(),
        );
        let selected = select(value);
        let restored: QqBotConfig = serde_json::from_value(selected.config).unwrap();
        assert_eq!(restored.gateway_intents, QQ_INTENT_GROUP_AND_C2C);
    }

    fn select_plugin(
        provider_id: &str,
        value: ConfigValue,
    ) -> Result<ConfiguredPluginSelection, ConfigError> {
        configured_plugin_selection_from_value(provider_id, &value, None)
    }

    #[test]
    fn bilibili_owner_document_stays_disabled_until_configured() {
        let selected = select_plugin(
            BILIBILI_PLUGIN_ID,
            bilibili_config_value(false, &BilibiliConfig::default()),
        )
        .unwrap();
        assert!(!selected.enabled);
        let config: BilibiliConfig = serde_json::from_value(selected.config).unwrap();
        assert_eq!(
            config.backend,
            BilibiliBackendConfig::WebCookie {
                cookie_secret_key: BILIBILI_COOKIE_KEY.into(),
            }
        );
    }

    #[test]
    fn enabled_bilibili_binds_product_media_provider_and_secret_keys() {
        let mut config = BilibiliConfig::default();
        config.management.enabled = true;
        config.management.admin_user_ids = vec!["admin".into()];
        config.management.self_binding_outbound_binding = "qq-main".into();
        let selected =
            select_plugin(BILIBILI_PLUGIN_ID, bilibili_config_value(true, &config)).unwrap();
        assert!(selected.enabled);
        let restored: BilibiliConfig = serde_json::from_value(selected.config).unwrap();
        assert_eq!(restored.media_provider_id, MEDIA_RESOURCE_PROVIDER_ID);
        assert!(restored.management.enabled);
        assert_eq!(restored.management.admin_user_ids, vec!["admin".to_owned()]);
        assert_eq!(restored.management.self_binding_outbound_binding, "qq-main");
        assert_eq!(
            restored.backend.cookie_secret_key(),
            Some(BILIBILI_COOKIE_KEY)
        );
    }

    #[test]
    fn link_card_owner_documents_only_carry_the_enable_switch() {
        for provider_id in [WORKSHOP_PLUGIN_ID, MIHUASHI_PLUGIN_ID] {
            let disabled = select_plugin(provider_id, link_card_config_value(false)).unwrap();
            assert!(!disabled.enabled);

            let enabled = select_plugin(provider_id, link_card_config_value(true)).unwrap();
            assert!(enabled.enabled);
            assert_eq!(enabled.config, serde_json::Value::Null);
        }
    }

    #[test]
    fn builtin_platform_plugins_are_rejected_from_runtime_plugins() {
        for plugin_id in [
            BILIBILI_PLUGIN_ID,
            WORKSHOP_PLUGIN_ID,
            MIHUASHI_PLUGIN_ID,
            BOT_COMMAND_PLUGIN_ID,
            BOT_INTERACTION_PLUGIN_ID,
        ] {
            let product = serde_json::json!({
                "runtime_plugins": {
                    plugin_id: { "enabled": true, "config": {} },
                },
            });
            let error = runtime_plugin_selections(&product).unwrap_err();
            assert!(
                matches!(&error, ConfigError::ApplyRejected { reason } if reason.contains(plugin_id)),
                "{plugin_id} must be rejected as an owner plugin: {error}"
            );
        }
    }
}
