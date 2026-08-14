use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use mutsuki_config_service::*;

#[derive(MutsukiConfig)]
#[config(provider_id = "example.settings", title = "Example")]
#[allow(dead_code)]
struct ExampleConfig {
    #[config(title = "Name", required)]
    name: String,
    #[config(title = "Mode", restart = "plugin_reload")]
    mode: String,
}

fn value(name: &str, mode: &str) -> ConfigValue {
    ConfigValue::Object(BTreeMap::from([
        ("name".into(), ConfigValue::String(name.into())),
        ("mode".into(), ConfigValue::String(mode.into())),
    ]))
}

fn service(repository: Arc<dyn ConfigRepository>) -> ConfigService {
    let registry = Arc::new(ConfigProviderRegistry::default());
    registry
        .register(Arc::new(MemoryConfigProvider::new(
            ExampleConfig::schema(),
            value("default", "safe"),
            ConfigApplyMode::HotReload,
        )))
        .unwrap();
    ConfigService::new(registry, repository).unwrap()
}

fn capabilities() -> Vec<String> {
    vec!["*".into()]
}

#[test]
fn derive_schema_round_trips_from_the_protocol_owner() {
    let schema = ExampleConfig::schema();
    assert_eq!(schema.provider_id.as_str(), "example.settings");
    let encoded = rmp_serde::to_vec_named(&schema).unwrap();
    let decoded: ConfigDescriptor = rmp_serde::from_slice(&encoded).unwrap();
    assert_eq!(decoded, schema);
}

#[test]
fn owned_provider_registration_cannot_remove_a_newer_owner() {
    let registry = Arc::new(ConfigProviderRegistry::default());
    let first = registry
        .register_owned(Arc::new(MemoryConfigProvider::new(
            ExampleConfig::schema(),
            value("first", "safe"),
            ConfigApplyMode::HotReload,
        )))
        .unwrap();
    let staged = registry
        .register_owned(Arc::new(MemoryConfigProvider::new(
            ExampleConfig::schema(),
            value("staged", "safe"),
            ConfigApplyMode::HotReload,
        )))
        .unwrap();
    drop(staged);
    assert_eq!(
        registry
            .get("example.settings")
            .unwrap()
            .provider
            .default_value(&ConfigContext::global())
            .unwrap(),
        value("first", "safe")
    );
    let second = registry
        .register_owned(Arc::new(MemoryConfigProvider::new(
            ExampleConfig::schema(),
            value("second", "safe"),
            ConfigApplyMode::HotReload,
        )))
        .unwrap();

    drop(first);
    assert_eq!(
        registry
            .get("example.settings")
            .unwrap()
            .provider
            .default_value(&ConfigContext::global())
            .unwrap(),
        value("second", "safe")
    );
    drop(second);
    assert!(registry.get("example.settings").is_err());
}

#[test]
fn staged_provider_stays_hidden_until_the_active_scope_retires() {
    let registry = Arc::new(ConfigProviderRegistry::default());
    let active = registry
        .register_owned(Arc::new(MemoryConfigProvider::new(
            ExampleConfig::schema(),
            value("active", "safe"),
            ConfigApplyMode::HotReload,
        )))
        .unwrap();
    let staged = registry
        .register_owned_staged(Arc::new(MemoryConfigProvider::new(
            ExampleConfig::schema(),
            value("staged", "safe"),
            ConfigApplyMode::HotReload,
        )))
        .unwrap();

    assert_eq!(
        registry
            .get("example.settings")
            .unwrap()
            .provider
            .default_value(&ConfigContext::plugin_instance("test"))
            .unwrap(),
        value("active", "safe")
    );
    drop(active);
    assert_eq!(
        registry
            .get("example.settings")
            .unwrap()
            .provider
            .default_value(&ConfigContext::plugin_instance("test"))
            .unwrap(),
        value("staged", "safe")
    );
    drop(staged);
    assert!(registry.get("example.settings").is_err());
}

#[tokio::test]
async fn preparing_a_provider_candidate_does_not_publish_it_before_scope_activation() {
    let registry = Arc::new(ConfigProviderRegistry::default());
    let _active = registry
        .register_owned(Arc::new(MemoryConfigProvider::new(
            ExampleConfig::schema(),
            value("active", "safe"),
            ConfigApplyMode::HotReload,
        )))
        .unwrap();
    let lifecycle = Arc::new(RecordingLifecycle {
        fail_execute: true,
        observations: Mutex::new(Vec::new()),
        rollbacks: AtomicUsize::new(0),
    });
    let service = ConfigService::new(
        registry.clone(),
        Arc::new(InMemoryConfigRepository::default()),
    )
    .unwrap()
    .with_lifecycle(lifecycle.clone());
    let candidate = Arc::new(MemoryConfigProvider::new(
        ExampleConfig::schema(),
        value("candidate", "safe"),
        ConfigApplyMode::HotReload,
    ));
    let context = ConfigContext::plugin_instance("candidate");

    let snapshot = service
        .prepare_provider_candidate(
            candidate.clone(),
            Some(value("seeded", "safe")),
            context.clone(),
        )
        .await
        .unwrap();
    assert_eq!(snapshot.value, value("seeded", "safe"));
    assert!(lifecycle.observations.lock().unwrap().is_empty());
    assert_eq!(
        registry
            .get("example.settings")
            .unwrap()
            .provider
            .default_value(&context)
            .unwrap(),
        value("active", "safe")
    );

    let candidate_registration = service.register_provider_owned(candidate).unwrap();
    assert_eq!(
        registry
            .get("example.settings")
            .unwrap()
            .provider
            .default_value(&context)
            .unwrap(),
        value("candidate", "safe")
    );
    drop(candidate_registration);
    assert_eq!(
        registry
            .get("example.settings")
            .unwrap()
            .provider
            .default_value(&context)
            .unwrap(),
        value("active", "safe")
    );
}

#[tokio::test]
async fn revision_subscription_stops_callbacks_after_dispose() {
    let service = service(Arc::new(InMemoryConfigRepository::default()));
    let notifications = Arc::new(AtomicUsize::new(0));
    let observed = notifications.clone();
    let mut subscription = service.subscribe_revision_changed(Arc::new(move |_| {
        observed.fetch_add(1, Ordering::SeqCst);
    }));
    let context = ConfigContext::plugin_instance("watch-test");
    service
        .apply(
            "example.settings",
            ConfigApplyRequest {
                candidate: value("first", "safe"),
                expected_revision: ConfigRevision::ABSENT,
                dry_run: false,
            },
            context.clone(),
            &capabilities(),
        )
        .await
        .unwrap();
    assert_eq!(notifications.load(Ordering::SeqCst), 1);

    assert!(subscription.dispose());
    assert!(!subscription.dispose());
    service
        .apply(
            "example.settings",
            ConfigApplyRequest {
                candidate: value("second", "safe"),
                expected_revision: ConfigRevision(1),
                dry_run: false,
            },
            context,
            &capabilities(),
        )
        .await
        .unwrap();
    assert_eq!(notifications.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn documents_have_independent_revisions_and_compare_and_set() {
    let service = service(Arc::new(InMemoryConfigRepository::default()));
    let first = ConfigContext::plugin_instance("first");
    let second = ConfigContext::plugin_instance("second");

    let initial = service
        .read("example.settings", first.clone(), &capabilities())
        .await
        .unwrap();
    assert_eq!(initial.revision, ConfigRevision::ABSENT);

    let applied = service
        .apply(
            "example.settings",
            ConfigApplyRequest {
                candidate: value("first", "fast"),
                expected_revision: initial.revision,
                dry_run: false,
            },
            first.clone(),
            &capabilities(),
        )
        .await
        .unwrap();
    assert_eq!(applied.revision, ConfigRevision(1));
    assert_eq!(
        service
            .read("example.settings", second, &capabilities())
            .await
            .unwrap()
            .revision,
        ConfigRevision::ABSENT
    );

    let stale = service
        .apply(
            "example.settings",
            ConfigApplyRequest {
                candidate: value("stale", "slow"),
                expected_revision: ConfigRevision::ABSENT,
                dry_run: false,
            },
            first,
            &capabilities(),
        )
        .await
        .unwrap_err();
    assert!(matches!(stale, ConfigError::RevisionConflict { .. }));
}

struct RecordingLifecycle {
    fail_execute: bool,
    observations: Mutex<Vec<(ConfigValue, ConfigValue)>>,
    rollbacks: AtomicUsize,
}

impl ConfigLifecycle for RecordingLifecycle {
    fn execute(
        &self,
        _provider_id: &str,
        current: &ConfigValue,
        candidate: &ConfigValue,
        _policy: RestartPolicy,
        pending: &[ConfigAction],
    ) -> Result<Vec<ConfigAction>, ConfigError> {
        self.observations
            .lock()
            .unwrap()
            .push((current.clone(), candidate.clone()));
        if self.fail_execute {
            Err(ConfigError::ReloadFailed {
                reason: "injected activation failure".into(),
            })
        } else {
            Ok(pending.to_vec())
        }
    }

    fn rollback(
        &self,
        _provider_id: &str,
        _current: &ConfigValue,
        _policy: RestartPolicy,
        _completed: &[ConfigAction],
    ) -> Result<(), ConfigError> {
        self.rollbacks.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn lifecycle_failure_preserves_the_committed_document() {
    let repository = Arc::new(InMemoryConfigRepository::default());
    let lifecycle = Arc::new(RecordingLifecycle {
        fail_execute: true,
        observations: Mutex::new(Vec::new()),
        rollbacks: AtomicUsize::new(0),
    });
    let service = service(repository.clone()).with_lifecycle(lifecycle.clone());
    let context = ConfigContext::plugin_instance("failed");

    let error = service
        .apply(
            "example.settings",
            ConfigApplyRequest {
                candidate: value("candidate", "fast"),
                expected_revision: ConfigRevision::ABSENT,
                dry_run: false,
            },
            context.clone(),
            &capabilities(),
        )
        .await
        .unwrap_err();

    assert!(matches!(error, ConfigError::ReloadFailed { .. }));
    assert!(
        repository
            .read(&ConfigDocumentKey::new("example.settings", context))
            .unwrap()
            .is_none()
    );
    assert_eq!(lifecycle.rollbacks.load(Ordering::SeqCst), 1);
}

struct FailingCommitRepository {
    inner: InMemoryConfigRepository,
}

impl ConfigRepository for FailingCommitRepository {
    fn read(&self, key: &ConfigDocumentKey) -> Result<Option<ConfigDocumentSnapshot>, ConfigError> {
        self.inner.read(key)
    }

    fn prepare_compare_and_set(
        &self,
        request: ConfigCompareAndSetRequest,
    ) -> Result<Box<dyn PreparedConfigWrite>, ConfigError> {
        Ok(Box::new(FailingCommitWrite {
            inner: self.inner.prepare_compare_and_set(request)?,
        }))
    }

    fn recover(&self) -> Result<(), ConfigError> {
        self.inner.recover()
    }
}

struct FailingCommitWrite {
    inner: Box<dyn PreparedConfigWrite>,
}

impl PreparedConfigWrite for FailingCommitWrite {
    fn set_commit_marker(&mut self, marker: Option<&std::path::Path>) -> Result<(), ConfigError> {
        self.inner.set_commit_marker(marker)
    }

    fn commit(&mut self) -> Result<ConfigDocumentSnapshot, ConfigError> {
        Err(ConfigError::PersistenceFailed {
            reason: "injected commit failure".into(),
        })
    }

    fn rollback(&mut self) -> Result<(), ConfigError> {
        self.inner.rollback()
    }

    fn finish(&mut self) -> Result<(), ConfigError> {
        self.inner.finish()
    }
}

#[tokio::test]
async fn repository_commit_failure_rolls_back_the_runtime_lifecycle() {
    let repository = Arc::new(FailingCommitRepository {
        inner: InMemoryConfigRepository::default(),
    });
    let lifecycle = Arc::new(RecordingLifecycle {
        fail_execute: false,
        observations: Mutex::new(Vec::new()),
        rollbacks: AtomicUsize::new(0),
    });
    let service = service(repository.clone()).with_lifecycle(lifecycle.clone());
    let context = ConfigContext::plugin_instance("commit-failure");

    let error = service
        .apply(
            "example.settings",
            ConfigApplyRequest {
                candidate: value("candidate", "fast"),
                expected_revision: ConfigRevision::ABSENT,
                dry_run: false,
            },
            context.clone(),
            &capabilities(),
        )
        .await
        .unwrap_err();

    assert!(matches!(error, ConfigError::PersistenceFailed { .. }));
    assert!(
        repository
            .read(&ConfigDocumentKey::new("example.settings", context))
            .unwrap()
            .is_none()
    );
    assert_eq!(lifecycle.rollbacks.load(Ordering::SeqCst), 1);
    let observations = lifecycle.observations.lock().unwrap();
    assert_eq!(
        observations[0],
        (value("default", "safe"), value("candidate", "fast"))
    );
}
