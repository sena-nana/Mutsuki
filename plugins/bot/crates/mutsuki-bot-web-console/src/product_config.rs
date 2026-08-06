//! Product TOML ConfigProvider for Console `include_config` assembly.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;

use mutsuki_bot_config::{
    ConfigApplyMode, ConfigContext, ConfigDescriptor, ConfigError, ConfigLifecycle,
    ConfigMutability, ConfigNode, ConfigPersistSink, ConfigProviderId, ConfigProviderRegistry,
    ConfigScope, ConfigService, ConfigValue, ConfigValueType, LocalizedText, MemoryConfigProvider,
    MutsukiConfigSchema, PreparedConfigPersist, RestartPolicy,
};
use mutsuki_plugin_bot_agent::{
    BOT_AGENT_CONFIG_PROVIDER_ID, BotAgentConfig, BotAgentConfigHandle, bot_agent_config_schema,
};
use mutsuki_plugin_bot_command::{BOT_COMMAND_PLUGIN_ID, BotCommandConfig};
use mutsuki_service_config::{ConfiguredPluginStore, PreparedConfiguredPluginChange};

#[derive(Debug, thiserror::Error)]
pub enum ProductConfigError {
    #[error("product config unreadable: {0}")]
    Unreadable(String),
    #[error("product config invalid: {0}")]
    Invalid(String),
    #[error("product config provider registration failed: {0}")]
    Register(String),
    #[error("Bot Agent is selected in product config but no live Agent bridge is installed")]
    MissingBotAgentBridge,
}

#[derive(Default)]
pub struct ProductConfigOptions {
    pub store: Option<ConfiguredPluginStore>,
    pub lifecycle: Option<Arc<dyn ConfigLifecycle>>,
    pub bot_agent_config: Option<BotAgentConfigHandle>,
}

pub fn product_config_service(
    product_config_path: &Path,
) -> Result<Arc<ConfigService>, ProductConfigError> {
    product_config_service_with_options(product_config_path, ProductConfigOptions::default())
}

pub fn product_config_service_with_options(
    product_config_path: &Path,
    options: ProductConfigOptions,
) -> Result<Arc<ConfigService>, ProductConfigError> {
    let text = std::fs::read_to_string(product_config_path).map_err(|error| {
        ProductConfigError::Unreadable(format!("{}: {error}", product_config_path.display()))
    })?;
    let product: toml::Value =
        toml::from_str(&text).map_err(|error| ProductConfigError::Invalid(error.to_string()))?;
    let bot_agent_config = options.bot_agent_config;
    let store = options
        .store
        .unwrap_or_else(|| ConfiguredPluginStore::open(product_config_path));
    store
        .recover()
        .map_err(|error| ProductConfigError::Invalid(format!("config recovery: {error}")))?;
    let registry = Arc::new(ConfigProviderRegistry::default());

    let product_provider = Arc::new(
        MemoryConfigProvider::new(
            product_descriptor(),
            product_defaults(&product),
            ConfigApplyMode::RequireRestart,
        )
        .with_persist(Arc::new(ProductSurfacePersist {
            store: store.clone(),
        })),
    );
    registry
        .register(product_provider)
        .map_err(|error| ProductConfigError::Register(error.to_string()))?;

    if let Some(command_defaults) = command_defaults_from_product(&product) {
        let command_provider = Arc::new(
            MemoryConfigProvider::new(
                BotCommandConfig::schema(),
                command_defaults,
                ConfigApplyMode::HotReload,
            )
            .with_persist(Arc::new(ConfiguredPluginPersist {
                store: store.clone(),
                plugin_id: BOT_COMMAND_PLUGIN_ID.into(),
                bot_agent_config: None,
            })),
        );
        registry
            .register(command_provider)
            .map_err(|error| ProductConfigError::Register(error.to_string()))?;
    }

    if let Some(handle) = bot_agent_config {
        if let Some(defaults) = bot_agent_defaults_from_product(&product, &handle)? {
            let provider = Arc::new(
                MemoryConfigProvider::new(
                    bot_agent_config_schema(),
                    ConfigValue::from_json(
                        &serde_json::to_value(&defaults)
                            .map_err(|error| ProductConfigError::Invalid(error.to_string()))?,
                    ),
                    ConfigApplyMode::HotReload,
                )
                .with_persist(Arc::new(ConfiguredPluginPersist {
                    store: store.clone(),
                    plugin_id: BOT_AGENT_CONFIG_PROVIDER_ID.into(),
                    bot_agent_config: Some(handle),
                })),
            );
            registry
                .register(provider)
                .map_err(|error| ProductConfigError::Register(error.to_string()))?;
        }
    } else if plugin_selected(&product, BOT_AGENT_CONFIG_PROVIDER_ID) {
        return Err(ProductConfigError::MissingBotAgentBridge);
    }

    let mut service = ConfigService::new(registry);
    if let Some(lifecycle) = options.lifecycle {
        service = service.with_lifecycle(lifecycle);
    }
    Ok(Arc::new(service))
}

struct ProductSurfacePersist {
    store: ConfiguredPluginStore,
}

impl ConfigPersistSink for ProductSurfacePersist {
    fn prepare(
        &self,
        _context: &ConfigContext,
        value: &ConfigValue,
        _secrets: &HashMap<String, String>,
    ) -> Result<Box<dyn PreparedConfigPersist>, ConfigError> {
        let ConfigValue::Object(map) = value else {
            return Err(ConfigError::PersistenceFailed {
                reason: "product candidate must be an object".into(),
            });
        };
        let mut fields = BTreeMap::new();
        for key in [
            "profile",
            "console_enabled",
            "console_listen",
            "include_config",
        ] {
            if let Some(field) = map.get(key) {
                fields.insert(key.to_string(), field.to_json());
            }
        }
        let prepared = self
            .store
            .prepare_product_surface(fields)
            .map_err(|error| ConfigError::PersistenceFailed {
                reason: error.to_string(),
            })?;
        Ok(Box::new(ProductSurfaceChange { prepared }))
    }
}

struct ProductSurfaceChange {
    prepared: PreparedConfiguredPluginChange,
}

impl PreparedConfigPersist for ProductSurfaceChange {
    fn commit(&mut self) -> Result<(), ConfigError> {
        self.prepared
            .commit()
            .map_err(|error| ConfigError::PersistenceFailed {
                reason: error.to_string(),
            })
    }

    fn rollback(&mut self) -> Result<(), ConfigError> {
        self.prepared
            .rollback()
            .map_err(|error| ConfigError::PersistenceFailed {
                reason: error.to_string(),
            })
    }
}

struct ConfiguredPluginPersist {
    store: ConfiguredPluginStore,
    plugin_id: String,
    bot_agent_config: Option<BotAgentConfigHandle>,
}

impl ConfigPersistSink for ConfiguredPluginPersist {
    fn prepare(
        &self,
        _context: &ConfigContext,
        value: &ConfigValue,
        _secrets: &HashMap<String, String>,
    ) -> Result<Box<dyn PreparedConfigPersist>, ConfigError> {
        let json = value.to_json();
        if self.plugin_id == BOT_COMMAND_PLUGIN_ID {
            let decoded: BotCommandConfig =
                serde_json::from_value(json.clone()).map_err(|error| {
                    ConfigError::PersistenceFailed {
                        reason: format!("command config decode failed: {error}"),
                    }
                })?;
            decoded
                .validate()
                .map_err(|reason| ConfigError::ApplyRejected { reason })?;
        }
        let decoded_bot_agent = if self.plugin_id == BOT_AGENT_CONFIG_PROVIDER_ID {
            let decoded: BotAgentConfig =
                serde_json::from_value(json.clone()).map_err(|error| {
                    ConfigError::PersistenceFailed {
                        reason: format!("bot-agent config decode failed: {error}"),
                    }
                })?;
            decoded
                .validate()
                .map_err(|error| ConfigError::ApplyRejected {
                    reason: error.to_string(),
                })?;
            Some(decoded)
        } else {
            None
        };
        let prepared = self
            .store
            .prepare_replace_config(&self.plugin_id, json)
            .map_err(|error| ConfigError::PersistenceFailed {
                reason: error.to_string(),
            })?;
        let previous_bot_agent = self
            .bot_agent_config
            .as_ref()
            .map(BotAgentConfigHandle::snapshot);
        Ok(Box::new(ConfiguredPluginChange {
            prepared,
            bot_agent_config: self.bot_agent_config.clone(),
            previous_bot_agent,
            candidate_bot_agent: decoded_bot_agent,
        }))
    }
}

struct ConfiguredPluginChange {
    prepared: PreparedConfiguredPluginChange,
    bot_agent_config: Option<BotAgentConfigHandle>,
    previous_bot_agent: Option<BotAgentConfig>,
    candidate_bot_agent: Option<BotAgentConfig>,
}

impl PreparedConfigPersist for ConfiguredPluginChange {
    fn activate(&mut self) -> Result<(), ConfigError> {
        if let (Some(handle), Some(config)) = (&self.bot_agent_config, &self.candidate_bot_agent) {
            handle
                .replace(config.clone())
                .map_err(|error| ConfigError::PersistenceFailed {
                    reason: format!("candidate bot-agent config activation failed: {error}"),
                })?;
        }
        Ok(())
    }

    fn commit(&mut self) -> Result<(), ConfigError> {
        self.prepared
            .commit()
            .map_err(|error| ConfigError::PersistenceFailed {
                reason: error.to_string(),
            })
    }

    fn rollback(&mut self) -> Result<(), ConfigError> {
        let file_result =
            self.prepared
                .rollback()
                .map_err(|error| ConfigError::PersistenceFailed {
                    reason: error.to_string(),
                });
        let live_result = match (&self.bot_agent_config, &self.previous_bot_agent) {
            (Some(handle), Some(previous)) => {
                handle
                    .replace(previous.clone())
                    .map_err(|error| ConfigError::PersistenceFailed {
                        reason: format!("bot-agent config rollback failed: {error}"),
                    })
            }
            _ => Ok(()),
        };
        match (file_result, live_result) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(file), Ok(())) => Err(file),
            (Ok(()), Err(live)) => Err(live),
            (Err(file), Err(live)) => Err(ConfigError::PersistenceFailed {
                reason: format!("{file}; {live}"),
            }),
        }
    }
}

fn bot_agent_defaults_from_product(
    product: &toml::Value,
    handle: &BotAgentConfigHandle,
) -> Result<Option<BotAgentConfig>, ProductConfigError> {
    let Some(selection) = configured_plugin_selection(product, BOT_AGENT_CONFIG_PROVIDER_ID) else {
        return Ok(None);
    };
    let value = selection
        .get("config")
        .cloned()
        .unwrap_or_else(|| toml::Value::Table(toml::map::Map::new()));
    let json = serde_json::to_value(value)
        .map_err(|error| ProductConfigError::Invalid(error.to_string()))?;
    let config: BotAgentConfig = serde_json::from_value(json)
        .map_err(|error| ProductConfigError::Invalid(format!("bot-agent config: {error}")))?;
    config
        .validate()
        .map_err(|error| ProductConfigError::Invalid(format!("bot-agent config: {error}")))?;
    handle
        .replace(config.clone())
        .map_err(|error| ProductConfigError::Invalid(format!("bot-agent config: {error}")))?;
    Ok(Some(config))
}

fn plugin_selected(product: &toml::Value, plugin_id: &str) -> bool {
    configured_plugin_selection(product, plugin_id).is_some()
}

fn configured_plugin_selection<'a>(
    product: &'a toml::Value,
    plugin_id: &str,
) -> Option<&'a toml::Value> {
    product
        .get("plugins")
        .and_then(|plugins| plugins.get("configured"))
        .and_then(|value| value.as_array())
        .and_then(|configured| {
            configured.iter().find(|selection| {
                selection
                    .get("id")
                    .and_then(|value| value.as_str())
                    .is_some_and(|id| id.trim() == plugin_id)
                    && selection
                        .get("enabled")
                        .and_then(|value| value.as_bool())
                        .unwrap_or(true)
            })
        })
}

fn command_defaults_from_product(product: &toml::Value) -> Option<ConfigValue> {
    if let Some(selection) = configured_plugin_selection(product, BOT_COMMAND_PLUGIN_ID) {
        let config = selection
            .get("config")
            .cloned()
            .unwrap_or_else(|| toml::Value::Table(toml::map::Map::new()));
        let json = serde_json::to_value(&config).unwrap_or(serde_json::Value::Null);
        return Some(ConfigValue::from_json(&json));
    }
    None
}

fn product_descriptor() -> ConfigDescriptor {
    ConfigDescriptor {
        provider_id: ConfigProviderId::new("product"),
        schema_version: 1,
        value_version: 1,
        title: LocalizedText::new("产品配置"),
        description: Some(LocalizedText::new(
            "来自产品 TOML 的真实装配面（service / distribution / web.console）",
        )),
        scopes: vec![ConfigScope::Global],
        root: ConfigNode {
            key: "product".into(),
            value_type: ConfigValueType::Object,
            title: LocalizedText::new("产品"),
            description: None,
            default_value: None,
            constraints: Default::default(),
            presentation: Default::default(),
            visibility: None,
            enabled_if: None,
            mutability: ConfigMutability::ReadWrite,
            restart_policy: RestartPolicy::BotRestart,
            children: vec![
                string_node("profile", "服务 Profile", ConfigMutability::ReadWrite),
                string_node("instance_id", "实例 ID", ConfigMutability::ReadOnly),
                string_node("distribution_mode", "分发模式", ConfigMutability::ReadOnly),
                bool_node("console_enabled", "启用 Web Console"),
                string_node(
                    "console_listen",
                    "Console 监听地址",
                    ConfigMutability::ReadWrite,
                ),
                bool_node("include_config", "挂载配置页"),
                string_node(
                    "auth_token_key",
                    "Console Auth Secret Key（仅引用名）",
                    ConfigMutability::ReadOnly,
                ),
            ],
        },
        groups: vec![],
    }
}

fn product_defaults(product: &toml::Value) -> ConfigValue {
    let service = product.get("service");
    let distribution = product.get("distribution");
    let console = product.get("web").and_then(|web| web.get("console"));
    let mut map = BTreeMap::new();
    map.insert(
        "profile".into(),
        ConfigValue::String(
            service
                .and_then(|s| s.get("profile"))
                .and_then(|v| v.as_str())
                .unwrap_or("bot")
                .into(),
        ),
    );
    map.insert(
        "instance_id".into(),
        ConfigValue::String(
            service
                .and_then(|s| s.get("instance_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .into(),
        ),
    );
    map.insert(
        "distribution_mode".into(),
        ConfigValue::String(
            distribution
                .and_then(|d| d.get("mode"))
                .and_then(|v| v.as_str())
                .unwrap_or("disabled")
                .into(),
        ),
    );
    map.insert(
        "console_enabled".into(),
        ConfigValue::Bool(
            console
                .and_then(|c| c.get("enabled"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        ),
    );
    map.insert(
        "console_listen".into(),
        ConfigValue::String(
            console
                .and_then(|c| c.get("listen"))
                .and_then(|v| v.as_str())
                .unwrap_or("127.0.0.1:8787")
                .into(),
        ),
    );
    map.insert(
        "include_config".into(),
        ConfigValue::Bool(
            console
                .and_then(|c| c.get("include_config"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        ),
    );
    map.insert(
        "auth_token_key".into(),
        ConfigValue::String(
            console
                .and_then(|c| c.get("auth_token_key"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .into(),
        ),
    );
    ConfigValue::Object(map)
}

fn string_node(key: &str, title: &str, mutability: ConfigMutability) -> ConfigNode {
    ConfigNode {
        key: key.into(),
        value_type: ConfigValueType::String { multiline: false },
        title: LocalizedText::new(title),
        description: None,
        default_value: None,
        constraints: Default::default(),
        presentation: Default::default(),
        visibility: None,
        enabled_if: None,
        mutability,
        restart_policy: RestartPolicy::BotRestart,
        children: vec![],
    }
}

fn bool_node(key: &str, title: &str) -> ConfigNode {
    ConfigNode {
        key: key.into(),
        value_type: ConfigValueType::Bool,
        title: LocalizedText::new(title),
        description: None,
        default_value: None,
        constraints: Default::default(),
        presentation: Default::default(),
        visibility: None,
        enabled_if: None,
        mutability: ConfigMutability::ReadWrite,
        restart_policy: RestartPolicy::BotRestart,
        children: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_bot_config::{ConfigApplyRequest, ConfigRevision, ConfigSource};
    use tempfile::tempdir;

    #[tokio::test]
    async fn product_config_service_registers_product_provider() {
        let root = tempdir().unwrap();
        let path = root.path().join("product.toml");
        std::fs::write(
            &path,
            r#"
[service]
profile = "bot"
instance_id = "demo"

[distribution]
mode = "disabled"

[web.console]
enabled = true
listen = "127.0.0.1:8787"
auth_token_key = "WEB_CONSOLE_AUTH_TOKEN"
include_config = true
"#,
        )
        .unwrap();
        let service = product_config_service(&path).unwrap();
        let caps = vec!["config.schema.read".into(), "config.value.read".into()];
        let providers = service.list_providers(&caps).unwrap();
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].0, "product");
        let snapshot = service
            .read(
                "product",
                mutsuki_bot_config::ConfigContext::global(),
                &caps,
            )
            .await
            .unwrap();
        match snapshot.value {
            ConfigValue::Object(map) => {
                assert_eq!(
                    map.get("instance_id"),
                    Some(&ConfigValue::String("demo".into()))
                );
                assert_eq!(
                    map.get("auth_token_key"),
                    Some(&ConfigValue::String("WEB_CONSOLE_AUTH_TOKEN".into()))
                );
            }
            other => panic!("unexpected value: {other:?}"),
        }
    }

    #[tokio::test]
    async fn product_apply_persists_atomically_and_fails_loud() {
        let root = tempdir().unwrap();
        let path = root.path().join("product.toml");
        std::fs::write(
            &path,
            r#"
[service]
profile = "bot"
instance_id = "demo"

[web.console]
enabled = false
listen = "127.0.0.1:1"
include_config = false
auth_token_key = "WEB_CONSOLE_AUTH_TOKEN"
"#,
        )
        .unwrap();
        let service = product_config_service(&path).unwrap();
        let caps = vec![
            "config.schema.read".into(),
            "config.value.read".into(),
            "config.value.write".into(),
            "config.apply".into(),
        ];
        let snap = service
            .read("product", ConfigContext::global(), &caps)
            .await
            .unwrap();
        let mut candidate = match snap.value {
            ConfigValue::Object(map) => ConfigValue::Object(map),
            other => panic!("{other:?}"),
        };
        candidate.as_object_mut().unwrap().insert(
            "console_listen".into(),
            ConfigValue::String("127.0.0.1:8787".into()),
        );
        candidate
            .as_object_mut()
            .unwrap()
            .insert("console_enabled".into(), ConfigValue::Bool(true));
        let applied = service
            .apply(
                "product",
                ConfigApplyRequest {
                    candidate,
                    expected_revision: snap.revision,
                    dry_run: false,
                },
                ConfigContext::global(),
                &caps,
            )
            .await
            .unwrap();
        assert!(applied.applied);
        let persisted: toml::Value =
            toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            persisted["web"]["console"]["listen"].as_str(),
            Some("127.0.0.1:8787")
        );
        assert_eq!(persisted["web"]["console"]["enabled"].as_bool(), Some(true));
        let again = service
            .read("product", ConfigContext::global(), &caps)
            .await
            .unwrap();
        assert_eq!(again.source, ConfigSource::Persisted);
        assert_eq!(again.revision, ConfigRevision(1));
    }

    #[tokio::test]
    async fn command_provider_registers_when_configured() {
        let root = tempdir().unwrap();
        let path = root.path().join("product.toml");
        std::fs::write(
            &path,
            r#"
[service]
profile = "bot"
instance_id = "demo"

[[plugins.configured]]
id = "mutsuki.bot.command"
config = { prefixes = ["/", "!"] }
"#,
        )
        .unwrap();
        let service = product_config_service(&path).unwrap();
        let caps = vec!["*".into()];
        let providers = service.list_providers(&caps).unwrap();
        assert!(providers.iter().any(|id| id.0 == "mutsuki.bot.command"));
        let schema = service.get_schema("mutsuki.bot.command", &caps).unwrap();
        assert_eq!(schema.provider_id.0, "mutsuki.bot.command");
        let snap = service
            .read(
                "mutsuki.bot.command",
                ConfigContext::plugin_instance("default"),
                &caps,
            )
            .await
            .unwrap();
        match snap.value {
            ConfigValue::Object(map) => match map.get("prefixes") {
                Some(ConfigValue::Array(items)) => {
                    assert_eq!(items.len(), 2);
                }
                other => panic!("unexpected prefixes: {other:?}"),
            },
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn bot_agent_provider_updates_the_live_bridge_and_product_file() {
        let root = tempdir().unwrap();
        let path = root.path().join("product.toml");
        std::fs::write(
            &path,
            r#"
[service]
profile = "bot"
instance_id = "demo"

[[plugins.configured]]
id = "mutsuki.plugin.bot.agent"
config = { enabled = true, default_profile_id = "from-file", streaming = "final_only", max_concurrency = 2, timeout_ms = 10000, max_message_bytes = 1200 }
"#,
        )
        .unwrap();
        let handle = BotAgentConfigHandle::default();
        let service = product_config_service_with_options(
            &path,
            ProductConfigOptions {
                bot_agent_config: Some(handle.clone()),
                ..ProductConfigOptions::default()
            },
        )
        .unwrap();
        let caps = vec!["*".into()];
        assert!(
            service
                .list_providers(&caps)
                .unwrap()
                .iter()
                .any(|id| id.0 == BOT_AGENT_CONFIG_PROVIDER_ID)
        );
        assert_eq!(
            handle.snapshot().default_profile_id,
            "from-file".to_string()
        );

        let snapshot = service
            .read(
                BOT_AGENT_CONFIG_PROVIDER_ID,
                ConfigContext::plugin_instance("default"),
                &caps,
            )
            .await
            .unwrap();
        let mut candidate = snapshot.value;
        candidate.as_object_mut().unwrap().insert(
            "streaming".into(),
            ConfigValue::String("segment_messages".into()),
        );
        let result = service
            .apply(
                BOT_AGENT_CONFIG_PROVIDER_ID,
                ConfigApplyRequest {
                    candidate,
                    expected_revision: snapshot.revision,
                    dry_run: false,
                },
                ConfigContext::plugin_instance("default"),
                &caps,
            )
            .await
            .unwrap();
        assert!(result.applied);
        assert_eq!(result.restart_policy, RestartPolicy::PluginReload);
        assert_eq!(handle.snapshot().streaming, "segment_messages");
        let persisted: toml::Value =
            toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            persisted["plugins"]["configured"][0]["config"]["streaming"].as_str(),
            Some("segment_messages")
        );
    }

    #[test]
    fn selected_bot_agent_without_a_live_bridge_fails_loud() {
        let root = tempdir().unwrap();
        let path = root.path().join("product.toml");
        std::fs::write(
            &path,
            r#"
[[plugins.configured]]
id = "mutsuki.plugin.bot.agent"
"#,
        )
        .unwrap();
        assert!(matches!(
            product_config_service(&path),
            Err(ProductConfigError::MissingBotAgentBridge)
        ));
    }

    #[test]
    fn missing_product_file_fails_loud() {
        match product_config_service(Path::new("/no/such/product.toml")) {
            Err(ProductConfigError::Unreadable(_)) => {}
            Err(other) => panic!("unexpected error: {other}"),
            Ok(_) => panic!("expected unreadable product config"),
        }
    }
}
