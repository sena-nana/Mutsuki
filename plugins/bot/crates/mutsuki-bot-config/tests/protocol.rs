use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

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

#[derive(Clone)]
struct PersistenceState {
    live: ConfigValue,
    durable: ConfigValue,
    fail_commit: bool,
    activations: usize,
    commits: usize,
    rollbacks: usize,
}

struct RecordingPersistSink {
    state: Arc<Mutex<PersistenceState>>,
}

impl ConfigPersistSink for RecordingPersistSink {
    fn prepare(
        &self,
        _context: &ConfigContext,
        value: &ConfigValue,
        _secrets: &std::collections::HashMap<String, String>,
    ) -> Result<Box<dyn PreparedConfigPersist>, ConfigError> {
        let state = self.state.lock().unwrap();
        Ok(Box::new(RecordingPersistChange {
            state: self.state.clone(),
            previous_live: state.live.clone(),
            previous_durable: state.durable.clone(),
            candidate: value.clone(),
        }))
    }
}

struct RecordingPersistChange {
    state: Arc<Mutex<PersistenceState>>,
    previous_live: ConfigValue,
    previous_durable: ConfigValue,
    candidate: ConfigValue,
}

impl PreparedConfigPersist for RecordingPersistChange {
    fn activate(&mut self) -> Result<(), ConfigError> {
        let mut state = self.state.lock().unwrap();
        state.activations += 1;
        state.live = self.candidate.clone();
        Ok(())
    }

    fn commit(&mut self) -> Result<(), ConfigError> {
        let mut state = self.state.lock().unwrap();
        state.commits += 1;
        if state.fail_commit {
            return Err(ConfigError::PersistenceFailed {
                reason: "injected durable commit failure".into(),
            });
        }
        state.durable = self.candidate.clone();
        Ok(())
    }

    fn rollback(&mut self) -> Result<(), ConfigError> {
        let mut state = self.state.lock().unwrap();
        state.rollbacks += 1;
        state.live = self.previous_live.clone();
        state.durable = self.previous_durable.clone();
        Ok(())
    }
}

struct RecordingLifecycle {
    persistence: Arc<Mutex<PersistenceState>>,
    fail_execute: bool,
    observed_live: Mutex<Vec<ConfigValue>>,
    rollbacks: AtomicUsize,
}

impl ConfigLifecycle for RecordingLifecycle {
    fn execute(
        &self,
        _provider_id: &str,
        _policy: RestartPolicy,
        pending: &[ConfigAction],
    ) -> Result<Vec<ConfigAction>, ConfigError> {
        self.observed_live
            .lock()
            .unwrap()
            .push(self.persistence.lock().unwrap().live.clone());
        if self.fail_execute {
            Err(ConfigError::ReloadFailed {
                reason: "injected runtime reload failure".into(),
            })
        } else {
            Ok(pending.to_vec())
        }
    }

    fn rollback(
        &self,
        _provider_id: &str,
        _policy: RestartPolicy,
        _completed: &[ConfigAction],
    ) -> Result<(), ConfigError> {
        self.rollbacks.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn transactional_service(
    fail_commit: bool,
    fail_execute: bool,
) -> (
    ConfigService,
    Arc<MemoryConfigProvider>,
    Arc<Mutex<PersistenceState>>,
    Arc<RecordingLifecycle>,
) {
    let state = Arc::new(Mutex::new(PersistenceState {
        live: defaults(),
        durable: defaults(),
        fail_commit,
        activations: 0,
        commits: 0,
        rollbacks: 0,
    }));
    let provider = Arc::new(
        MemoryConfigProvider::new(
            DiscordConfig::schema(),
            defaults(),
            ConfigApplyMode::HotReload,
        )
        .with_persist(Arc::new(RecordingPersistSink {
            state: state.clone(),
        })),
    );
    let registry = Arc::new(ConfigProviderRegistry::default());
    registry.register(provider.clone()).unwrap();
    let lifecycle = Arc::new(RecordingLifecycle {
        persistence: state.clone(),
        fail_execute,
        observed_live: Mutex::new(Vec::new()),
        rollbacks: AtomicUsize::new(0),
    });
    (
        ConfigService::new(registry).with_lifecycle(lifecycle.clone()),
        provider,
        state,
        lifecycle,
    )
}

fn changed_candidate(prefix: &str) -> ConfigValue {
    let mut candidate = defaults();
    candidate
        .as_object_mut()
        .unwrap()
        .insert("command_prefix".into(), ConfigValue::String(prefix.into()));
    candidate
        .as_object_mut()
        .unwrap()
        .insert("reconnect_interval".into(), ConfigValue::Integer(6));
    candidate
}

fn write_capabilities() -> Vec<String> {
    vec![
        capability::APPLY.into(),
        capability::VALUE_WRITE.into(),
        capability::SECRET_WRITE.into(),
    ]
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
async fn failed_reload_restores_live_durable_and_provider_snapshots() {
    let (service, provider, persistence, lifecycle) = transactional_service(false, true);
    let notifications = Arc::new(AtomicUsize::new(0));
    let notification_count = notifications.clone();
    service.subscribe_revision_changed(Arc::new(move |_| {
        notification_count.fetch_add(1, Ordering::SeqCst);
    }));
    let context = ConfigContext::plugin_instance("reload-failure");
    let candidate = changed_candidate("!");

    assert!(matches!(
        service
            .apply(
                "discord",
                ConfigApplyRequest {
                    candidate: candidate.clone(),
                    expected_revision: ConfigRevision(1),
                    dry_run: false,
                },
                context.clone(),
                &write_capabilities(),
            )
            .await,
        Err(ConfigError::ReloadFailed { .. })
    ));

    let state = persistence.lock().unwrap().clone();
    assert_eq!(state.live, defaults());
    assert_eq!(state.durable, defaults());
    assert_eq!(
        (state.activations, state.commits, state.rollbacks),
        (1, 0, 1)
    );
    assert_eq!(
        lifecycle.observed_live.lock().unwrap().as_slice(),
        &[candidate]
    );
    assert_eq!(lifecycle.rollbacks.load(Ordering::SeqCst), 1);
    assert_eq!(notifications.load(Ordering::SeqCst), 0);
    let snapshot = provider.read(context).await.unwrap();
    assert_eq!(snapshot.revision, ConfigRevision(1));
    assert_eq!(snapshot.value, defaults());
}

#[tokio::test]
async fn failed_durable_commit_reloads_the_previous_candidate_and_never_publishes() {
    let (service, provider, persistence, lifecycle) = transactional_service(true, false);
    let notifications = Arc::new(AtomicUsize::new(0));
    let notification_count = notifications.clone();
    service.subscribe_revision_changed(Arc::new(move |_| {
        notification_count.fetch_add(1, Ordering::SeqCst);
    }));
    let context = ConfigContext::plugin_instance("commit-failure");
    let candidate = changed_candidate("!");

    assert!(matches!(
        service
            .apply(
                "discord",
                ConfigApplyRequest {
                    candidate: candidate.clone(),
                    expected_revision: ConfigRevision(1),
                    dry_run: false,
                },
                context.clone(),
                &write_capabilities(),
            )
            .await,
        Err(ConfigError::PersistenceFailed { .. })
    ));

    let state = persistence.lock().unwrap().clone();
    assert_eq!(state.live, defaults());
    assert_eq!(state.durable, defaults());
    assert_eq!(
        (state.activations, state.commits, state.rollbacks),
        (1, 1, 1)
    );
    assert_eq!(
        lifecycle.observed_live.lock().unwrap().as_slice(),
        &[candidate]
    );
    assert_eq!(lifecycle.rollbacks.load(Ordering::SeqCst), 1);
    assert_eq!(notifications.load(Ordering::SeqCst), 0);
    assert_eq!(provider.read(context).await.unwrap().value, defaults());
}

#[tokio::test]
async fn successful_config_transaction_publishes_only_after_reload_and_commit() {
    let (service, provider, persistence, lifecycle) = transactional_service(false, false);
    let notifications = Arc::new(AtomicUsize::new(0));
    let notification_count = notifications.clone();
    service.subscribe_revision_changed(Arc::new(move |_| {
        notification_count.fetch_add(1, Ordering::SeqCst);
    }));
    let context = ConfigContext::plugin_instance("success");
    let candidate = changed_candidate("!");

    let result = service
        .apply(
            "discord",
            ConfigApplyRequest {
                candidate: candidate.clone(),
                expected_revision: ConfigRevision(1),
                dry_run: false,
            },
            context.clone(),
            &write_capabilities(),
        )
        .await
        .unwrap();

    assert!(result.actions.contains(&ConfigAction::PluginReloaded));
    assert!(result.pending_actions.is_empty());
    let state = persistence.lock().unwrap().clone();
    assert_eq!(state.live, candidate);
    assert_eq!(state.durable, candidate);
    assert_eq!(
        (state.activations, state.commits, state.rollbacks),
        (1, 1, 0)
    );
    assert_eq!(lifecycle.rollbacks.load(Ordering::SeqCst), 0);
    assert_eq!(notifications.load(Ordering::SeqCst), 1);
    let snapshot = provider.read(context).await.unwrap();
    assert_eq!(snapshot.revision, result.revision);
    assert_eq!(snapshot.value, candidate);
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
