//! Product TOML ConfigProvider for Console `include_config` assembly.
//!
//! Registers a real product-scoped provider from the product config file.
//! Secrets stay as key references only; values are never mirrored into the
//! descriptor. Demo providers remain test-only.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use mutsuki_bot_config::{
    ConfigApplyMode, ConfigDescriptor, ConfigMutability, ConfigNode, ConfigProviderId,
    ConfigProviderRegistry, ConfigScope, ConfigService, ConfigValue, ConfigValueType,
    LocalizedText, MemoryConfigProvider, RestartPolicy,
};

#[derive(Debug, thiserror::Error)]
pub enum ProductConfigError {
    #[error("product config unreadable: {0}")]
    Unreadable(String),
    #[error("product config invalid: {0}")]
    Invalid(String),
    #[error("product config provider registration failed: {0}")]
    Register(String),
}

/// Build a ConfigService backed by the product TOML surface (not an empty registry).
pub fn product_config_service(
    product_config_path: &Path,
) -> Result<Arc<ConfigService>, ProductConfigError> {
    let text = std::fs::read_to_string(product_config_path).map_err(|error| {
        ProductConfigError::Unreadable(format!("{}: {error}", product_config_path.display()))
    })?;
    let product: toml::Value =
        toml::from_str(&text).map_err(|error| ProductConfigError::Invalid(error.to_string()))?;
    let defaults = product_defaults(&product);
    let descriptor = product_descriptor();
    let registry = Arc::new(ConfigProviderRegistry::default());
    registry
        .register(Arc::new(MemoryConfigProvider::new(
            descriptor,
            defaults,
            ConfigApplyMode::RequireRestart,
        )))
        .map_err(|error| ProductConfigError::Register(error.to_string()))?;
    Ok(Arc::new(ConfigService::new(registry)))
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

    #[test]
    fn missing_product_file_fails_loud() {
        match product_config_service(Path::new("/no/such/product.toml")) {
            Err(ProductConfigError::Unreadable(_)) => {}
            Err(other) => panic!("unexpected error: {other}"),
            Ok(_) => panic!("expected unreadable product config"),
        }
    }
}
