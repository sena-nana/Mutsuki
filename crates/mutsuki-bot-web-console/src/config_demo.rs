//! Minimal ConfigService for embedded console demos when no product providers are registered.

use std::sync::Arc;

use mutsuki_bot_config::{
    ConfigApplyMode, ConfigDescriptor, ConfigMutability, ConfigNode, ConfigProviderId,
    ConfigProviderRegistry, ConfigScope, ConfigService, ConfigValue, ConfigValueType,
    LocalizedText, MemoryConfigProvider, RestartPolicy, SecretState,
};

pub fn demo_config_service() -> Arc<ConfigService> {
    let registry = Arc::new(ConfigProviderRegistry::default());
    let descriptor = ConfigDescriptor {
        provider_id: ConfigProviderId::new("product"),
        schema_version: 1,
        value_version: 1,
        title: LocalizedText::new("产品设置"),
        description: Some(LocalizedText::new("Console 演示用最小 ConfigProvider")),
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
            restart_policy: RestartPolicy::None,
            children: vec![
                ConfigNode {
                    key: "display_name".into(),
                    value_type: ConfigValueType::String { multiline: false },
                    title: LocalizedText::new("显示名称"),
                    description: None,
                    default_value: Some(ConfigValue::String("Mutsuki Bot".into())),
                    constraints: Default::default(),
                    presentation: Default::default(),
                    visibility: None,
                    enabled_if: None,
                    mutability: ConfigMutability::ReadWrite,
                    restart_policy: RestartPolicy::None,
                    children: vec![],
                },
                ConfigNode {
                    key: "owner_token".into(),
                    value_type: ConfigValueType::Secret,
                    title: LocalizedText::new("Owner Token"),
                    description: Some(LocalizedText::new("演示 secret 字段，不回显值")),
                    default_value: Some(ConfigValue::Secret(SecretState::Absent)),
                    constraints: Default::default(),
                    presentation: Default::default(),
                    visibility: None,
                    enabled_if: None,
                    mutability: ConfigMutability::ReadWrite,
                    restart_policy: RestartPolicy::None,
                    children: vec![],
                },
            ],
        },
        groups: vec![],
    };
    let defaults = ConfigValue::Object(
        [
            (
                "display_name".into(),
                ConfigValue::String("Mutsuki Bot".into()),
            ),
            (
                "owner_token".into(),
                ConfigValue::Secret(SecretState::Absent),
            ),
        ]
        .into_iter()
        .collect(),
    );
    registry
        .register(Arc::new(MemoryConfigProvider::new(
            descriptor,
            defaults,
            ConfigApplyMode::HotReload,
        )))
        .expect("demo config provider registers once");
    Arc::new(ConfigService::new(registry))
}
