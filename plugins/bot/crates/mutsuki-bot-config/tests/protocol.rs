use std::collections::BTreeMap;
use std::sync::Arc;

use mutsuki_bot_config::*;

#[derive(MutsukiConfig)]
#[config(provider_id = "discord", title = "Discord")]
#[allow(dead_code)]
struct DiscordConfig {
    #[config(title = "Bot Token", secret, required)]
    token: String,
    #[config(title = "指令前缀", default = "/")]
    command_prefix: String,
    #[config(title = "自动重连")]
    auto_reconnect: bool,
    #[config(
        title = "重连间隔",
        unit = "秒",
        min = 1.0,
        max = 300.0,
        visible_if = "auto_reconnect == true",
        restart = "plugin_reload"
    )]
    reconnect_interval: u32,
}

fn defaults() -> ConfigValue {
    ConfigValue::Object(BTreeMap::from([
        ("token".into(), ConfigValue::Secret(SecretState::Absent)),
        ("command_prefix".into(), ConfigValue::String("/".into())),
        ("auto_reconnect".into(), ConfigValue::Bool(true)),
        ("reconnect_interval".into(), ConfigValue::Integer(5)),
    ]))
}

fn provider() -> Arc<MemoryConfigProvider> {
    Arc::new(MemoryConfigProvider::new(
        DiscordConfig::schema(),
        defaults(),
        ConfigApplyMode::HotReload,
    ))
}

#[tokio::test]
async fn derive_schema_round_trip() {
    let schema = DiscordConfig::schema();
    assert_eq!(schema.provider_id.as_str(), "discord");
    assert!(schema.root.children.iter().any(|n| n.presentation.secret));
    let encoded = serde_json::to_value(&schema).unwrap();
    let decoded: ConfigDescriptor = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded.provider_id, schema.provider_id);
}

#[tokio::test]
async fn read_validate_apply_revision_and_conflict() {
    let p = provider();
    let ctx = ConfigContext::plugin_instance("demo");
    let snap = p.read(ctx.clone()).await.unwrap();
    assert_eq!(snap.revision.0, 1);
    match snap.value.get_path(&ConfigPath(vec!["token".into()])) {
        Some(ConfigValue::Secret(SecretState::Absent)) => {}
        other => panic!("secret leaked or wrong: {other:?}"),
    }

    let mut candidate = defaults();
    candidate.as_object_mut().unwrap().insert(
        "token".into(),
        ConfigValue::Secret(SecretState::Set {
            value: SecretValue::new("super-secret"),
        }),
    );
    candidate
        .as_object_mut()
        .unwrap()
        .insert("command_prefix".into(), ConfigValue::String("!".into()));

    let applied = p
        .apply(
            ConfigApplyRequest {
                candidate: candidate.clone(),
                expected_revision: ConfigRevision(1),
                dry_run: false,
            },
            ctx.clone(),
        )
        .await
        .unwrap();
    assert!(applied.applied);
    assert_eq!(applied.revision.0, 1);

    let snap2 = p.read(ctx.clone()).await.unwrap();
    assert_eq!(snap2.revision.0, 1);
    let ok = p
        .apply(
            ConfigApplyRequest {
                candidate: {
                    let mut v = defaults();
                    v.as_object_mut()
                        .unwrap()
                        .insert("token".into(), ConfigValue::Secret(SecretState::Keep));
                    v.as_object_mut()
                        .unwrap()
                        .insert("command_prefix".into(), ConfigValue::String("!!".into()));
                    v
                },
                expected_revision: snap2.revision,
                dry_run: false,
            },
            ctx.clone(),
        )
        .await
        .unwrap();
    assert_eq!(ok.revision.0, 2);

    let stale = p
        .apply(
            ConfigApplyRequest {
                candidate,
                expected_revision: ConfigRevision(1),
                dry_run: false,
            },
            ctx,
        )
        .await
        .unwrap_err();
    assert!(matches!(
        stale,
        ConfigError::RevisionConflict { current: 2, .. }
    ));
}

#[tokio::test]
async fn secret_keep_set_clear_and_no_plaintext_read() {
    let p = provider();
    let ctx = ConfigContext::plugin_instance("sec");
    p.apply(
        ConfigApplyRequest {
            candidate: {
                let mut v = defaults();
                v.as_object_mut().unwrap().insert(
                    "token".into(),
                    ConfigValue::Secret(SecretState::Set {
                        value: SecretValue::new("abc"),
                    }),
                );
                v
            },
            expected_revision: ConfigRevision(1),
            dry_run: false,
        },
        ctx.clone(),
    )
    .await
    .unwrap();
    let snap = p.read(ctx.clone()).await.unwrap();
    let token = snap
        .value
        .get_path(&ConfigPath(vec!["token".into()]))
        .unwrap();
    assert_eq!(token, &ConfigValue::Secret(SecretState::Configured));
    assert!(!format!("{token:?}").contains("abc"));

    p.apply(
        ConfigApplyRequest {
            candidate: {
                let mut v = defaults();
                v.as_object_mut()
                    .unwrap()
                    .insert("token".into(), ConfigValue::Secret(SecretState::Clear));
                v
            },
            expected_revision: snap.revision,
            dry_run: false,
        },
        ctx.clone(),
    )
    .await
    .unwrap();
    let snap2 = p.read(ctx).await.unwrap();
    assert_eq!(
        snap2.value.get_path(&ConfigPath(vec!["token".into()])),
        Some(&ConfigValue::Secret(SecretState::Absent))
    );
}

#[tokio::test]
async fn scope_isolation_and_unsupported_scope() {
    let p = provider();
    let a = ConfigContext::plugin_instance("a");
    let b = ConfigContext::plugin_instance("b");
    p.apply(
        ConfigApplyRequest {
            candidate: {
                let mut v = defaults();
                v.as_object_mut()
                    .unwrap()
                    .insert("command_prefix".into(), ConfigValue::String("a".into()));
                v.as_object_mut()
                    .unwrap()
                    .insert("token".into(), ConfigValue::Secret(SecretState::Keep));
                v
            },
            expected_revision: ConfigRevision(1),
            dry_run: false,
        },
        a.clone(),
    )
    .await
    .unwrap();
    let snap_b = p.read(b).await.unwrap();
    assert_eq!(
        snap_b
            .value
            .get_path(&ConfigPath(vec!["command_prefix".into()])),
        Some(&ConfigValue::String("/".into()))
    );

    let bad = ConfigContext {
        scope: ConfigScope::Bot,
        bot_id: Some(BotId::new("bot-1")),
        ..ConfigContext::global()
    };
    let err = p.read(bad).await.unwrap_err();
    assert!(matches!(err, ConfigError::ScopeUnsupported { .. }));
}

#[tokio::test]
async fn validation_rejects_range_unknown_and_type() {
    let schema = DiscordConfig::schema();
    let mut bad = defaults();
    bad.as_object_mut()
        .unwrap()
        .insert("reconnect_interval".into(), ConfigValue::Integer(999));
    bad.as_object_mut()
        .unwrap()
        .insert("extra".into(), ConfigValue::Bool(true));
    let result = validate_structure(&schema, &bad);
    assert!(!result.ok);
    assert!(
        result
            .issues
            .iter()
            .any(|i| i.code == ValidationCode::OutOfRange)
    );
    assert!(
        result
            .issues
            .iter()
            .any(|i| i.code == ValidationCode::UnknownField)
    );
}

#[tokio::test]
async fn expression_and_restart_policy_on_apply() {
    let expr = ConfigExpr::parse_simple("auto_reconnect == true").unwrap();
    let root = defaults();
    assert!(expr.eval(&root).unwrap());
    let p = provider();
    let ctx = ConfigContext::plugin_instance("restart");
    let result = p
        .apply(
            ConfigApplyRequest {
                candidate: {
                    let mut v = defaults();
                    v.as_object_mut()
                        .unwrap()
                        .insert("reconnect_interval".into(), ConfigValue::Integer(9));
                    v.as_object_mut()
                        .unwrap()
                        .insert("token".into(), ConfigValue::Secret(SecretState::Keep));
                    v
                },
                expected_revision: ConfigRevision(1),
                dry_run: false,
            },
            ctx,
        )
        .await
        .unwrap();
    assert_eq!(result.restart_policy, RestartPolicy::PluginReload);
    assert!(
        result
            .pending_actions
            .contains(&ConfigAction::PluginReloaded)
    );
    assert!(!result.actions.contains(&ConfigAction::PluginReloaded));
}

#[tokio::test]
async fn migration_dry_run_does_not_destroy_original() {
    let original = defaults();
    let plan = MigrationPlan {
        from_version: 1,
        to_version: 2,
        steps: vec![MigrationStep::RenameField {
            from: "command_prefix".into(),
            to: "prefix".into(),
        }],
    };
    let (projected, _) = migrate(&original, &plan, true).unwrap();
    assert!(projected.as_object().unwrap().contains_key("prefix"));
    assert!(original.as_object().unwrap().contains_key("command_prefix"));
    let err = require_migration(1, 2).unwrap_err();
    assert!(matches!(err, ConfigError::ValueMigrationRequired { .. }));
}

#[tokio::test]
async fn budget_rejects_malicious_depth() {
    let mut budgets = DEFAULT_BUDGETS;
    budgets.max_schema_depth = 1;
    let mut node = ConfigNode {
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
        children: vec![],
    };
    node.children.push(ConfigNode {
        key: ConfigKey::new("child"),
        value_type: ConfigValueType::Object,
        title: LocalizedText::new("child"),
        description: None,
        default_value: None,
        constraints: Default::default(),
        presentation: Default::default(),
        visibility: None,
        enabled_if: None,
        mutability: ConfigMutability::ReadWrite,
        restart_policy: RestartPolicy::None,
        children: vec![ConfigNode {
            key: ConfigKey::new("deep"),
            value_type: ConfigValueType::Bool,
            title: LocalizedText::new("deep"),
            description: None,
            default_value: None,
            constraints: Default::default(),
            presentation: Default::default(),
            visibility: None,
            enabled_if: None,
            mutability: ConfigMutability::ReadWrite,
            restart_policy: RestartPolicy::None,
            children: vec![],
        }],
    });
    let descriptor = ConfigDescriptor {
        provider_id: ConfigProviderId::new("x"),
        schema_version: 1,
        value_version: 1,
        title: LocalizedText::new("x"),
        description: None,
        scopes: vec![ConfigScope::Global],
        root: node,
        groups: vec![],
    };
    assert!(matches!(
        descriptor.validate_budgets(&budgets),
        Err(ConfigError::BudgetExceeded { .. })
    ));
}

#[tokio::test]
async fn service_enforces_capabilities() {
    let registry = Arc::new(ConfigProviderRegistry::default());
    registry.register(provider()).unwrap();
    let service = ConfigService::new(registry);
    let denied = service.list_providers(&[]).unwrap_err();
    assert!(matches!(denied, ConfigError::PermissionDenied { .. }));
    let list = service
        .list_providers(&[capability::SCHEMA_READ.into()])
        .unwrap();
    assert_eq!(list[0].as_str(), "discord");
}

#[tokio::test]
async fn debug_redacts_secret_value() {
    let secret = SecretValue::new("should-not-appear");
    assert!(!format!("{secret:?}").contains("should-not-appear"));
}
