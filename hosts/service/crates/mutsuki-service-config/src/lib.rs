// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::doc_markdown,
    clippy::map_unwrap_or,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate
)]

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("configured service file does not exist: {path}")]
    MissingConfigFile { path: PathBuf },
    #[error("failed to read config file {path}: {source}")]
    ReadFile {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to parse config file {path}: {source}")]
    ParseFile {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("failed to read secret file {path}: {source}")]
    ReadSecretFile {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to parse secret file {path}: {source}")]
    ParseSecretFile {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("invalid secret file {path}: {detail}")]
    InvalidSecretFile { path: PathBuf, detail: String },
    #[error("failed to create directory {path}: {source}")]
    CreateDir {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("configured directory is not readable and writable {path}: {source}")]
    DirectoryAccess {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("control token is required; set MUTSUKI_CONTROL_TOKEN or configure [ipc].token")]
    MissingControlToken,
    #[error("Host secret rotation requires a configured secret_file")]
    SecretRotationUnavailable,
    #[error("Host secret {key} is controlled by environment variable {variable}")]
    SecretEnvironmentOverride { key: String, variable: String },
    #[error("Host secret {key} must not be empty")]
    InvalidSecretValue { key: String },
    #[error("failed to persist managed config file {path}: {source}")]
    WriteManagedFile {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("managed config journal failed for {path}: {detail}")]
    ConfigJournal { path: PathBuf, detail: String },
}

pub type ConfigResult<T> = Result<T, ConfigError>;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ServiceConfig {
    #[serde(default)]
    pub service: ServiceSection,
    #[serde(default)]
    pub core: CoreSection,
    #[serde(default)]
    pub ipc: IpcSection,
    #[serde(default)]
    pub plugins: PluginsSection,
    #[serde(default)]
    pub runners: RunnersSection,
    #[serde(default)]
    pub observe: ObserveSection,
    #[serde(default)]
    pub security: SecuritySection,
    #[serde(skip)]
    secret_store: SecretStore,
}

#[derive(Default)]
struct SecretStoreInner {
    entries: RwLock<BTreeMap<String, String>>,
    path: Option<PathBuf>,
    write_lock: Mutex<()>,
    transaction_active: Mutex<bool>,
}

#[derive(Clone, Default)]
pub struct SecretStore(Arc<SecretStoreInner>);

impl std::fmt::Debug for SecretStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SecretStore")
            .field(
                "entries",
                &self.0.entries.read().expect("secret store read lock").len(),
            )
            .finish_non_exhaustive()
    }
}

impl SecretStore {
    pub fn resolve(&self, key: &str) -> Option<String> {
        self.0
            .entries
            .read()
            .expect("secret store read lock")
            .get(&normalize_secret_key(key))
            .cloned()
    }

    fn rotate(&self, key: &str, value: String) -> ConfigResult<()> {
        let key = normalize_secret_key(key);
        if key.is_empty() || value.trim().is_empty() {
            return Err(ConfigError::InvalidSecretValue { key });
        }
        let path = self
            .0
            .path
            .clone()
            .ok_or(ConfigError::SecretRotationUnavailable)?;
        let mut reservation = SecretTransactionReservation::reserve(self.clone())?;
        let _write = self.0.write_lock.lock().expect("secret store write lock");
        let content = fs::read_to_string(&path).map_err(|source| ConfigError::ReadSecretFile {
            path: path.clone(),
            source,
        })?;
        let mut file: SecretFile =
            toml::from_str(&content).map_err(|source| ConfigError::ParseSecretFile {
                path: path.clone(),
                source,
            })?;
        file.secrets
            .retain(|candidate, _| normalize_secret_key(candidate) != key);
        file.secrets.insert(key.clone(), value);
        let content =
            toml::to_string_pretty(&file).map_err(|source| ConfigError::InvalidSecretFile {
                path: path.clone(),
                detail: source.to_string(),
            })?;
        atomic_write(&path, content.as_bytes(), true)?;
        let entries = validate_secret_entries(&path, file.secrets)?;
        *self.0.entries.write().expect("secret store write lock") = entries;
        reservation.release();
        Ok(())
    }
}

struct SecretTransactionReservation {
    store: SecretStore,
    finished: bool,
}

impl SecretTransactionReservation {
    fn reserve(store: SecretStore) -> ConfigResult<Self> {
        let mut active = store
            .0
            .transaction_active
            .lock()
            .expect("secret transaction lock");
        if *active {
            return Err(ConfigError::ConfigJournal {
                path: store
                    .0
                    .path
                    .clone()
                    .unwrap_or_else(|| PathBuf::from("secret_file")),
                detail: "another secret transaction is already prepared".into(),
            });
        }
        *active = true;
        drop(active);
        Ok(Self {
            store,
            finished: false,
        })
    }

    fn release(&mut self) {
        *self
            .store
            .0
            .transaction_active
            .lock()
            .expect("secret transaction lock") = false;
        self.finished = true;
    }
}

impl Drop for SecretTransactionReservation {
    fn drop(&mut self) {
        if !self.finished {
            self.release();
        }
    }
}

#[derive(Clone)]
pub struct HostSecretStore {
    store: SecretStore,
    env_prefix: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ConfigJournalPhase {
    Prepared,
    Committing,
    Committed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SecretChangeJournal {
    path: PathBuf,
    previous: String,
    candidate: String,
}

struct PreparedSecretChange {
    journal: SecretChangeJournal,
    previous_entries: BTreeMap<String, String>,
    candidate_entries: BTreeMap<String, String>,
    reservation: SecretTransactionReservation,
}

pub struct PreparedHostSecretTransaction {
    journal_path: PathBuf,
    journal: StandaloneSecretJournal,
    change: Option<PreparedSecretChange>,
    activated: bool,
    committed: bool,
    finished: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct StandaloneSecretJournal {
    version: u32,
    phase: ConfigJournalPhase,
    secret: SecretChangeJournal,
    #[serde(default)]
    commit_marker: Option<PathBuf>,
}

impl std::fmt::Debug for HostSecretStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HostSecretStore")
            .field("store", &self.store)
            .finish_non_exhaustive()
    }
}

impl HostSecretStore {
    pub fn rotation_available(&self) -> bool {
        self.store.0.path.is_some()
    }

    pub fn resolve(&self, key: &str) -> Option<String> {
        let key = normalize_secret_key(key);
        env::var(format!("{}{key}", self.env_prefix))
            .ok()
            .or_else(|| self.store.resolve(&key))
    }

    /// Atomically persists a rotated secret in the configured Host secret file.
    /// Environment-backed secrets are intentionally immutable at runtime.
    pub fn rotate(&self, key: &str, value: String) -> ConfigResult<()> {
        let key = normalize_secret_key(key);
        let variable = format!("{}{key}", self.env_prefix);
        if env::var_os(&variable).is_some() {
            return Err(ConfigError::SecretEnvironmentOverride { key, variable });
        }
        self.store.rotate(&key, value)
    }

    /// Prepares one recoverable secret-file transaction without exposing plaintext on read.
    pub fn prepare_transaction(
        &self,
        updates: BTreeMap<String, String>,
    ) -> ConfigResult<PreparedHostSecretTransaction> {
        self.prepare_mutations(
            updates
                .into_iter()
                .map(|(key, value)| (key, Some(value)))
                .collect(),
        )
    }

    /// Prepares set/clear mutations under the same recoverable Host secret journal.
    /// `None` removes the named file-backed secret. Environment-backed keys remain immutable.
    pub fn prepare_mutations(
        &self,
        updates: BTreeMap<String, Option<String>>,
    ) -> ConfigResult<PreparedHostSecretTransaction> {
        let change = self.prepare_change(updates)?;
        let Some(change) = change else {
            return Ok(PreparedHostSecretTransaction {
                journal_path: PathBuf::new(),
                journal: StandaloneSecretJournal {
                    version: 2,
                    phase: ConfigJournalPhase::Prepared,
                    secret: SecretChangeJournal {
                        path: PathBuf::new(),
                        previous: String::new(),
                        candidate: String::new(),
                    },
                    commit_marker: None,
                },
                change: None,
                activated: false,
                committed: false,
                finished: false,
            });
        };
        let journal_path = secret_journal_path(&change.journal.path);
        if journal_path.exists() {
            return Err(ConfigError::ConfigJournal {
                path: journal_path,
                detail: "unfinished secret transaction requires recovery".into(),
            });
        }
        let journal = StandaloneSecretJournal {
            version: 2,
            phase: ConfigJournalPhase::Prepared,
            secret: change.journal.clone(),
            commit_marker: None,
        };
        write_secret_journal(&journal_path, &journal)?;
        Ok(PreparedHostSecretTransaction {
            journal_path,
            journal,
            change: Some(change),
            activated: false,
            committed: false,
            finished: false,
        })
    }

    fn prepare_change(
        &self,
        updates: BTreeMap<String, Option<String>>,
    ) -> ConfigResult<Option<PreparedSecretChange>> {
        if updates.is_empty() {
            return Ok(None);
        }
        let mut normalized = BTreeMap::new();
        for (raw_key, value) in updates {
            let key = normalize_secret_key(&raw_key);
            if key.is_empty()
                || value
                    .as_deref()
                    .is_some_and(|candidate| candidate.trim().is_empty())
            {
                return Err(ConfigError::InvalidSecretValue { key });
            }
            let variable = format!("{}{key}", self.env_prefix);
            if env::var_os(&variable).is_some() {
                return Err(ConfigError::SecretEnvironmentOverride { key, variable });
            }
            normalized.insert(key, value);
        }
        let path = self
            .store
            .0
            .path
            .clone()
            .ok_or(ConfigError::SecretRotationUnavailable)?;
        let reservation = SecretTransactionReservation::reserve(self.store.clone())?;
        let previous = fs::read_to_string(&path).map_err(|source| ConfigError::ReadSecretFile {
            path: path.clone(),
            source,
        })?;
        let mut file: SecretFile =
            toml::from_str(&previous).map_err(|source| ConfigError::ParseSecretFile {
                path: path.clone(),
                source,
            })?;
        let previous_entries = validate_secret_entries(&path, file.secrets.clone())?;
        for (key, value) in normalized {
            file.secrets
                .retain(|candidate, _| normalize_secret_key(candidate) != key);
            if let Some(value) = value {
                file.secrets.insert(key, value);
            }
        }
        let candidate_entries = validate_secret_entries(&path, file.secrets.clone())?;
        let candidate =
            toml::to_string_pretty(&file).map_err(|source| ConfigError::InvalidSecretFile {
                path: path.clone(),
                detail: source.to_string(),
            })?;
        Ok(Some(PreparedSecretChange {
            journal: SecretChangeJournal {
                path,
                previous,
                candidate,
            },
            previous_entries,
            candidate_entries,
            reservation,
        }))
    }
}

impl PreparedHostSecretTransaction {
    /// Opts this secret transaction into a shared commit decision used by ConfigService.
    /// The returned marker must also be recorded by the configuration repository.
    pub fn enable_coordinated_commit(&mut self) -> ConfigResult<Option<PathBuf>> {
        if self.change.is_none() {
            return Ok(None);
        }
        if let Some(marker) = &self.journal.commit_marker {
            return Ok(Some(marker.clone()));
        }
        let marker = self.journal_path.with_extension("commit");
        if marker.exists() {
            return Err(ConfigError::ConfigJournal {
                path: marker,
                detail: "unfinished coordinated transaction requires recovery".into(),
            });
        }
        self.journal.commit_marker = Some(marker.clone());
        write_secret_journal(&self.journal_path, &self.journal)?;
        Ok(Some(marker))
    }

    #[must_use]
    pub fn commit_marker(&self) -> Option<&Path> {
        self.journal.commit_marker.as_deref()
    }

    pub fn activate(&mut self) -> ConfigResult<()> {
        let Some(change) = &mut self.change else {
            self.activated = true;
            return Ok(());
        };
        if self.activated {
            return Ok(());
        }
        self.journal.phase = ConfigJournalPhase::Committing;
        write_secret_journal(&self.journal_path, &self.journal)?;
        atomic_write(
            &change.journal.path,
            change.journal.candidate.as_bytes(),
            true,
        )?;
        *change
            .reservation
            .store
            .0
            .entries
            .write()
            .expect("secret store write lock") = change.candidate_entries.clone();
        self.activated = true;
        Ok(())
    }

    pub fn commit(&mut self) -> ConfigResult<()> {
        self.activate()?;
        if let Some(change) = &mut self.change {
            self.journal.phase = ConfigJournalPhase::Committed;
            write_secret_journal(&self.journal_path, &self.journal)?;
            if let Some(marker) = &self.journal.commit_marker {
                atomic_write(marker, b"committed\n", true)?;
                self.committed = true;
                return Ok(());
            }
            remove_journal(&self.journal_path)?;
            change.reservation.release();
        }
        self.committed = true;
        self.finished = true;
        Ok(())
    }

    /// Releases the secret-side recovery journal after the shared marker is durable.
    /// The repository removes the marker only after its pending record is gone.
    pub fn finish(&mut self) -> ConfigResult<()> {
        if let Some(change) = &mut self.change {
            remove_journal(&self.journal_path)?;
            change.reservation.release();
        }
        self.finished = true;
        Ok(())
    }

    pub fn rollback(&mut self) -> ConfigResult<()> {
        if let Some(change) = &mut self.change {
            if self
                .journal
                .commit_marker
                .as_deref()
                .is_some_and(Path::exists)
            {
                remove_journal(&self.journal_path)?;
                change.reservation.release();
                self.finished = true;
                return Ok(());
            }
            if self.activated {
                atomic_write(
                    &change.journal.path,
                    change.journal.previous.as_bytes(),
                    true,
                )?;
                *change
                    .reservation
                    .store
                    .0
                    .entries
                    .write()
                    .expect("secret store write lock") = change.previous_entries.clone();
            }
            remove_journal(&self.journal_path)?;
            change.reservation.release();
        }
        self.finished = true;
        Ok(())
    }
}

impl Drop for PreparedHostSecretTransaction {
    fn drop(&mut self) {
        if !self.finished {
            if self.committed {
                let _ = self.finish();
            } else {
                let _ = self.rollback();
            }
        }
    }
}

fn remove_journal(path: &Path) -> ConfigResult<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(ConfigError::ConfigJournal {
            path: path.to_path_buf(),
            detail: source.to_string(),
        }),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ServiceSection {
    pub profile: String,
    pub instance_id: String,
    pub home_dir: PathBuf,
    pub data_dir: PathBuf,
    pub log_dir: PathBuf,
    pub plugin_dir: PathBuf,
    pub run_dir: PathBuf,
}

impl Default for ServiceSection {
    fn default() -> Self {
        let home = default_home_dir();
        Self {
            profile: "default".into(),
            instance_id: "default".into(),
            home_dir: home,
            data_dir: PathBuf::from("data"),
            log_dir: PathBuf::from("logs"),
            plugin_dir: PathBuf::from("plugins"),
            run_dir: PathBuf::from("run"),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct CoreSection {
    pub max_tasks: usize,
    pub shutdown_timeout_ms: u64,
    pub worker_profile: WorkerProfile,
    pub worker_threads: Option<usize>,
    pub blocking_threads: Option<usize>,
    pub pool_queue_limit: Option<usize>,
    pub pool_max_inflight_bytes: Option<usize>,
    pub max_isolated_workers: Option<usize>,
    pub runner_wall_clock_timeout_ms: Option<u64>,
    pub cancel_grace_period_ms: Option<u64>,
    pub worker_health_timeout_ms: Option<u64>,
    /// Optional physical execution topology. Empty preserves the legacy
    /// compute/blocking worker pools selected by `worker_profile`.
    pub execution_domains: Vec<ExecutionDomainSection>,
    pub actor_control_queue_limit: Option<usize>,
    pub actor_data_queue_limit: Option<usize>,
    pub actor_control_quota: Option<usize>,
}

impl Default for CoreSection {
    fn default() -> Self {
        Self {
            max_tasks: 4096,
            shutdown_timeout_ms: 30_000,
            worker_profile: WorkerProfile::Desktop,
            worker_threads: None,
            blocking_threads: None,
            pool_queue_limit: None,
            pool_max_inflight_bytes: None,
            max_isolated_workers: None,
            runner_wall_clock_timeout_ms: None,
            cancel_grace_period_ms: Some(30_000),
            worker_health_timeout_ms: None,
            execution_domains: Vec::new(),
            actor_control_queue_limit: None,
            actor_data_queue_limit: None,
            actor_control_quota: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ExecutionDomainSection {
    pub id: String,
    pub execution_classes: Vec<ExecutionClassName>,
    pub threads: usize,
    pub queue_capacity: usize,
    pub max_inflight_bytes: usize,
    pub max_isolated_threads: usize,
    pub lanes: BTreeMap<DispatchLaneName, LanePolicySection>,
}

impl Default for ExecutionDomainSection {
    fn default() -> Self {
        Self {
            id: String::new(),
            execution_classes: Vec::new(),
            threads: 1,
            queue_capacity: 1_024,
            max_inflight_bytes: 64 * 1024 * 1024,
            max_isolated_threads: 2,
            lanes: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionClassName {
    Orchestration,
    Io,
    Cpu,
    Blocking,
    Script,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum DispatchLaneName {
    Control,
    Interactive,
    Normal,
    Background,
    Bulk,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct LanePolicySection {
    pub weight: Option<usize>,
    pub reserved_entries: Option<usize>,
    pub max_share_percent: Option<u8>,
    pub queue_entry_limit: Option<usize>,
    pub max_inflight_bytes: Option<usize>,
    pub starvation_steps: Option<u64>,
    pub allow_idle_borrow: Option<bool>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum WorkerProfile {
    LowResource,
    #[default]
    Desktop,
    Server,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerPoolSettings {
    pub compute_threads: usize,
    pub blocking_threads: usize,
    pub queue_capacity: usize,
    pub max_inflight_bytes: usize,
    pub max_isolated_workers: usize,
}

impl CoreSection {
    pub fn worker_pool_settings(&self) -> WorkerPoolSettings {
        let parallelism = std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(2)
            .max(1);
        let profile = match self.worker_profile {
            WorkerProfile::LowResource => WorkerPoolSettings {
                compute_threads: 1,
                blocking_threads: 1,
                queue_capacity: 64,
                max_inflight_bytes: 8 * 1024 * 1024,
                max_isolated_workers: 1,
            },
            WorkerProfile::Desktop => WorkerPoolSettings {
                compute_threads: parallelism,
                blocking_threads: 2,
                queue_capacity: 256,
                max_inflight_bytes: 64 * 1024 * 1024,
                max_isolated_workers: 1,
            },
            WorkerProfile::Server => {
                let blocking_threads = (parallelism / 4).clamp(2, 8);
                WorkerPoolSettings {
                    compute_threads: parallelism,
                    blocking_threads,
                    queue_capacity: 1024,
                    max_inflight_bytes: 256 * 1024 * 1024,
                    max_isolated_workers: blocking_threads,
                }
            }
        };
        WorkerPoolSettings {
            compute_threads: self.worker_threads.unwrap_or(profile.compute_threads),
            blocking_threads: self.blocking_threads.unwrap_or(profile.blocking_threads),
            queue_capacity: self.pool_queue_limit.unwrap_or(profile.queue_capacity),
            max_inflight_bytes: self
                .pool_max_inflight_bytes
                .unwrap_or(profile.max_inflight_bytes),
            max_isolated_workers: self
                .max_isolated_workers
                .unwrap_or(profile.max_isolated_workers),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct IpcSection {
    pub enabled: bool,
    pub transport: IpcTransport,
    pub codec: IpcCodec,
    pub name: String,
    pub token: Option<String>,
    pub tcp_debug_addr: Option<String>,
    pub max_frame_bytes: usize,
    pub max_payload_bytes: usize,
    pub max_in_flight: usize,
    pub idle_timeout_ms: u64,
    pub request_timeout_ms: u64,
}

/// Hand-written so the control token never reaches a log line. `ServiceConfig` derives `Debug`,
/// and it is routinely dumped on startup and in diagnostics, which would otherwise print the
/// credential that grants full control-plane authority in plaintext.
impl std::fmt::Debug for IpcSection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IpcSection")
            .field("enabled", &self.enabled)
            .field("transport", &self.transport)
            .field("codec", &self.codec)
            .field("name", &self.name)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .field("tcp_debug_addr", &self.tcp_debug_addr)
            .field("max_frame_bytes", &self.max_frame_bytes)
            .field("max_payload_bytes", &self.max_payload_bytes)
            .field("max_in_flight", &self.max_in_flight)
            .field("idle_timeout_ms", &self.idle_timeout_ms)
            .field("request_timeout_ms", &self.request_timeout_ms)
            .finish()
    }
}

impl Default for IpcSection {
    fn default() -> Self {
        Self {
            enabled: true,
            transport: default_transport(),
            codec: IpcCodec::Binary,
            name: "mutsuki-service-default".into(),
            token: None,
            tcp_debug_addr: None,
            max_frame_bytes: 1024 * 1024,
            max_payload_bytes: 512 * 1024,
            max_in_flight: 64,
            idle_timeout_ms: 60_000,
            request_timeout_ms: 30_000,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum IpcTransport {
    NamedPipe,
    UnixSocket,
    TcpDebug,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum IpcCodec {
    #[default]
    Binary,
}

impl Default for IpcTransport {
    fn default() -> Self {
        default_transport()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PluginsSection {
    #[serde(default)]
    pub configured: Vec<ConfiguredPluginSelection>,
    pub dynamic_dirs: Vec<PathBuf>,
    pub disabled_dir: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfiguredPluginSelection {
    pub id: String,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    #[serde(default)]
    pub config: serde_json::Value,
}

fn enabled_by_default() -> bool {
    true
}

impl Default for PluginsSection {
    fn default() -> Self {
        let home = default_home_dir();
        Self {
            configured: Vec::new(),
            dynamic_dirs: vec![home.join("plugins").join("installed")],
            disabled_dir: home.join("plugins").join("disabled"),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct RunnersSection {
    pub restart: bool,
    pub max_restart_per_minute: u32,
    pub graceful_shutdown_ms: u64,
    pub env_allowlist: Vec<String>,
}

impl Default for RunnersSection {
    fn default() -> Self {
        Self {
            restart: true,
            max_restart_per_minute: 5,
            graceful_shutdown_ms: 5_000,
            env_allowlist: default_env_allowlist(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ObserveSection {
    pub console: bool,
    pub json: bool,
    pub log_file: String,
    pub panic_file: String,
}

impl Default for ObserveSection {
    fn default() -> Self {
        Self {
            console: true,
            json: false,
            log_file: "service.log".into(),
            panic_file: "panic.log".into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SecuritySection {
    pub control_token_env: String,
    pub secret_env_prefix: String,
    pub secret_file: Option<PathBuf>,
}

impl Default for SecuritySection {
    fn default() -> Self {
        Self {
            control_token_env: "MUTSUKI_CONTROL_TOKEN".into(),
            secret_env_prefix: "MUTSUKI_SECRET_".into(),
            secret_file: None,
        }
    }
}

#[derive(Clone, Default)]
pub struct ConfigOverrides {
    pub profile: Option<String>,
    pub config_file: Option<PathBuf>,
    pub home_dir: Option<PathBuf>,
    pub control_token: Option<String>,
}

/// Carries the CLI `--token`, so it is redacted for the same reason as [`IpcSection`].
impl std::fmt::Debug for ConfigOverrides {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ConfigOverrides")
            .field("profile", &self.profile)
            .field("config_file", &self.config_file)
            .field("home_dir", &self.home_dir)
            .field(
                "control_token",
                &self.control_token.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl ServiceConfig {
    /// Finalizes a candidate loaded from ConfigRepository using only bootstrap
    /// directory and secret boundaries. It never reads product configuration.
    pub fn finalize_bootstrap(
        mut self,
        bootstrap_file: &Path,
        control_token: Option<String>,
    ) -> ConfigResult<Self> {
        if let Some(token) = control_token {
            self.ipc.token = Some(token);
        }
        self.apply_env();
        self.load_secret_file(bootstrap_file)?;
        self.resolve_relative_dirs();
        self.ensure_dirs()?;
        self.ensure_control_token()?;
        Ok(self)
    }

    /// Resolves a named secret through the Host environment boundary.
    ///
    /// The returned value must remain inside product assembly and effectful
    /// adapters; task payloads and ordinary configuration should only store
    /// the key passed to this method.
    pub fn secret(&self, key: &str) -> Option<String> {
        self.host_secret_store().resolve(key)
    }

    /// Snapshot used only by Host boundaries that must resolve secret keys
    /// after service configuration has been loaded.
    pub fn secret_store(&self) -> SecretStore {
        self.secret_store.clone()
    }

    /// Host-owned secret boundary for runtime credential resolution and rotation.
    pub fn host_secret_store(&self) -> HostSecretStore {
        HostSecretStore {
            store: self.secret_store.clone(),
            env_prefix: self.security.secret_env_prefix.clone(),
        }
    }

    pub fn load(overrides: ConfigOverrides) -> ConfigResult<Self> {
        let mut config = Self::default();
        if let Some(home) = overrides.home_dir {
            config.service.home_dir = home;
        } else if let Ok(home) = env::var("MUTSUKI_HOME") {
            config.service.home_dir = PathBuf::from(home);
        }
        config.resolve_relative_dirs();

        let explicit_config_file = overrides.config_file.is_some();
        let local_file = overrides
            .config_file
            .clone()
            .unwrap_or_else(|| config.service.home_dir.join("config").join("service.toml"));
        if explicit_config_file && !local_file.is_file() {
            return Err(ConfigError::MissingConfigFile { path: local_file });
        }
        let local_profile = read_optional_config(&local_file)?
            .map(|file_config| file_config.service.profile)
            .filter(|profile| !profile.is_empty());
        if let Some(profile) = overrides
            .profile
            .clone()
            .or_else(|| env::var("MUTSUKI_PROFILE").ok())
            .or(local_profile)
        {
            config.service.profile = profile;
        }

        let profile_file = config
            .service
            .home_dir
            .join("config")
            .join("profiles")
            .join(format!("{}.toml", config.service.profile));
        if let Some(profile_config) = read_optional_config(&profile_file)? {
            config.merge(profile_config);
        }
        if let Some(local_config) = read_optional_config(&local_file)? {
            config.merge(local_config);
        }
        config.apply_env();
        if let Some(profile) = overrides.profile {
            config.service.profile = profile;
        }
        if let Some(token) = overrides.control_token {
            config.ipc.token = Some(token);
        }
        config.load_secret_file(&local_file)?;
        config.resolve_relative_dirs();
        config.ensure_dirs()?;
        config.ensure_control_token()?;
        Ok(config)
    }

    fn load_secret_file(&mut self, local_file: &Path) -> ConfigResult<()> {
        let Some(configured_path) = self.security.secret_file.clone() else {
            self.secret_store = SecretStore::default();
            return Ok(());
        };
        let path = if configured_path.is_absolute() {
            configured_path
        } else {
            local_file
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(configured_path)
        };
        recover_secret_journal(&path)?;
        let content = fs::read_to_string(&path).map_err(|source| ConfigError::ReadSecretFile {
            path: path.clone(),
            source,
        })?;
        let file: SecretFile =
            toml::from_str(&content).map_err(|source| ConfigError::ParseSecretFile {
                path: path.clone(),
                source,
            })?;
        let secrets = validate_secret_entries(&path, file.secrets)?;
        self.security.secret_file = Some(path);
        self.secret_store = SecretStore(Arc::new(SecretStoreInner {
            entries: RwLock::new(secrets),
            path: self.security.secret_file.clone(),
            write_lock: Mutex::new(()),
            transaction_active: Mutex::new(false),
        }));
        Ok(())
    }

    pub fn control_token(&self) -> &str {
        self.ipc
            .token
            .as_deref()
            .expect("config validated control token")
    }

    pub fn ipc_endpoint(&self) -> String {
        match self.ipc.transport {
            IpcTransport::NamedPipe => self.ipc.name.clone(),
            IpcTransport::UnixSocket => self
                .service
                .run_dir
                .join(format!("{}.sock", self.ipc.name))
                .to_string_lossy()
                .into_owned(),
            IpcTransport::TcpDebug => self
                .ipc
                .tcp_debug_addr
                .clone()
                .unwrap_or_else(|| "127.0.0.1:7687".into()),
        }
    }

    fn merge(&mut self, other: ServiceConfig) {
        self.service = other.service;
        self.core = other.core;
        self.ipc = other.ipc;
        self.plugins = other.plugins;
        self.runners = other.runners;
        self.observe = other.observe;
        self.security = other.security;
    }

    fn apply_env(&mut self) {
        if let Ok(instance) = env::var("MUTSUKI_INSTANCE_ID") {
            self.service.instance_id = instance;
        }
        if let Ok(token) = env::var(&self.security.control_token_env) {
            self.ipc.token = Some(token);
        }
        if let Ok(transport) = env::var("MUTSUKI_IPC_TRANSPORT") {
            self.ipc.transport = match transport.as_str() {
                "named-pipe" => IpcTransport::NamedPipe,
                "unix-socket" => IpcTransport::UnixSocket,
                "tcp-debug" => IpcTransport::TcpDebug,
                _ => self.ipc.transport.clone(),
            };
        }
    }

    fn resolve_relative_dirs(&mut self) {
        let home = self.service.home_dir.clone();
        self.service.data_dir = absolutize(&home, &self.service.data_dir);
        self.service.log_dir = absolutize(&home, &self.service.log_dir);
        self.service.plugin_dir = absolutize(&home, &self.service.plugin_dir);
        self.service.run_dir = absolutize(&home, &self.service.run_dir);
        self.plugins.dynamic_dirs = self
            .plugins
            .dynamic_dirs
            .iter()
            .map(|path| absolutize(&home, path))
            .collect();
        self.plugins.disabled_dir = absolutize(&home, &self.plugins.disabled_dir);
    }

    fn ensure_dirs(&self) -> ConfigResult<()> {
        for path in [
            &self.service.home_dir,
            &self.service.data_dir,
            &self.service.log_dir,
            &self.service.plugin_dir,
            &self.service.run_dir,
        ] {
            fs::create_dir_all(path).map_err(|source| ConfigError::CreateDir {
                path: path.clone(),
                source,
            })?;
            verify_directory_access(path)?;
        }
        Ok(())
    }

    fn ensure_control_token(&mut self) -> ConfigResult<()> {
        if self.ipc.token.is_some() {
            return Ok(());
        }
        if env::var("MUTSUKI_TEST_ALLOW_EMPTY_TOKEN").as_deref() == Ok("1") {
            self.ipc.token = Some(String::new());
            return Ok(());
        }
        let token_path = self.service.run_dir.join("control.token");
        if let Ok(token) = fs::read_to_string(&token_path) {
            let token = token.trim().to_string();
            if !token.is_empty() {
                self.ipc.token = Some(token);
                return Ok(());
            }
        }
        let generated = generate_local_token();
        write_control_token(&token_path, &generated)?;
        self.ipc.token = Some(generated);
        Ok(())
    }
}

/// Writes the control token so that only the owner can read it back.
///
/// The mode is applied at creation rather than with a follow-up `set_permissions`, because the
/// window between the two would expose the token to every local user on a default `umask`.
pub fn write_control_token(path: &Path, token: &str) -> ConfigResult<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|source| ConfigError::CreateDir {
            path: path.to_path_buf(),
            source,
        })?;
    file.write_all(token.as_bytes())
        .map_err(|source| ConfigError::CreateDir {
            path: path.to_path_buf(),
            source,
        })?;
    // An existing file keeps its old mode through `create(true)`, so tighten it explicitly.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|source| {
            ConfigError::CreateDir {
                path: path.to_path_buf(),
                source,
            }
        })?;
    }
    Ok(())
}

fn secret_journal_path(path: &Path) -> PathBuf {
    path.with_extension("mutsuki-secret-transaction.toml")
}

fn write_secret_journal(path: &Path, journal: &StandaloneSecretJournal) -> ConfigResult<()> {
    let bytes = toml::to_string(journal).map_err(|source| ConfigError::ConfigJournal {
        path: path.to_path_buf(),
        detail: source.to_string(),
    })?;
    atomic_write(path, bytes.as_bytes(), true)
}

fn recover_secret_journal(secret_path: &Path) -> ConfigResult<()> {
    let journal_path = secret_journal_path(secret_path);
    if !journal_path.exists() {
        return Ok(());
    }
    let content =
        fs::read_to_string(&journal_path).map_err(|source| ConfigError::ConfigJournal {
            path: journal_path.clone(),
            detail: source.to_string(),
        })?;
    let journal: StandaloneSecretJournal =
        toml::from_str(&content).map_err(|source| ConfigError::ConfigJournal {
            path: journal_path.clone(),
            detail: source.to_string(),
        })?;
    if !matches!(journal.version, 1 | 2) || journal.secret.path != secret_path {
        return Err(ConfigError::ConfigJournal {
            path: journal_path,
            detail: "invalid secret transaction journal".into(),
        });
    }
    let recovered = match journal.commit_marker.as_deref() {
        Some(marker) if marker.exists() => journal.secret.candidate,
        Some(_) => journal.secret.previous,
        None => match journal.phase {
            ConfigJournalPhase::Prepared | ConfigJournalPhase::Committing => {
                journal.secret.previous
            }
            ConfigJournalPhase::Committed => journal.secret.candidate,
        },
    };
    atomic_write(secret_path, recovered.as_bytes(), true)?;
    remove_journal(&journal_path)
}

/// Recovers a coordinated Host secret transaction before its paired config repository opens.
/// Applications using a separate repository must call this first so the shared marker remains
/// available to both recovery participants.
pub fn recover_host_secret_transaction(secret_path: &Path) -> ConfigResult<()> {
    recover_secret_journal(secret_path)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SecretFile {
    secrets: BTreeMap<String, String>,
}

fn validate_secret_entries(
    path: &Path,
    entries: BTreeMap<String, String>,
) -> ConfigResult<BTreeMap<String, String>> {
    let mut secrets = BTreeMap::new();
    for (raw_key, value) in entries {
        let key = normalize_secret_key(&raw_key);
        if key.is_empty() {
            return Err(ConfigError::InvalidSecretFile {
                path: path.to_path_buf(),
                detail: "secret key must not be empty".into(),
            });
        }
        if value.trim().is_empty() {
            return Err(ConfigError::InvalidSecretFile {
                path: path.to_path_buf(),
                detail: format!("secret {raw_key} must not be empty"),
            });
        }
        if secrets.insert(key.clone(), value).is_some() {
            return Err(ConfigError::InvalidSecretFile {
                path: path.to_path_buf(),
                detail: format!("duplicate normalized secret key {key}"),
            });
        }
    }
    Ok(secrets)
}

fn atomic_write(path: &Path, bytes: &[u8], secret: bool) -> ConfigResult<()> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let temp = path.with_extension(format!("mutsuki-{nonce:x}.tmp"));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)
            .map_err(|source| ConfigError::WriteManagedFile {
                path: temp.clone(),
                source,
            })?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|source| ConfigError::WriteManagedFile {
                path: temp.clone(),
                source,
            })?;
        #[cfg(unix)]
        if secret {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&temp, fs::Permissions::from_mode(0o600)).map_err(|source| {
                ConfigError::WriteManagedFile {
                    path: temp.clone(),
                    source,
                }
            })?;
            file.sync_all()
                .map_err(|source| ConfigError::WriteManagedFile {
                    path: temp.clone(),
                    source,
                })?;
        }
        #[cfg(not(unix))]
        let _ = secret;
        fs::rename(&temp, path).map_err(|source| ConfigError::WriteManagedFile {
            path: path.to_path_buf(),
            source,
        })?;
        #[cfg(not(windows))]
        if let Some(parent) = path.parent() {
            fs::File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(|source| ConfigError::WriteManagedFile {
                    path: parent.to_path_buf(),
                    source,
                })?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

fn normalize_secret_key(key: &str) -> String {
    key.trim()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

fn verify_directory_access(path: &Path) -> ConfigResult<()> {
    let access_error = |source| ConfigError::DirectoryAccess {
        path: path.to_path_buf(),
        source,
    };
    fs::read_dir(path).map_err(&access_error)?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let probe = path.join(format!(
        ".mutsuki-access-probe-{}-{nonce:x}",
        std::process::id()
    ));
    fs::write(&probe, []).map_err(&access_error)?;
    fs::remove_file(probe).map_err(access_error)
}

fn default_home_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".mutsuki")
}

fn read_optional_config(path: &Path) -> ConfigResult<Option<ServiceConfig>> {
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(path).map_err(|source| ConfigError::ReadFile {
        path: path.to_path_buf(),
        source,
    })?;
    let config = toml::from_str(&content).map_err(|source| ConfigError::ParseFile {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(Some(config))
}

fn absolutize(home: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        home.join(path)
    }
}

fn default_env_allowlist() -> Vec<String> {
    #[cfg(windows)]
    let mut vars = vec!["PATH".to_string()];
    #[cfg(not(windows))]
    let vars = vec!["PATH".to_string()];
    #[cfg(windows)]
    {
        vars.extend(["SystemRoot".into(), "WINDIR".into(), "COMSPEC".into()]);
    }
    vars
}

fn default_transport() -> IpcTransport {
    #[cfg(windows)]
    {
        IpcTransport::NamedPipe
    }
    #[cfg(not(windows))]
    {
        IpcTransport::UnixSocket
    }
}

/// Mints the control token used when the operator did not configure one.
///
/// The value must not be derivable from observable process state: the control socket accepts any
/// local caller that presents it, so a clock- or pid-derived token has a search space small enough
/// to brute force. `Uuid::new_v4` draws from the OS CSPRNG.
fn generate_local_token() -> String {
    format!("local-{}", Uuid::new_v4().simple())
}

/// Mints a per-Runner session token.
///
/// This is deliberately unrelated to the control token: a sidecar receives it through the
/// environment, so reusing the control token here would hand every Runner process full
/// control-plane authority. The supervisor re-mints one on every spawn, so a leaked token dies
/// with the process that leaked it.
#[must_use]
pub fn generate_runner_session_token() -> String {
    format!("runner-{}", Uuid::new_v4().simple())
}

pub fn filtered_environment(
    allowlist: &[String],
    extra: BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut envs = BTreeMap::new();
    for key in allowlist {
        if let Ok(value) = env::var(key) {
            envs.insert(key.clone(), value);
        }
    }
    envs.extend(extra);
    envs
}

// Secret precedence is defined against the process environment, so these tests have to mutate
// it. `env::set_var` is unsafe under the 2024 edition, which is why this module is on the
// workspace `unsafe_code` exception list. Production code in this crate carries no `unsafe`.
#[cfg(test)]
#[allow(unsafe_code)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// Serialises every test that touches the process environment. `env::set_var` is
    /// process-global and unsound to race against concurrent readers in other test threads.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn set_secret_env(key: &str, value: &str) {
        // SAFETY: every caller holds `ENV_LOCK` for the duration of the test, so no other
        // test thread reads or writes the environment concurrently.
        unsafe { env::set_var(key, value) };
    }

    fn remove_secret_env(key: &str) {
        // SAFETY: see `set_secret_env`; callers hold `ENV_LOCK`.
        unsafe { env::remove_var(key) };
    }

    /// The control token grants full control-plane authority, so it must not be world-readable
    /// even for the moment between creation and a follow-up `chmod`.
    #[cfg(unix)]
    #[test]
    fn control_token_file_is_owner_only_including_when_it_replaces_a_loose_file() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("control.token");
        fs::write(&path, "stale").expect("seed loose file");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("loosen");

        write_control_token(&path, "fresh").expect("write token");

        let mode = fs::metadata(&path).expect("metadata").permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(fs::read_to_string(&path).expect("read token"), "fresh");
    }

    /// `ServiceConfig` derives `Debug` and is dumped in startup diagnostics, so the sections that
    /// hold credentials must redact them.
    #[test]
    fn debug_output_redacts_control_tokens() {
        let ipc = IpcSection {
            token: Some("super-secret-token".into()),
            ..IpcSection::default()
        };
        let rendered = format!("{ipc:?}");
        assert!(!rendered.contains("super-secret-token"));
        assert!(rendered.contains("redacted"));

        let overrides = ConfigOverrides {
            control_token: Some("super-secret-token".into()),
            ..ConfigOverrides::default()
        };
        let rendered = format!("{overrides:?}");
        assert!(!rendered.contains("super-secret-token"));

        let mut config = ServiceConfig::default();
        config.ipc.token = Some("super-secret-token".into());
        assert!(!format!("{config:?}").contains("super-secret-token"));
    }

    #[test]
    fn runner_session_tokens_are_unique_per_mint_and_never_reuse_the_control_token() {
        let first = generate_runner_session_token();
        let second = generate_runner_session_token();
        assert_ne!(first, second);
        assert_ne!(first, generate_local_token());
    }

    /// A clock- or pid-derived token collides whenever two mints land in the same observable
    /// state, which is exactly the property that makes such a token guessable.
    #[test]
    fn generated_control_tokens_stay_distinct_across_a_tight_mint_loop() {
        let tokens = (0..1024)
            .map(|_| generate_local_token())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(tokens.len(), 1024);
    }

    #[test]
    fn worker_profiles_keep_default_threads_close_to_compute_plus_bounded_blocking() {
        let mut core = CoreSection {
            worker_profile: WorkerProfile::LowResource,
            ..CoreSection::default()
        };
        let low = core.worker_pool_settings();
        assert_eq!((low.compute_threads, low.blocking_threads), (1, 1));

        core.worker_profile = WorkerProfile::Desktop;
        let desktop = core.worker_pool_settings();
        assert_eq!(desktop.blocking_threads, 2);
        assert!(desktop.compute_threads >= 1);

        core.worker_profile = WorkerProfile::Server;
        let server = core.worker_pool_settings();
        assert_eq!(server.compute_threads, desktop.compute_threads);
        assert!((2..=8).contains(&server.blocking_threads));
        assert!(server.queue_capacity > desktop.queue_capacity);
    }

    #[test]
    fn explicit_worker_overrides_are_applied_on_top_of_profile() {
        let core = CoreSection {
            worker_profile: WorkerProfile::LowResource,
            worker_threads: Some(3),
            blocking_threads: Some(4),
            pool_queue_limit: Some(5),
            pool_max_inflight_bytes: Some(6),
            max_isolated_workers: Some(2),
            runner_wall_clock_timeout_ms: None,
            cancel_grace_period_ms: None,
            worker_health_timeout_ms: None,
            ..CoreSection::default()
        };

        assert_eq!(
            core.worker_pool_settings(),
            WorkerPoolSettings {
                compute_threads: 3,
                blocking_threads: 4,
                queue_capacity: 5,
                max_inflight_bytes: 6,
                max_isolated_workers: 2,
            }
        );
    }

    #[test]
    fn execution_domains_and_lane_policies_round_trip_through_toml() {
        let config: ServiceConfig = toml::from_str(
            r#"
                [core]
                actor_control_queue_limit = 64
                actor_data_queue_limit = 512
                actor_control_quota = 8

                [[core.execution_domains]]
                id = "interactive"
                execution_classes = ["orchestration", "cpu"]
                threads = 2

                [core.execution_domains.lanes.interactive]
                weight = 16
                reserved_entries = 4
                max_share_percent = 100
                allow_idle_borrow = true
            "#,
        )
        .expect("multi-domain config should parse");

        assert_eq!(config.core.execution_domains.len(), 1);
        let domain = &config.core.execution_domains[0];
        assert_eq!(domain.id, "interactive");
        assert_eq!(
            domain.execution_classes,
            vec![ExecutionClassName::Orchestration, ExecutionClassName::Cpu,]
        );
        assert_eq!(
            domain.lanes[&DispatchLaneName::Interactive].reserved_entries,
            Some(4)
        );
        assert_eq!(config.core.actor_control_quota, Some(8));
    }

    #[test]
    fn directory_access_probe_is_cleaned_up() {
        let dir = tempfile::tempdir().unwrap();

        verify_directory_access(dir.path()).unwrap();

        assert!(fs::read_dir(dir.path()).unwrap().next().is_none());
    }

    #[test]
    fn secret_file_loads_relative_to_config_and_environment_overrides_it() {
        let _env = ENV_LOCK.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        let config_path = write_product_config(root.path(), "local.secret.toml");
        fs::write(
            root.path().join("local.secret.toml"),
            "[secrets]\nQQBOT_CLIENT_SECRET = \"FILE_SECRET\"\n",
        )
        .unwrap();

        let config = ServiceConfig::load(ConfigOverrides {
            config_file: Some(config_path),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            config.secret("qqbot-client-secret").as_deref(),
            Some("FILE_SECRET")
        );
        assert!(!format!("{config:?}").contains("FILE_SECRET"));
        assert!(!toml::to_string(&config).unwrap().contains("FILE_SECRET"));

        set_secret_env("MUTSUKI_SECRET_QQBOT_CLIENT_SECRET", "ENV_SECRET");
        assert_eq!(
            config.secret("QQBOT_CLIENT_SECRET").as_deref(),
            Some("ENV_SECRET")
        );
        remove_secret_env("MUTSUKI_SECRET_QQBOT_CLIENT_SECRET");
    }

    #[test]
    fn host_secret_rotation_is_atomic_shared_and_environment_safe() {
        let _env = ENV_LOCK.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        let config_path = write_product_config(root.path(), "local.secret.toml");
        let secret_path = root.path().join("local.secret.toml");
        fs::write(&secret_path, "[secrets]\nBILIBILI_COOKIE = \"OLD\"\n").unwrap();
        let config = ServiceConfig::load(ConfigOverrides {
            config_file: Some(config_path),
            ..Default::default()
        })
        .unwrap();
        let first = config.host_secret_store();
        let second = config.host_secret_store();

        first
            .rotate("bilibili-cookie", "SESSDATA=ROTATED".into())
            .unwrap();
        assert_eq!(
            second.resolve("BILIBILI_COOKIE").as_deref(),
            Some("SESSDATA=ROTATED")
        );
        assert!(
            fs::read_to_string(&secret_path)
                .unwrap()
                .contains("SESSDATA=ROTATED")
        );
        assert!(!format!("{first:?}").contains("SESSDATA"));

        set_secret_env("MUTSUKI_SECRET_BILIBILI_COOKIE", "ENV");
        assert!(matches!(
            first.rotate("BILIBILI_COOKIE", "REJECTED".into()),
            Err(ConfigError::SecretEnvironmentOverride { .. })
        ));
        remove_secret_env("MUTSUKI_SECRET_BILIBILI_COOKIE");
    }

    #[test]
    fn prepared_secret_transaction_recovers_the_previous_file_after_interruption() {
        let _env = ENV_LOCK.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        let config_path = write_product_config(root.path(), "local.secret.toml");
        let secret_path = root.path().join("local.secret.toml");
        fs::write(&secret_path, "[secrets]\nOPENAI_API_KEY = \"OLD\"\n").unwrap();
        let config = ServiceConfig::load(ConfigOverrides {
            config_file: Some(config_path.clone()),
            ..Default::default()
        })
        .unwrap();

        let mut prepared = config
            .host_secret_store()
            .prepare_mutations(BTreeMap::from([(
                "OPENAI_API_KEY".into(),
                Some("NEW".into()),
            )]))
            .unwrap();
        assert_eq!(config.secret("OPENAI_API_KEY").as_deref(), Some("OLD"));
        prepared.activate().unwrap();
        assert_eq!(config.secret("OPENAI_API_KEY").as_deref(), Some("NEW"));
        std::mem::forget(prepared);

        let recovered = ServiceConfig::load(ConfigOverrides {
            config_file: Some(config_path),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(recovered.secret("OPENAI_API_KEY").as_deref(), Some("OLD"));
        assert!(!secret_journal_path(&secret_path).exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(secret_path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn coordinated_secret_transaction_recovers_committed_candidate() {
        let _env = ENV_LOCK.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        let config_path = write_product_config(root.path(), "local.secret.toml");
        let secret_path = root.path().join("local.secret.toml");
        fs::write(&secret_path, "[secrets]\nOPENAI_API_KEY = \"OLD\"\n").unwrap();
        let config = ServiceConfig::load(ConfigOverrides {
            config_file: Some(config_path.clone()),
            ..Default::default()
        })
        .unwrap();

        let mut prepared = config
            .host_secret_store()
            .prepare_mutations(BTreeMap::from([(
                "OPENAI_API_KEY".into(),
                Some("NEW".into()),
            )]))
            .unwrap();
        let marker = prepared.enable_coordinated_commit().unwrap().unwrap();
        prepared.activate().unwrap();
        prepared.commit().unwrap();
        std::mem::forget(prepared);

        recover_host_secret_transaction(&secret_path).unwrap();
        assert!(fs::read_to_string(&secret_path).unwrap().contains("NEW"));
        assert!(marker.exists());
        fs::remove_file(marker).unwrap();
    }

    #[test]
    fn prepared_secret_transaction_rejects_environment_owned_keys() {
        let _env = ENV_LOCK.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        let config_path = write_product_config(root.path(), "local.secret.toml");
        let secret_path = root.path().join("local.secret.toml");
        fs::write(&secret_path, "[secrets]\nOPENAI_API_KEY = \"FILE\"\n").unwrap();
        let config = ServiceConfig::load(ConfigOverrides {
            config_file: Some(config_path),
            ..Default::default()
        })
        .unwrap();

        set_secret_env("MUTSUKI_SECRET_OPENAI_API_KEY", "ENV");
        let result = config
            .host_secret_store()
            .prepare_mutations(BTreeMap::from([(
                "OPENAI_API_KEY".into(),
                Some("REJECTED".into()),
            )]));
        remove_secret_env("MUTSUKI_SECRET_OPENAI_API_KEY");

        assert!(matches!(
            result,
            Err(ConfigError::SecretEnvironmentOverride { .. })
        ));
        assert_eq!(
            fs::read_to_string(secret_path).unwrap(),
            "[secrets]\nOPENAI_API_KEY = \"FILE\"\n"
        );
    }

    #[test]
    fn explicit_config_and_secret_files_fail_loud() {
        let root = tempfile::tempdir().unwrap();
        let missing_config = root.path().join("missing.toml");
        assert!(matches!(
            ServiceConfig::load(ConfigOverrides {
                config_file: Some(missing_config.clone()),
                ..Default::default()
            }),
            Err(ConfigError::MissingConfigFile { path }) if path == missing_config
        ));

        let config_path = write_product_config(root.path(), "missing.secret.toml");
        assert!(matches!(
            ServiceConfig::load(ConfigOverrides {
                config_file: Some(config_path),
                ..Default::default()
            }),
            Err(ConfigError::ReadSecretFile { .. })
        ));
    }

    #[test]
    fn partial_sections_keep_host_defaults() {
        let root = tempfile::tempdir().unwrap();
        let config_path = root.path().join("simple.toml");
        fs::write(
            &config_path,
            format!(
                r#"[service]
profile = "simple"
home_dir = "{}"

[ipc]
enabled = false

[plugins]
dynamic_dirs = []

[observe]
json = true
"#,
                root.path()
                    .join("home")
                    .to_string_lossy()
                    .replace('\\', "/")
            ),
        )
        .unwrap();

        let config = ServiceConfig::load(ConfigOverrides {
            config_file: Some(config_path),
            ..Default::default()
        })
        .unwrap();

        assert_eq!(config.service.profile, "simple");
        assert_eq!(config.service.instance_id, "default");
        assert_eq!(config.service.home_dir, root.path().join("home"));
        assert_eq!(
            config.service.data_dir,
            config.service.home_dir.join("data")
        );
        assert!(!config.ipc.enabled);
        assert_eq!(config.ipc.name, "mutsuki-service-default");
        assert!(config.plugins.dynamic_dirs.is_empty());
        assert!(config.observe.json);
        assert_eq!(config.observe.log_file, "service.log");
    }

    #[test]
    fn legacy_builtin_plugin_selection_is_rejected() {
        let error = toml::from_str::<PluginsSection>(
            r#"builtin = ["legacy.plugin"]
dynamic_dirs = []
disabled_dir = "disabled"
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("unknown field `builtin`"));
    }

    #[test]
    fn secret_file_rejects_malformed_empty_and_duplicate_entries() {
        for (name, content, expected) in [
            ("malformed", "not = [valid", "parse"),
            (
                "empty",
                "[secrets]\nQQBOT_CLIENT_SECRET = \"  \"\n",
                "invalid",
            ),
            (
                "duplicate",
                "[secrets]\n\"qqbot-client-secret\" = \"one\"\nQQBOT_CLIENT_SECRET = \"two\"\n",
                "invalid",
            ),
        ] {
            let root = tempfile::tempdir().unwrap();
            let secret_name = format!("{name}.secret.toml");
            let config_path = write_product_config(root.path(), &secret_name);
            fs::write(root.path().join(secret_name), content).unwrap();
            let error = ServiceConfig::load(ConfigOverrides {
                config_file: Some(config_path),
                ..Default::default()
            })
            .unwrap_err();
            match expected {
                "parse" => assert!(matches!(error, ConfigError::ParseSecretFile { .. })),
                "invalid" => assert!(matches!(error, ConfigError::InvalidSecretFile { .. })),
                _ => unreachable!(),
            }
        }
    }

    fn write_product_config(root: &Path, secret_file: &str) -> PathBuf {
        let path = root.join("local.toml");
        fs::write(
            &path,
            format!(
                r#"[service]
profile = "test"
instance_id = "test"
home_dir = "{}"
data_dir = "data"
log_dir = "logs"
plugin_dir = "plugins"
run_dir = "run"

[ipc]
enabled = false
transport = "named-pipe"
name = "secret-test"

[security]
secret_file = "{secret_file}"
"#,
                root.to_string_lossy().replace('\\', "/")
            ),
        )
        .unwrap();
        path
    }
}
