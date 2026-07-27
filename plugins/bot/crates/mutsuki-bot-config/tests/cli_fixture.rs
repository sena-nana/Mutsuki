//! CLI / automation fixture — same Schema + ConfigService path as Web (no WebHost).

use std::sync::{Arc, Mutex};

use mutsuki_bot_config::*;

#[derive(MutsukiConfig)]
#[config(provider_id = "cli_fixture", title = "CLI Fixture")]
#[allow(dead_code)]
struct CliFixtureConfig {
    #[config(title = "Name", required)]
    name: String,
    #[config(title = "Mode", format = "cron-expression")]
    schedule: String,
    #[config(title = "Tags")]
    tags: Vec<String>,
}

fn caps() -> Vec<String> {
    vec!["*".into()]
}

#[tokio::test]
async fn cli_and_automation_share_schema_apply_and_revision_watch() {
    let registry = Arc::new(ConfigProviderRegistry::default());
    let defaults = ConfigValue::Object(
        [
            ("name".into(), ConfigValue::String("cli".into())),
            ("schedule".into(), ConfigValue::String("0 * * * *".into())),
            (
                "tags".into(),
                ConfigValue::Array(vec![ConfigValue::String("a".into())]),
            ),
        ]
        .into_iter()
        .collect(),
    );
    registry
        .register(Arc::new(MemoryConfigProvider::new(
            CliFixtureConfig::schema(),
            defaults,
            ConfigApplyMode::HotReload,
        )))
        .unwrap();
    let service = ConfigService::new(registry);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_watch = seen.clone();
    service.subscribe_revision_changed(Arc::new(move |event| {
        seen_watch
            .lock()
            .unwrap()
            .push((event.provider_id.0.clone(), event.revision.0));
    }));

    let schema = service.get_schema("cli_fixture", &caps()).unwrap();
    assert_eq!(schema.provider_id.as_str(), "cli_fixture");
    let schedule = schema
        .root
        .children
        .iter()
        .find(|node| node.key.as_str() == "schedule")
        .unwrap();
    assert_eq!(
        schedule.presentation.format.as_deref(),
        Some("cron-expression")
    );
    let tags = schema
        .root
        .children
        .iter()
        .find(|node| node.key.as_str() == "tags")
        .unwrap();
    assert!(matches!(tags.value_type, ConfigValueType::Array { .. }));

    let ctx = ConfigContext::plugin_instance("cli");
    let snap = service
        .read("cli_fixture", ctx.clone(), &caps())
        .await
        .unwrap();
    let applied = service
        .apply(
            "cli_fixture",
            ConfigApplyRequest {
                candidate: ConfigValue::Object(
                    [
                        ("name".into(), ConfigValue::String("updated".into())),
                        ("schedule".into(), ConfigValue::String("*/5 * * * *".into())),
                        (
                            "tags".into(),
                            ConfigValue::Array(vec![
                                ConfigValue::String("a".into()),
                                ConfigValue::String("b".into()),
                            ]),
                        ),
                    ]
                    .into_iter()
                    .collect(),
                ),
                expected_revision: snap.revision,
                dry_run: false,
            },
            ctx.clone(),
            &caps(),
        )
        .await
        .unwrap();
    assert!(applied.applied);

    // First write keeps revision 1; advance once more so stale expected can conflict.
    let advanced = service
        .apply(
            "cli_fixture",
            ConfigApplyRequest {
                candidate: ConfigValue::Object(
                    [
                        ("name".into(), ConfigValue::String("updated-2".into())),
                        ("schedule".into(), ConfigValue::String("*/5 * * * *".into())),
                        (
                            "tags".into(),
                            ConfigValue::Array(vec![ConfigValue::String("a".into())]),
                        ),
                    ]
                    .into_iter()
                    .collect(),
                ),
                expected_revision: applied.revision,
                dry_run: false,
            },
            ctx.clone(),
            &caps(),
        )
        .await
        .unwrap();
    assert_eq!(advanced.revision.0, 2);

    let conflict = service
        .apply(
            "cli_fixture",
            ConfigApplyRequest {
                candidate: ConfigValue::Object(
                    [
                        ("name".into(), ConfigValue::String("stale".into())),
                        ("schedule".into(), ConfigValue::String("0 * * * *".into())),
                        ("tags".into(), ConfigValue::Array(vec![])),
                    ]
                    .into_iter()
                    .collect(),
                ),
                expected_revision: snap.revision,
                dry_run: false,
            },
            ctx,
            &caps(),
        )
        .await
        .unwrap_err();
    assert!(matches!(conflict, ConfigError::RevisionConflict { .. }));

    let events = seen.lock().unwrap().clone();
    assert!(events.contains(&("cli_fixture".into(), applied.revision.0)));
    assert!(events.contains(&("cli_fixture".into(), advanced.revision.0)));
}
