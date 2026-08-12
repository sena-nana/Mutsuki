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
