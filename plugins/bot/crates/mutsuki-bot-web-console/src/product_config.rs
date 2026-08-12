//! Product configuration provider. Durable storage is supplied by product bootstrap.

use std::collections::BTreeMap;
use std::sync::Arc;

use mutsuki_agent_service_host_integration::{AGENT_CONNECTIONS_PLUGIN_ID, AgentConnectionsConfig};
use mutsuki_config_service::{
    ConfigApplyMode, ConfigConstraints, ConfigContext, ConfigDescriptor, ConfigError,
    ConfigLifecycle, ConfigMutability, ConfigNode, ConfigPersistSink, ConfigPersistTransaction,
    ConfigPresentation, ConfigProviderId, ConfigProviderRegistry, ConfigRepository, ConfigScope,
    ConfigSecretMutation, ConfigService, ConfigValue, ConfigValueType, LocalizedText,
    MapKeyStrategy, MemoryConfigProvider, RestartPolicy, capability,
};
use mutsuki_plugin_bot_adapter_qqbot::{
    QQ_CLIENT_SECRET_FIELD, QQ_CLIENT_SECRET_KEY, QQBOT_ADAPTER_PLUGIN_ID, QqBotConfig,
    qq_config_descriptor, qq_config_value,
};
use mutsuki_plugin_bot_agent::{
    BOT_AGENT_BRIDGE_PLUGIN_ID, BotAgentConfig, bot_agent_config_schema,
};
use mutsuki_plugin_bot_event_router::BOT_FLOW_ROUTER_PLUGIN_ID;
use mutsuki_service_config::{
    ConfiguredPluginSelection, HostSecretStore, PreparedHostSecretTransaction,
};

pub const PRODUCT_CONFIG_PROVIDER_ID: &str = "mutsuki.product";

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

    fn commit(&mut self) -> Result<(), ConfigError> {
        self.0.commit().map_err(persist_error)
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

pub fn product_config_service(
    repository: Arc<dyn ConfigRepository>,
) -> Result<Arc<ConfigService>, ProductConfigError> {
    product_config_service_with_options(ProductConfigOptions::new(repository))
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
        schema_version: 1,
        value_version: 1,
        title: LocalizedText::new("产品配置"),
        description: None,
        scopes: vec![ConfigScope::global()],
        root: ConfigNode {
            key: "product".into(),
            value_type: ConfigValueType::Object,
            title: LocalizedText::new("产品配置"),
            description: None,
            default_value: None,
            constraints: ConfigConstraints::default(),
            presentation: ConfigPresentation::default(),
            visibility: None,
            enabled_if: None,
            mutability: ConfigMutability::ReadWrite,
            restart_policy: RestartPolicy::None,
            children: vec![
                string_node("profile", "运行配置", RestartPolicy::ApplicationRestart),
                bool_node("console_enabled", "启用管理台", RestartPolicy::Reconfigure),
                string_node(
                    "console_listen",
                    "管理地址",
                    RestartPolicy::ApplicationRestart,
                ),
                string_node(
                    "auth_token_key",
                    "鉴权密钥引用",
                    RestartPolicy::ApplicationRestart,
                ),
                ConfigNode {
                    key: "extensions".into(),
                    value_type: ConfigValueType::Array {
                        item: Box::new(ConfigValueType::String { multiline: false }),
                    },
                    title: LocalizedText::new("管理扩展"),
                    description: None,
                    default_value: None,
                    constraints: ConfigConstraints::default(),
                    presentation: ConfigPresentation::default(),
                    visibility: None,
                    enabled_if: None,
                    mutability: ConfigMutability::ReadWrite,
                    restart_policy: RestartPolicy::ApplicationRestart,
                    children: Vec::new(),
                },
                ConfigNode {
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
                },
            ],
        },
        groups: Vec::new(),
    }
}

fn string_node(key: &str, title: &str, restart_policy: RestartPolicy) -> ConfigNode {
    ConfigNode {
        key: key.into(),
        value_type: ConfigValueType::String { multiline: false },
        title: LocalizedText::new(title),
        description: None,
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

fn bool_node(key: &str, title: &str, restart_policy: RestartPolicy) -> ConfigNode {
    ConfigNode {
        key: key.into(),
        value_type: ConfigValueType::Bool,
        title: LocalizedText::new(title),
        description: None,
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

pub fn product_seed_defaults() -> ConfigValue {
    ConfigValue::Object(BTreeMap::from([
        ("profile".into(), ConfigValue::String("bot".into())),
        ("console_enabled".into(), ConfigValue::Bool(true)),
        (
            "console_listen".into(),
            ConfigValue::String("127.0.0.1:8787".into()),
        ),
        (
            "auth_token_key".into(),
            ConfigValue::String("mutsuki.web.console.token".into()),
        ),
        (
            "extensions".into(),
            ConfigValue::Array(vec![ConfigValue::String("config".into())]),
        ),
        (
            "runtime_plugins".into(),
            ConfigValue::Object(BTreeMap::new()),
        ),
    ]))
}

/// Known local-product selections. An empty repository enables none of them.
#[must_use]
pub fn product_runtime_selections() -> Vec<ConfiguredPluginSelection> {
    vec![
        ConfiguredPluginSelection {
            id: AGENT_CONNECTIONS_PLUGIN_ID.into(),
            enabled: false,
            config: serde_json::to_value(AgentConnectionsConfig::default())
                .expect("Agent connection defaults serialize"),
        },
        ConfiguredPluginSelection {
            id: BOT_FLOW_ROUTER_PLUGIN_ID.into(),
            enabled: false,
            config: serde_json::json!({}),
        },
        ConfiguredPluginSelection {
            id: QQBOT_ADAPTER_PLUGIN_ID.into(),
            enabled: false,
            config: serde_json::to_value(QqBotConfig::default()).expect("QQ defaults serialize"),
        },
        ConfiguredPluginSelection {
            id: BOT_AGENT_BRIDGE_PLUGIN_ID.into(),
            enabled: false,
            config: serde_json::to_value(BotAgentConfig::default())
                .expect("Bot Agent defaults serialize"),
        },
    ]
}

/// Preserves stored product selections while ensuring every v1 owner component is declared.
pub fn merge_required_product_selections(selections: &mut Vec<ConfiguredPluginSelection>) {
    for required in product_runtime_selections() {
        if !selections
            .iter()
            .any(|selection| selection.id == required.id)
        {
            selections.push(required);
        }
    }
}

/// Registers the product-facing owner providers after Host secrets have been loaded.
pub async fn register_configured_product_providers(
    service: &Arc<ConfigService>,
    selections: &[ConfiguredPluginSelection],
    secrets: HostSecretStore,
) -> Result<(), ProductConfigError> {
    let qq = selection_or_default(selections, QQBOT_ADAPTER_PLUGIN_ID);
    let qq_config = deserialize_or_default::<QqBotConfig>(&qq.config, "QQ Bot")?;
    let mut qq_provider = MemoryConfigProvider::new(
        qq_config_descriptor(QQBOT_ADAPTER_PLUGIN_ID),
        qq_config_value(qq.enabled, &qq_config),
        ConfigApplyMode::HotReload,
    )
    .with_persist(Arc::new(HostSecretPersist {
        store: secrets.clone(),
        key_by_field: BTreeMap::from([(
            QQ_CLIENT_SECRET_FIELD.into(),
            QQ_CLIENT_SECRET_KEY.into(),
        )]),
    }));
    if secrets.resolve(QQ_CLIENT_SECRET_KEY).is_some() {
        qq_provider = qq_provider.with_initial_secret(QQ_CLIENT_SECRET_FIELD, "configured".into());
    }
    service
        .registry()
        .register(Arc::new(qq_provider))
        .map_err(register_error)?;

    let bridge = selection_or_default(selections, BOT_AGENT_BRIDGE_PLUGIN_ID);
    let bridge_config = deserialize_or_default::<BotAgentConfig>(&bridge.config, "Bot Agent")?;
    service
        .registry()
        .register(Arc::new(MemoryConfigProvider::new(
            bot_agent_config_schema(),
            ConfigValue::from_json(
                &serde_json::to_value(bridge_config).expect("Bot Agent config serializes"),
            ),
            ConfigApplyMode::HotReload,
        )))
        .map_err(register_error)?;

    for provider_id in [QQBOT_ADAPTER_PLUGIN_ID, BOT_AGENT_BRIDGE_PLUGIN_ID] {
        service
            .restore(provider_id, provider_context(provider_id))
            .await
            .map_err(register_error)?;
    }
    Ok(())
}

/// Overlays persisted owner documents onto the runtime selections before initial startup.
pub async fn restore_configured_product_selections(
    service: &Arc<ConfigService>,
    selections: &mut Vec<ConfiguredPluginSelection>,
) -> Result<(), ProductConfigError> {
    for provider_id in [QQBOT_ADAPTER_PLUGIN_ID, BOT_AGENT_BRIDGE_PLUGIN_ID] {
        let snapshot = service
            .read(
                provider_id,
                provider_context(provider_id),
                &[capability::VALUE_READ.into()],
            )
            .await
            .map_err(register_error)?;
        let base = selections
            .iter()
            .find(|selection| selection.id == provider_id);
        let restored = configured_plugin_selection_from_value(provider_id, &snapshot.value, base)
            .map_err(register_error)?;
        if let Some(current) = selections
            .iter_mut()
            .find(|selection| selection.id == provider_id)
        {
            *current = restored;
        } else {
            selections.push(restored);
        }
    }
    Ok(())
}

pub fn configured_plugin_selection_from_value(
    provider_id: &str,
    value: &ConfigValue,
    base: Option<&ConfiguredPluginSelection>,
) -> Result<ConfiguredPluginSelection, ConfigError> {
    let json = value.to_json();
    let object = json.as_object().ok_or_else(|| ConfigError::ApplyRejected {
        reason: "配置必须是对象".into(),
    })?;
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
            let mut config: QqBotConfig = base
                .and_then(|selection| serde_json::from_value(selection.config.clone()).ok())
                .unwrap_or_default();
            config.account_id = if config.account_id.trim().is_empty() {
                "local".into()
            } else {
                config.account_id
            };
            config.app_id = string_field(object, "app_id")?;
            config.client_secret_key = QQ_CLIENT_SECRET_KEY.into();
            config.gateway_intents = unsigned_field(object, "gateway_intents")?;
            config.shard = [
                unsigned_field(object, "shard_index")?,
                unsigned_field(object, "shard_count")?,
            ];
            let enabled = bool_field(object, "enabled")?;
            if enabled {
                config
                    .validate()
                    .map_err(|error| ConfigError::ApplyRejected {
                        reason: error.to_string(),
                    })?;
            }
            Ok(ConfiguredPluginSelection {
                id: provider_id.into(),
                enabled,
                config: serde_json::to_value(config).expect("QQ config serializes"),
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
        _ => Err(ConfigError::ApplyRejected {
            reason: format!("未知产品配置 `{provider_id}`"),
        }),
    }
}

fn selection_or_default(
    selections: &[ConfiguredPluginSelection],
    plugin_id: &str,
) -> ConfiguredPluginSelection {
    selections
        .iter()
        .find(|selection| selection.id == plugin_id)
        .cloned()
        .or_else(|| {
            product_runtime_selections()
                .into_iter()
                .find(|selection| selection.id == plugin_id)
        })
        .expect("product owner selection exists")
}

fn deserialize_or_default<T>(
    value: &serde_json::Value,
    title: &str,
) -> Result<T, ProductConfigError>
where
    T: serde::de::DeserializeOwned + Default,
{
    if value.is_null() {
        Ok(T::default())
    } else {
        serde_json::from_value(value.clone())
            .map_err(|error| ProductConfigError::Register(format!("{title} 配置无效：{error}")))
    }
}

fn bool_field(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<bool, ConfigError> {
    object
        .get(field)
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| ConfigError::ApplyRejected {
            reason: format!("`{field}` 必须是布尔值"),
        })
}

fn string_field(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<String, ConfigError> {
    object
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ConfigError::ApplyRejected {
            reason: format!("`{field}` 必须是字符串"),
        })
}

fn unsigned_field<T>(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<T, ConfigError>
where
    T: TryFrom<u64>,
{
    object
        .get(field)
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| T::try_from(value).ok())
        .ok_or_else(|| ConfigError::ApplyRejected {
            reason: format!("`{field}` 超出允许范围"),
        })
}

fn persist_error(error: mutsuki_service_config::ConfigError) -> ConfigError {
    ConfigError::ApplyRejected {
        reason: error.to_string(),
    }
}

fn provider_context(provider_id: &str) -> ConfigContext {
    if provider_id == BOT_AGENT_BRIDGE_PLUGIN_ID {
        ConfigContext::plugin_instance("default")
    } else {
        ConfigContext::global()
    }
}

fn register_error(error: ConfigError) -> ProductConfigError {
    ProductConfigError::Register(error.to_string())
}
