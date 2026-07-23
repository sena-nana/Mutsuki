use std::sync::Arc;

use criterion::{Criterion, criterion_group, criterion_main};
use mutsuki_bot_config::*;

fn large_schema(nodes: usize) -> ConfigDescriptor {
    let children = (0..nodes)
        .map(|i| ConfigNode {
            key: ConfigKey::new(format!("f{i}")),
            value_type: ConfigValueType::Integer,
            title: LocalizedText::new(format!("Field {i}")),
            description: None,
            default_value: Some(ConfigValue::Integer(0)),
            constraints: ConfigConstraints {
                min: Some(0.0),
                max: Some(100.0),
                ..Default::default()
            },
            presentation: ConfigPresentation {
                order: i as i32,
                ..Default::default()
            },
            visibility: None,
            enabled_if: None,
            mutability: ConfigMutability::ReadWrite,
            restart_policy: RestartPolicy::None,
            children: vec![],
        })
        .collect();
    ConfigDescriptor {
        provider_id: ConfigProviderId::new("perf"),
        schema_version: 1,
        value_version: 1,
        title: LocalizedText::new("perf"),
        description: None,
        scopes: vec![ConfigScope::PluginInstance],
        root: ConfigNode {
            key: ConfigKey::new("root"),
            value_type: ConfigValueType::Object,
            title: LocalizedText::new("root"),
            description: None,
            default_value: None,
            constraints: Default::default(),
            presentation: Default::default(),
            visibility: None,
            enabled_if: None,
            mutability: ConfigMutability::ReadWrite,
            restart_policy: RestartPolicy::None,
            children,
        },
        groups: vec![],
    }
}

fn bench_schema_cache_and_validate(c: &mut Criterion) {
    let schema = large_schema(256);
    schema.validate_default_budgets().unwrap();
    let registry = ConfigProviderRegistry::default();
    let provider = Arc::new(MemoryConfigProvider::new(
        schema.clone(),
        ConfigValue::object_empty(),
        ConfigApplyMode::PersistOnly,
    ));
    registry.register(provider).unwrap();

    c.bench_function("schema_cache_hit", |b| {
        b.iter(|| {
            let _ = registry.schema("perf").unwrap();
        });
    });

    let mut value = std::collections::BTreeMap::new();
    for i in 0..256 {
        value.insert(format!("f{i}"), ConfigValue::Integer(i as i64 % 50));
    }
    let candidate = ConfigValue::Object(value);
    c.bench_function("validate_256_fields", |b| {
        b.iter(|| {
            let result = validate_structure(&schema, &candidate);
            assert!(result.ok);
        });
    });
}

criterion_group!(benches, bench_schema_cache_and_validate);
criterion_main!(benches);
