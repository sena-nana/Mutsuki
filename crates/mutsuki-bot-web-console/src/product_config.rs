//! Product TOML ConfigProvider for Console `include_config` assembly.
//!
//! Apply goes through Host `ConfiguredPluginStore` atomic writes. Demo/memory-only
//! providers stay test-only.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;

use mutsuki_bot_config::{
    ConfigApplyMode, ConfigContext, ConfigDescriptor, ConfigError, ConfigLifecycle,
    ConfigMutability, ConfigNode, ConfigPersistSink, ConfigProviderId, ConfigProviderRegistry,
    ConfigScope, ConfigService, ConfigValue, ConfigValueType, LocalizedText, MemoryConfigProvider,
    MutsukiConfigSchema, RestartPolicy,
};
use mutsuki_plugin_bot_command::{BOT_COMMAND_PLUGIN_ID, BotCommandConfig};
use mutsuki_service_config::ConfiguredPluginStore;

#[derive(Debug, thiserror::Error)]
pub enum ProductConfigError {
    #[error("product config unreadable: {0}")]
    Unreadable(String),
    #[error("product config invalid: {0}")]
    Invalid(String),
    #[error("product config provider registration failed: {0}")]
    Register(String),
}

/// Options for assembling the product ConfigService.
#[derive(Default)]
pub struct ProductConfigOptions {
    pub store: Option<ConfiguredPluginStore>,
    pub lifecycle: Option<Arc<dyn ConfigLifecycle>>,
}

/// Build a ConfigService backed by the product TOML surface (not an empty registry).
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
    let store = options
        .store
        .unwrap_or_else(|| ConfiguredPluginStore::open(product_config_path));
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
            })),
        );
        registry
            .register(command_provider)
            .map_err(|error| ProductConfigError::Register(error.to_string()))?;
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
    fn persist(
        &self,
        _context: &ConfigContext,
        value: &ConfigValue,
        _secrets: &HashMap<String, String>,
    ) -> Result<(), ConfigError> {
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
                fields.insert(key.to_string(), config_value_to_json(field)?);
            }
        }
        self.store
            .patch_product_surface(fields)
            .map_err(|error| ConfigError::PersistenceFailed {
                reason: error.to_string(),
            })
    }
}

struct ConfiguredPluginPersist {
    store: ConfiguredPluginStore,
    plugin_id: String,
}

impl ConfigPersistSink for ConfiguredPluginPersist {
    fn persist(
        &self,
        _context: &ConfigContext,
        value: &ConfigValue,
        _secrets: &HashMap<String, String>,
    ) -> Result<(), ConfigError> {
        let json = config_value_to_json(value)?;
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
        self.store
            .replace_config(&self.plugin_id, json)
            .map_err(|error| ConfigError::PersistenceFailed {
                reason: error.to_string(),
            })
    }
}

fn config_value_to_json(value: &ConfigValue) -> Result<serde_json::Value, ConfigError> {
    Ok(match value {
        ConfigValue::Null => serde_json::Value::Null,
        ConfigValue::Bool(v) => serde_json::Value::Bool(*v),
        ConfigValue::Integer(v) => serde_json::json!(*v),
        ConfigValue::Float(v) => serde_json::json!(*v),
        ConfigValue::String(v) => serde_json::Value::String(v.clone()),
        ConfigValue::Array(items) => serde_json::Value::Array(
            items
                .iter()
                .map(config_value_to_json)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        ConfigValue::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, child) in map {
                out.insert(key.clone(), config_value_to_json(child)?);
            }
            serde_json::Value::Object(out)
        }
        ConfigValue::Secret(_) => {
            return Err(ConfigError::PersistenceFailed {
                reason: "secret values must not be serialized into product TOML".into(),
            });
        }
    })
}

fn json_to_config_value(value: &serde_json::Value) -> ConfigValue {
    match value {
        serde_json::Value::Null => ConfigValue::Null,
        serde_json::Value::Bool(v) => ConfigValue::Bool(*v),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                ConfigValue::Integer(i)
            } else if let Some(u) = n.as_u64() {
                ConfigValue::Integer(i64::try_from(u).unwrap_or(i64::MAX))
            } else {
                ConfigValue::Float(n.as_f64().unwrap_or_default())
            }
        }
        serde_json::Value::String(s) => ConfigValue::String(s.clone()),
        serde_json::Value::Array(items) => {
            ConfigValue::Array(items.iter().map(json_to_config_value).collect())
        }
        serde_json::Value::Object(map) => ConfigValue::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), json_to_config_value(v)))
                .collect(),
        ),
    }
}

fn command_defaults_from_product(product: &toml::Value) -> Option<ConfigValue> {
    let configured = product
        .get("plugins")
        .and_then(|plugins| plugins.get("configured"))
        .and_then(|value| value.as_array())?;
    for selection in configured {
        let id = selection.get("id").and_then(|value| value.as_str())?;
        if id.trim() != BOT_COMMAND_PLUGIN_ID {
            continue;
        }
        let config = selection
            .get("config")
            .cloned()
            .unwrap_or_else(|| toml::Value::Table(toml::map::Map::new()));
        let json = toml_to_json(&config);
        return Some(json_to_config_value(&json));
    }
    None
}

fn toml_to_json(value: &toml::Value) -> serde_json::Value {
    match value {
        toml::Value::String(v) => serde_json::Value::String(v.clone()),
        toml::Value::Integer(v) => serde_json::json!(*v),
        toml::Value::Float(v) => serde_json::json!(*v),
        toml::Value::Boolean(v) => serde_json::Value::Bool(*v),
        toml::Value::Datetime(v) => serde_json::Value::String(v.to_string()),
        toml::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(toml_to_json).collect())
        }
        toml::Value::Table(map) => {
            let mut out = serde_json::Map::new();
            for (key, child) in map {
                out.insert(key.clone(), toml_to_json(child));
            }
            serde_json::Value::Object(out)
        }
    }
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

    #[test]
    fn missing_product_file_fails_loud() {
        match product_config_service(Path::new("/no/such/product.toml")) {
            Err(ProductConfigError::Unreadable(_)) => {}
            Err(other) => panic!("unexpected error: {other}"),
            Ok(_) => panic!("expected unreadable product config"),
        }
    }
}
