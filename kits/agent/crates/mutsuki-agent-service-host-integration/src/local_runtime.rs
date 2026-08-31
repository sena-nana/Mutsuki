//! Configured in-process Agent runtime owned by the Agent ServiceHost integration.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use mutsuki_agent_adapter_api::{
    CredentialBroker, CredentialFuture, CredentialValue, ModelProtocolAdapter,
};
use mutsuki_agent_adapter_openai::OpenAiCompatibleAdapter;
use mutsuki_agent_bundle::{
    AdapterBackedModelProvider, AgentLoop, AgentPluginBundle, AgentRuntimeRunner, ModelGateway,
    SessionStore,
};
use mutsuki_agent_client::{
    AgentClient, AgentClientBackend, AgentWireAuthority, AgentWireRuntime, AgentWireStateStore,
    InProcessAgentClient, InProcessAgentService,
};
use mutsuki_agent_contracts::{
    AGENT_RUN_PROTOCOL, AgentBudget, AgentEventEnvelope, AgentMessage, AgentModelGenerateRequest,
    AgentRunRequest, AgentRunResult, AgentRunStatus, AgentSession, AgentSessionCreateRequest,
    AgentSessionForkRequest, AgentSessionGetRequest, AgentSessionState, AgentSessionStatus,
    AgentToolCall, AgentToolDescriptor, AgentToolListRequest, AgentTurnState, AgentTurnStatus,
    AgentUsage, AgentWireError, AgentWireNegotiation, AgentWireRequestEnvelope,
    AgentWireResponseEnvelope, CredentialRef, InteractionResolution, ModelGenerateRequest,
    PendingApproval, PermissionDecision, ProviderInstanceDescriptor, ResourceRef, SessionVersion,
};
use mutsuki_agent_runtime::SessionPersistence;
use mutsuki_config_service::{
    ConfigConstraints, ConfigDescriptor, ConfigExpr, ConfigKey, ConfigMutability, ConfigNode,
    ConfigPresentation, ConfigProviderId, ConfigScope, ConfigValue, ConfigValueType, LocalizedText,
    RestartPolicy, SecretState,
};
use mutsuki_runtime_contracts::{
    PluginDeploymentKind, PluginManifest, RuntimeError, RuntimeProfile, RuntimeProfileMode, Task,
    TaskBatch, TaskHandle, TaskOutcome,
};
use mutsuki_runtime_core::{Runner, RuntimeFailure, RuntimeResult};
use mutsuki_runtime_host::{
    HostRuntime, HostRuntimeConfig, RuntimeBootstrapper, TokioAsyncExecutor,
};
use mutsuki_runtime_sdk::{
    HostRuntime as _, LoadedPlugin, PluginBuilder, RuntimeBootstrapperService, RuntimeClient,
    RuntimeClientRef, TaskSubmitterRuntimeClient,
};
use mutsuki_service_config::HostSecretStore;
use mutsuki_service_runtime::{
    ConfiguredPluginFactory, LoadPlanLifecycleHook, ServiceRuntimeBuilder,
};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Mutex;
use url::Url;

use crate::AgentConnectionRegistry;

pub const LOCAL_AGENT_PLUGIN_ID: &str = "mutsuki.agent.runtime.local";
pub const LOCAL_AGENT_CONNECTION_ID: &str = "local";
pub const LOCAL_AGENT_SERVICE_ID: &str = "mutsuki.agent.runtime.local.service";
pub const LOCAL_AGENT_MANAGEMENT_SERVICE_ID: &str = "mutsuki.agent.runtime.local.management";
pub const LOCAL_AGENT_CONFIG_PROVIDER_ID: &str = LOCAL_AGENT_PLUGIN_ID;
pub const LOCAL_AGENT_PROFILE_ID: &str = "default";
pub const LOCAL_AGENT_PROVIDER_ID: &str = "local-openai";
pub const LOCAL_AGENT_API_KEY: &str = "OPENAI_API_KEY";
pub const LOCAL_AGENT_API_KEY_FIELD: &str = "api_key";

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct LocalAgentConfig {
    pub endpoint: String,
    pub model: String,
    pub api_key_key: String,
    pub assistant_instruction: String,
}

impl Default for LocalAgentConfig {
    fn default() -> Self {
        Self {
            endpoint: "https://api.openai.com/v1".into(),
            model: "gpt-5-mini".into(),
            api_key_key: LOCAL_AGENT_API_KEY.into(),
            assistant_instruction: "你是一个可靠、简洁的 QQ 助手。".into(),
        }
    }
}

impl LocalAgentConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.model.trim().is_empty() {
            return Err("model is required".into());
        }
        if self.api_key_key != LOCAL_AGENT_API_KEY {
            return Err(format!("api_key_key must be `{LOCAL_AGENT_API_KEY}`"));
        }
        if self.assistant_instruction.trim().is_empty() {
            return Err("assistant_instruction is required".into());
        }
        let endpoint = Url::parse(&self.endpoint).map_err(|_| "endpoint is invalid")?;
        let loopback = matches!(endpoint.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
        if endpoint.scheme() != "https" && !loopback {
            return Err("endpoint must use HTTPS unless it is loopback".into());
        }
        if endpoint.username() != "" || endpoint.password().is_some() {
            return Err("endpoint must not contain credentials".into());
        }
        Ok(())
    }
}

/// Product-facing Local Agent configuration. Runtime budgets stay on safe owner defaults.
#[must_use]
pub fn local_agent_config_descriptor() -> ConfigDescriptor {
    ConfigDescriptor {
        provider_id: ConfigProviderId::new(LOCAL_AGENT_CONFIG_PROVIDER_ID),
        schema_version: 1,
        value_version: 1,
        title: LocalizedText::new("模型"),
        description: None,
        scopes: vec![ConfigScope::global()],
        root: ConfigNode {
            key: ConfigKey::new("agent"),
            value_type: ConfigValueType::Object,
            title: LocalizedText::new("模型"),
            description: None,
            default_value: None,
            constraints: ConfigConstraints::default(),
            presentation: ConfigPresentation::default(),
            visibility: None,
            enabled_if: None,
            mutability: ConfigMutability::ReadWrite,
            restart_policy: RestartPolicy::PluginReload,
            children: vec![
                local_agent_field("enabled", "启用", ConfigValueType::Bool, false),
                gated_field(
                    "endpoint",
                    "接口地址",
                    ConfigValueType::String { multiline: false },
                    Some("兼容 OpenAI 的接口地址。"),
                    Some("https://api.openai.com/v1"),
                ),
                gated_field(
                    "model",
                    "模型名称",
                    ConfigValueType::String { multiline: false },
                    Some("服务商提供的模型名称。"),
                    None,
                ),
                when_enabled(local_agent_secret_field()),
                gated_field(
                    "assistant_instruction",
                    "系统提示",
                    ConfigValueType::String { multiline: true },
                    Some("助手的默认人设和规则。"),
                    None,
                ),
            ],
        },
        groups: Vec::new(),
    }
}

#[must_use]
pub fn local_agent_config_value(enabled: bool, config: &LocalAgentConfig) -> ConfigValue {
    ConfigValue::Object(
        [
            ("enabled".into(), ConfigValue::Bool(enabled)),
            (
                "endpoint".into(),
                ConfigValue::String(config.endpoint.clone()),
            ),
            ("model".into(), ConfigValue::String(config.model.clone())),
            (
                LOCAL_AGENT_API_KEY_FIELD.into(),
                ConfigValue::Secret(SecretState::Keep),
            ),
            (
                "assistant_instruction".into(),
                ConfigValue::String(config.assistant_instruction.clone()),
            ),
        ]
        .into_iter()
        .collect(),
    )
}

fn local_agent_field(
    key: &str,
    title: &str,
    value_type: ConfigValueType,
    required: bool,
) -> ConfigNode {
    ConfigNode {
        key: ConfigKey::new(key),
        value_type,
        title: LocalizedText::new(title),
        description: None,
        default_value: None,
        constraints: ConfigConstraints {
            required,
            min_length: required.then_some(1),
            max_length: required.then_some(16_384),
            ..ConfigConstraints::default()
        },
        presentation: ConfigPresentation::default(),
        visibility: None,
        enabled_if: None,
        mutability: ConfigMutability::ReadWrite,
        restart_policy: RestartPolicy::PluginReload,
        children: Vec::new(),
    }
}

fn gated_field(
    key: &str,
    title: &str,
    value_type: ConfigValueType,
    description: Option<&str>,
    placeholder: Option<&str>,
) -> ConfigNode {
    let mut node = when_enabled(local_agent_field(key, title, value_type, true));
    node.description = description.map(LocalizedText::new);
    node.presentation.placeholder = placeholder.map(str::to_owned);
    node
}

fn when_enabled(mut node: ConfigNode) -> ConfigNode {
    node.enabled_if = Some(ConfigExpr::Field {
        key: ConfigKey::new("enabled"),
    });
    node
}

fn local_agent_secret_field() -> ConfigNode {
    let mut node = local_agent_field(
        LOCAL_AGENT_API_KEY_FIELD,
        "API Key",
        ConfigValueType::Secret,
        true,
    );
    node.presentation.secret = true;
    node
}

#[derive(Clone)]
struct SqliteAgentRepository {
    connection: Arc<Mutex<Connection>>,
}

impl SqliteAgentRepository {
    fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let mut connection = Connection::open(path).map_err(|error| error.to_string())?;
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL;
                 PRAGMA foreign_keys=ON;
                 CREATE TABLE IF NOT EXISTS agent_transcript_sessions (
                    session_id TEXT PRIMARY KEY,
                    payload TEXT NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS agent_wire_sessions (
                    session_id TEXT PRIMARY KEY,
                    payload TEXT NOT NULL
                 );",
            )
            .map_err(|error| error.to_string())?;
        migrate_agent_run_results(&mut connection)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    fn list_sessions(
        &self,
        after_session_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<AgentSession>, String> {
        let connection = self.connection.lock().unwrap();
        let mut statement = connection
            .prepare(
                "SELECT payload FROM agent_transcript_sessions
                 WHERE (?1 IS NULL OR session_id > ?1)
                 ORDER BY session_id ASC LIMIT ?2",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(
                params![after_session_id, i64::try_from(limit).unwrap_or(i64::MAX)],
                |row| row.get::<_, String>(0),
            )
            .map_err(|error| error.to_string())?;
        rows.map(|row| {
            let payload = row.map_err(|error| error.to_string())?;
            serde_json::from_str(&payload).map_err(|error| error.to_string())
        })
        .collect()
    }

    fn store_run_result(
        &self,
        session_id: &str,
        turn_id: &str,
        result: &AgentRunResult,
    ) -> Result<(), String> {
        let payload = serde_json::to_string(result).map_err(|error| error.to_string())?;
        self.connection
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO agent_run_results(session_id, turn_id, payload) VALUES(?1, ?2, ?3)
                 ON CONFLICT(session_id, turn_id) DO UPDATE SET payload=excluded.payload",
                params![session_id, turn_id, payload],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    fn load_run_results(&self, session_id: &str) -> Result<Vec<(String, AgentRunResult)>, String> {
        let connection = self.connection.lock().unwrap();
        let mut statement = connection
            .prepare(
                "SELECT turn_id, payload FROM agent_run_results
                 WHERE session_id=?1 ORDER BY rowid ASC",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(params![session_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| error.to_string())?;
        rows.map(|row| {
            let (turn_id, payload) = row.map_err(|error| error.to_string())?;
            serde_json::from_str(&payload)
                .map(|result| (turn_id, result))
                .map_err(|error| error.to_string())
        })
        .collect()
    }
}

fn migrate_agent_run_results(connection: &mut Connection) -> Result<(), String> {
    let exists = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='agent_run_results')",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|error| error.to_string())?;
    if !exists {
        connection
            .execute_batch(
                "CREATE TABLE agent_run_results (
                    session_id TEXT NOT NULL,
                    turn_id TEXT NOT NULL,
                    payload TEXT NOT NULL,
                    PRIMARY KEY(session_id, turn_id)
                 );",
            )
            .map_err(|error| error.to_string())?;
        return Ok(());
    }
    let primary_key_columns = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('agent_run_results') WHERE pk > 0",
            [],
            |row| row.get::<_, u32>(0),
        )
        .map_err(|error| error.to_string())?;
    if primary_key_columns >= 2 {
        return Ok(());
    }
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    transaction
        .execute_batch(
            "ALTER TABLE agent_run_results RENAME TO agent_run_results_v1;
             CREATE TABLE agent_run_results (
                session_id TEXT NOT NULL,
                turn_id TEXT NOT NULL,
                payload TEXT NOT NULL,
                PRIMARY KEY(session_id, turn_id)
             );
             INSERT INTO agent_run_results(session_id, turn_id, payload)
                SELECT session_id, turn_id, payload FROM agent_run_results_v1;
             DROP TABLE agent_run_results_v1;",
        )
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())
}

impl SessionPersistence for SqliteAgentRepository {
    fn load(&self) -> Result<Vec<AgentSession>, mutsuki_agent_contracts::AgentError> {
        self.list_sessions(None, usize::MAX)
            .map_err(persistence_error)
    }

    fn store(&self, session: &AgentSession) -> Result<(), mutsuki_agent_contracts::AgentError> {
        let payload =
            serde_json::to_string(session).map_err(|error| persistence_error(error.to_string()))?;
        self.connection
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO agent_transcript_sessions(session_id, payload) VALUES(?1, ?2)
                 ON CONFLICT(session_id) DO UPDATE SET payload=excluded.payload",
                params![session.session_id, payload],
            )
            .map_err(|error| persistence_error(error.to_string()))?;
        Ok(())
    }
}

impl AgentWireStateStore for SqliteAgentRepository {
    fn load(&self) -> Result<Vec<(String, Value)>, AgentWireError> {
        let connection = self.connection.lock().unwrap();
        let mut statement = connection
            .prepare("SELECT session_id, payload FROM agent_wire_sessions ORDER BY session_id")
            .map_err(|error| wire_error("agent.wire.persistence", error.to_string(), false))?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| wire_error("agent.wire.persistence", error.to_string(), false))?;
        rows.map(|row| {
            let (session_id, payload) = row
                .map_err(|error| wire_error("agent.wire.persistence", error.to_string(), false))?;
            let value = serde_json::from_str(&payload).map_err(|error| {
                wire_error("agent.wire.persistence_invalid", error.to_string(), false)
            })?;
            Ok((session_id, value))
        })
        .collect()
    }

    fn store(&self, session_id: &str, state: &Value) -> Result<(), AgentWireError> {
        let payload = serde_json::to_string(state)
            .map_err(|error| wire_error("agent.wire.persistence", error.to_string(), false))?;
        self.connection
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO agent_wire_sessions(session_id, payload) VALUES(?1, ?2)
                 ON CONFLICT(session_id) DO UPDATE SET payload=excluded.payload",
                params![session_id, payload],
            )
            .map_err(|error| wire_error("agent.wire.persistence", error.to_string(), false))?;
        Ok(())
    }
}

fn persistence_error(message: String) -> mutsuki_agent_contracts::AgentError {
    mutsuki_agent_contracts::AgentError::new("agent.session.persistence_failed", message)
}

#[derive(Clone)]
struct HostSecretCredentialBroker {
    secrets: HostSecretStore,
    key: String,
}

impl CredentialBroker for HostSecretCredentialBroker {
    fn resolve(&self, credential: CredentialRef) -> CredentialFuture {
        let value = if credential.credential_id == self.key {
            self.secrets.resolve(&self.key)
        } else {
            None
        };
        Box::pin(async move {
            CredentialValue::new(value.ok_or_else(|| mutsuki_agent_contracts::ProtocolError {
                code: "agent.credential.not_found".into(),
                class: mutsuki_agent_contracts::ProtocolErrorClass::Authentication,
                message: "configured model credential is unavailable".into(),
                retry_after_ms: None,
            })?)
        })
    }
}

#[derive(Default)]
struct DeferredAgentClient {
    client: OnceLock<RuntimeClientRef>,
}

impl DeferredAgentClient {
    fn bind(&self, runtime: &HostRuntime) -> Result<(), String> {
        let submitter = runtime.host_context().task_submitter_ref();
        self.client
            .set(TaskSubmitterRuntimeClient::new(submitter).into_runtime_client())
            .map_err(|_| "local Agent runtime client was already bound".to_string())
    }

    fn client(&self) -> RuntimeResult<RuntimeClientRef> {
        self.client.get().cloned().ok_or_else(|| {
            RuntimeFailure::new(RuntimeError::new(
                mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
                LOCAL_AGENT_PLUGIN_ID,
                "runtime_client.not_bound",
            ))
        })
    }
}

impl RuntimeClient for DeferredAgentClient {
    fn submit_batch(&self, batch: TaskBatch) -> RuntimeResult<Vec<TaskHandle>> {
        self.client()?.submit_batch(batch)
    }

    fn task_outcome(&self, handle: &TaskHandle) -> RuntimeResult<Option<TaskOutcome>> {
        self.client()?.task_outcome(handle)
    }
}

/// Runner factory invoked with the embedded runtime client when the local
/// Agent engine boots.
pub type LocalAgentRunnerFactory = Arc<dyn Fn(RuntimeClientRef) -> Box<dyn Runner> + Send + Sync>;

/// Bot-agnostic extension installed into the embedded local Agent runtime.
/// `tools` become model-visible descriptors in the bundle tool registry, and
/// `manifests` plus `runners` provide the target protocols those tools route
/// to. Extensions grant no control-plane authority by themselves: write tools
/// still pass the loop approval gate and the tool-router binding check.
#[derive(Clone, Default)]
pub struct LocalAgentRuntimeExtension {
    pub manifests: Vec<PluginManifest>,
    pub runners: Vec<LocalAgentRunnerFactory>,
    pub tools: Vec<AgentToolDescriptor>,
}

#[derive(Clone)]
struct LocalAgentEngine {
    runtime: Arc<HostRuntime>,
    sessions: SessionStore,
    repository: SqliteAgentRepository,
    adapter: Arc<dyn ModelProtocolAdapter>,
    provider: ProviderInstanceDescriptor,
    model: String,
    active: Arc<Mutex<BTreeMap<(String, String), TaskHandle>>>,
    cancelled: Arc<Mutex<BTreeSet<(String, String)>>>,
    next_task: Arc<AtomicU64>,
}

impl LocalAgentEngine {
    fn new(
        config: &LocalAgentConfig,
        secrets: HostSecretStore,
        state_path: &Path,
        extensions: &[LocalAgentRuntimeExtension],
    ) -> Result<Self, String> {
        config.validate()?;
        let repository = SqliteAgentRepository::open(state_path)?;
        let sessions = SessionStore::with_persistence(Arc::new(repository.clone()))
            .map_err(|error| error.to_string())?;
        let broker: Arc<dyn CredentialBroker> = Arc::new(HostSecretCredentialBroker {
            secrets,
            key: config.api_key_key.clone(),
        });
        let adapter: Arc<dyn ModelProtocolAdapter> = Arc::new(
            OpenAiCompatibleAdapter::new(OpenAiCompatibleAdapter::default_descriptor(), broker)
                .map_err(|error| error.message)?,
        );
        let provider = provider_descriptor(config);
        preflight_provider(adapter.clone(), provider.clone(), &config.model)?;

        let model = ModelGateway::with_default_provider(LOCAL_AGENT_PROVIDER_ID);
        model.register(Arc::new(
            AdapterBackedModelProvider::new(provider.clone(), adapter.clone(), Vec::new())
                .map_err(|error| error.to_string())?,
        ));
        let bundle = AgentPluginBundle {
            agent_loop: AgentLoop::default().with_default_model(config.model.clone()),
            model,
            sessions: sessions.clone(),
            ..Default::default()
        };
        bundle
            .context
            .set_system_prompt(config.assistant_instruction.clone());
        for tool in extensions.iter().flat_map(|extension| &extension.tools) {
            bundle
                .tools
                .register(tool.clone())
                .map_err(|error| error.to_string())?;
        }
        if extensions
            .iter()
            .any(|extension| !extension.tools.is_empty())
        {
            bundle
                .context
                .set_tools(bundle.tools.list(AgentToolListRequest::default()).tools);
        }

        let deferred = Arc::new(DeferredAgentClient::default());
        let client: RuntimeClientRef = deferred.clone();
        let manifests = bundle.manifests();
        let mut bootstrapper = RuntimeBootstrapper::new();
        for manifest in &manifests {
            bootstrapper.register_manifest(manifest.clone());
        }
        for manifest in extensions.iter().flat_map(|extension| &extension.manifests) {
            bootstrapper.register_manifest(manifest.clone());
        }
        for kind in AgentRuntimeRunner::ALL {
            bootstrapper.register_builtin_runner(bundle.runtime_runner(kind, client.clone()));
        }
        for runner in extensions.iter().flat_map(|extension| &extension.runners) {
            bootstrapper.register_builtin_runner(runner(client.clone()));
        }
        bootstrapper.register_async_handler(bundle.model_async_handler());
        let mut enabled_plugins = manifests
            .iter()
            .map(|manifest| manifest.plugin_id.clone())
            .collect::<Vec<_>>();
        enabled_plugins.extend(
            extensions
                .iter()
                .flat_map(|extension| &extension.manifests)
                .map(|manifest| manifest.plugin_id.clone()),
        );
        let runtime = bootstrapper
            .into_host_runtime_with_config(
                RuntimeProfile {
                    profile_id: "mutsuki.agent.local.in-process".into(),
                    mode: RuntimeProfileMode::LockedBuiltin,
                    enabled_plugins: enabled_plugins.clone(),
                    bindings: BTreeMap::new(),
                    surface_bindings: BTreeMap::new(),
                    supported_extensions: Vec::new(),
                    plugin_deployments: enabled_plugins
                        .into_iter()
                        .map(|plugin_id| (plugin_id, PluginDeploymentKind::Builtin))
                        .collect(),
                    observability: Default::default(),
                    allow_dynamic_registration: false,
                    allow_hot_reload: false,
                },
                HostRuntimeConfig {
                    event_driven: true,
                    async_executor: Some(Arc::new(
                        TokioAsyncExecutor::new(1, 32, 64, 8 * 1024 * 1024)
                            .map_err(|error| error.to_string())?,
                    )),
                    ..HostRuntimeConfig::default()
                },
            )
            .map_err(|error| error.to_string())?;
        deferred.bind(&runtime)?;
        Ok(Self {
            runtime: Arc::new(runtime),
            sessions,
            repository,
            adapter,
            provider,
            model: config.model.clone(),
            active: Arc::new(Mutex::new(BTreeMap::new())),
            cancelled: Arc::new(Mutex::new(BTreeSet::new())),
            next_task: Arc::new(AtomicU64::new(0)),
        })
    }

    fn run_request(
        &self,
        session_id: &str,
        turn_id: &str,
        request: AgentRunRequest,
    ) -> Result<AgentRunResult, AgentWireError> {
        let task_number = self.next_task.fetch_add(1, Ordering::Relaxed) + 1;
        let task_id = format!("local-agent:{session_id}:{turn_id}:{task_number}");
        let payload = serde_json::to_value(request)
            .map_err(|error| wire_error("agent.local.request_invalid", error.to_string(), false))?;
        let handle = self
            .runtime
            .submit_task(Task::new(task_id, AGENT_RUN_PROTOCOL, payload))
            .map_err(runtime_wire_error)?;
        self.active
            .lock()
            .unwrap()
            .insert((session_id.into(), turn_id.into()), handle.clone());
        let deadline = Instant::now() + Duration::from_secs(120);
        let outcome = loop {
            if let Some(outcome) = self
                .runtime
                .task_outcome(&handle)
                .map_err(runtime_wire_error)?
            {
                break outcome;
            }
            if Instant::now() >= deadline {
                let _ = self.runtime.cancel_task(&handle);
                self.active
                    .lock()
                    .unwrap()
                    .remove(&(session_id.into(), turn_id.into()));
                return Err(wire_error(
                    "agent.local.turn_timeout",
                    "Agent turn exceeded its time budget".into(),
                    true,
                ));
            }
            thread::sleep(Duration::from_millis(1));
        };
        self.active
            .lock()
            .unwrap()
            .remove(&(session_id.into(), turn_id.into()));
        let result = match outcome {
            TaskOutcome::Completed {
                output: Some(output),
                ..
            } => serde_json::from_value(output).map_err(|error| {
                wire_error("agent.local.result_invalid", error.to_string(), false)
            })?,
            TaskOutcome::Completed { output: None, .. } => {
                return Err(wire_error(
                    "agent.local.result_missing",
                    "Agent turn completed without a result".into(),
                    false,
                ));
            }
            TaskOutcome::Failed { error, .. } => {
                return Err(wire_error(&error.code, format!("{error:?}"), false));
            }
            TaskOutcome::Cancelled { reason, .. } => {
                return Err(wire_error(
                    "agent.turn.cancelled",
                    reason.unwrap_or_else(|| "Agent turn was cancelled".into()),
                    false,
                ));
            }
            other => {
                return Err(wire_error(
                    "agent.local.turn_incomplete",
                    format!("Agent turn ended as {other:?}"),
                    false,
                ));
            }
        };
        self.repository
            .store_run_result(session_id, turn_id, &result)
            .map_err(|error| wire_error("agent.session.persistence_failed", error, false))?;
        Ok(result)
    }

    fn persisted_wire_version(&self, session_id: &str) -> Result<SessionVersion, AgentWireError> {
        let (_, value) = AgentWireStateStore::load(&self.repository)?
            .into_iter()
            .find(|(stored_session_id, _)| stored_session_id == session_id)
            .ok_or_else(|| {
                wire_error(
                    "agent.session.not_found",
                    format!("session `{session_id}` was not found"),
                    false,
                )
            })?;
        serde_json::from_value(value.get("version").cloned().ok_or_else(|| {
            wire_error(
                "agent.session.restore_invalid",
                "persisted session version is missing".into(),
                false,
            )
        })?)
        .map_err(|error| {
            wire_error(
                "agent.session.restore_invalid",
                format!("persisted session version is invalid: {error}"),
                false,
            )
        })
    }

    fn resume_request(
        &self,
        session_id: &str,
        turn_id: &str,
        permissions: Vec<PermissionDecision>,
        interactions: Vec<InteractionResolution>,
    ) -> Result<AgentRunResult, AgentWireError> {
        let session = self.session(session_id)?;
        let mut request = AgentRunRequest::new(session.profile_id, Vec::new());
        request.session_id = Some(session_id.into());
        request.turn_id = Some(turn_id.into());
        request.max_steps = 8;
        request.permission_decisions = permissions;
        request.interaction_resolutions = interactions;
        self.run_request(session_id, turn_id, request)
    }

    fn session(&self, session_id: &str) -> Result<AgentSession, AgentWireError> {
        self.sessions
            .get(AgentSessionGetRequest {
                session_id: session_id.into(),
            })
            .map_err(agent_wire_error)
    }

    fn test_provider(&self) -> Result<(), AgentWireError> {
        preflight_provider(self.adapter.clone(), self.provider.clone(), &self.model)
            .map_err(|message| wire_error("agent.provider.preflight_failed", message, true))
    }
}

impl AgentWireRuntime for LocalAgentEngine {
    fn start_session(
        &self,
        session_id: &str,
        mut request: AgentSessionCreateRequest,
    ) -> Result<AgentSession, AgentWireError> {
        request.session_id = Some(session_id.into());
        self.sessions.create(request).map_err(agent_wire_error)
    }

    fn session_state(&self, session_id: &str) -> Result<AgentSessionState, AgentWireError> {
        let session = self.session(session_id)?;
        let results = self
            .repository
            .load_run_results(session_id)
            .map_err(|error| wire_error("agent.session.persistence_failed", error, false))?;
        let expected_version = SessionVersion(session.turn_count.saturating_add(1));
        let mut usage = AgentUsage::default();
        let mut cost_microunits = 0_u64;
        let mut turns = Vec::with_capacity(results.len());
        for (turn_id, result) in &results {
            usage.add(&result.usage);
            cost_microunits = cost_microunits.saturating_add(result.cost_microunits);
            turns.push(AgentTurnState {
                turn_id: turn_id.clone(),
                status: agent_turn_status(&result.status),
                expected_version,
                steps: Vec::new(),
                stop_reason: None,
            });
        }
        let status = results
            .last()
            .map(|(_, result)| agent_session_status(&result.status))
            .unwrap_or(AgentSessionStatus::Active);
        let pending_approvals = results
            .last()
            .map(|(_, result)| {
                result
                    .pending_approvals
                    .iter()
                    .cloned()
                    .map(|request| PendingApproval {
                        tool_call: AgentToolCall {
                            call_id: request.action_id.clone(),
                            name: request.tool.clone(),
                            input: Value::Null,
                        },
                        request,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let pending_interactions = results
            .last()
            .map(|(_, result)| result.pending_interactions.clone())
            .unwrap_or_default();
        if let Some((_, turn_id)) = self
            .active
            .lock()
            .unwrap()
            .keys()
            .find(|(active_session_id, _)| active_session_id == session_id)
            .cloned()
        {
            turns.retain(|turn| turn.turn_id != turn_id);
            turns.push(AgentTurnState {
                turn_id,
                status: AgentTurnStatus::Generating,
                expected_version,
                steps: Vec::new(),
                stop_reason: None,
            });
        }
        Ok(AgentSessionState {
            session_id: session.session_id,
            profile_id: session.profile_id,
            version: expected_version,
            status,
            budget: AgentBudget {
                max_steps: Some(8),
                ..Default::default()
            },
            usage,
            cost_microunits,
            snapshot: session.resource,
            turns,
            pending_approvals,
            pending_interactions,
            completed_attempts: BTreeSet::new(),
            committed_side_effects: BTreeSet::new(),
        })
    }

    fn submit_turn(
        &self,
        session_id: &str,
        turn_id: &str,
        messages: &[AgentMessage],
    ) -> Result<mutsuki_agent_client::AgentWireTurnOutput, AgentWireError> {
        let session = self.session(session_id)?;
        let mut request = AgentRunRequest::new(session.profile_id, messages.to_vec());
        request.session_id = Some(session_id.into());
        request.turn_id = Some(turn_id.into());
        request.max_steps = 8;
        let result = self.run_request(session_id, turn_id, request)?;
        let session = self.session(session_id)?;
        Ok(mutsuki_agent_client::AgentWireTurnOutput {
            events: result.events.clone(),
            next_sequence: session.next_event_sequence,
            payload: serde_json::to_value(result).map_err(|error| {
                wire_error("agent.local.result_invalid", error.to_string(), false)
            })?,
        })
    }

    fn cancel_turn(&self, session_id: &str, turn_id: &str) -> Result<(), AgentWireError> {
        let key = (session_id.into(), turn_id.into());
        if self.cancelled.lock().unwrap().contains(&key) {
            return Ok(());
        }
        let handle = self
            .active
            .lock()
            .unwrap()
            .get(&key)
            .cloned()
            .ok_or_else(|| {
                wire_error("agent.turn.not_active", "turn is not active".into(), false)
            })?;
        self.runtime
            .cancel_task(&handle)
            .map_err(runtime_wire_error)?;
        self.cancelled.lock().unwrap().insert(key);
        Ok(())
    }

    fn apply_permission(
        &self,
        decision: &PermissionDecision,
    ) -> Result<mutsuki_agent_client::AgentWireTurnOutput, AgentWireError> {
        let result = self.resume_request(
            &decision.session_id,
            &decision.turn_id,
            vec![decision.clone()],
            Vec::new(),
        )?;
        let session = self.session(&decision.session_id)?;
        Ok(mutsuki_agent_client::AgentWireTurnOutput {
            events: result.events.clone(),
            next_sequence: session.next_event_sequence,
            payload: serde_json::to_value(result).map_err(|error| {
                wire_error("agent.local.result_invalid", error.to_string(), false)
            })?,
        })
    }

    fn apply_interactions(
        &self,
        resolutions: &[InteractionResolution],
    ) -> Result<mutsuki_agent_client::AgentWireTurnOutput, AgentWireError> {
        let first = resolutions.first().ok_or_else(|| {
            wire_error(
                "agent.interaction.empty",
                "interaction is required".into(),
                false,
            )
        })?;
        if resolutions
            .iter()
            .any(|item| item.session_id != first.session_id || item.turn_id != first.turn_id)
        {
            return Err(wire_error(
                "agent.interaction.mixed_turns",
                "interaction batch must belong to one turn".into(),
                false,
            ));
        }
        let result = self.resume_request(
            &first.session_id,
            &first.turn_id,
            Vec::new(),
            resolutions.to_vec(),
        )?;
        let session = self.session(&first.session_id)?;
        Ok(mutsuki_agent_client::AgentWireTurnOutput {
            events: result.events.clone(),
            next_sequence: session.next_event_sequence,
            payload: serde_json::to_value(result).map_err(|error| {
                wire_error("agent.local.result_invalid", error.to_string(), false)
            })?,
        })
    }

    fn events_after(
        &self,
        session_id: &str,
        after_sequence: u64,
    ) -> Result<Vec<AgentEventEnvelope>, AgentWireError> {
        Ok(self
            .session(session_id)?
            .events
            .into_iter()
            .filter(|event| event.sequence > after_sequence)
            .collect())
    }

    fn fork_session(
        &self,
        source_session_id: &str,
        target_session_id: &str,
    ) -> Result<AgentSession, AgentWireError> {
        self.sessions
            .fork(AgentSessionForkRequest {
                source_session_id: source_session_id.into(),
                target_session_id: target_session_id.into(),
                title: None,
                through_turn_id: None,
            })
            .map_err(agent_wire_error)
    }

    fn read_resource(
        &self,
        resource: &ResourceRef,
        offset: u64,
        length: u32,
    ) -> Result<(Vec<u8>, bool), AgentWireError> {
        let session = self
            .repository
            .list_sessions(None, usize::MAX)
            .map_err(|error| wire_error("agent.session.persistence_failed", error, false))?
            .into_iter()
            .find(|session| session.resource.ref_id == resource.ref_id)
            .ok_or_else(|| {
                wire_error(
                    "agent.resource.not_found",
                    "resource was not found".into(),
                    false,
                )
            })?;
        let bytes = serde_json::to_vec(&session)
            .map_err(|error| wire_error("agent.resource.invalid", error.to_string(), false))?;
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(bytes.len());
        let end = start.saturating_add(length as usize).min(bytes.len());
        Ok((bytes[start..end].to_vec(), end == bytes.len()))
    }

    fn capabilities(&self) -> Result<BTreeMap<String, String>, AgentWireError> {
        Ok(BTreeMap::from([
            ("runtime".into(), LOCAL_AGENT_PLUGIN_ID.into()),
            ("profile".into(), LOCAL_AGENT_PROFILE_ID.into()),
            ("connection".into(), LOCAL_AGENT_CONNECTION_ID.into()),
        ]))
    }
}

#[derive(Clone)]
pub struct LocalAgentRuntimeService {
    authority: Arc<Mutex<AgentWireAuthority<LocalAgentEngine, SqliteAgentRepository>>>,
    engine: LocalAgentEngine,
}

impl LocalAgentRuntimeService {
    fn new(engine: LocalAgentEngine) -> Result<Self, String> {
        let repository = engine.repository.clone();
        let authority = AgentWireAuthority::new(engine.clone(), repository)
            .map_err(|error| format!("{}: {}", error.code, error.message))?;
        Ok(Self {
            authority: Arc::new(Mutex::new(authority)),
            engine,
        })
    }

    pub fn list_sessions(
        &self,
        after_session_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<AgentSession>, AgentWireError> {
        self.engine
            .repository
            .list_sessions(after_session_id, limit.clamp(1, 100))
            .map_err(|error| wire_error("agent.session.persistence_failed", error, false))
    }

    fn session_state(&self, session_id: &str) -> Result<AgentSessionState, AgentWireError> {
        let mut state = self.engine.session_state(session_id)?;
        state.version = self.engine.persisted_wire_version(session_id)?;
        for turn in &mut state.turns {
            turn.expected_version = state.version;
        }
        Ok(state)
    }

    fn cancel_active_turn(
        &self,
        session_id: &str,
        turn_id: &str,
        expected_version: SessionVersion,
    ) -> Result<SessionVersion, AgentWireError> {
        let current = self.engine.persisted_wire_version(session_id)?;
        if current != expected_version {
            return Err(wire_error(
                "agent.session.version_conflict",
                format!(
                    "expected session version {}, current version is {}",
                    expected_version.0, current.0
                ),
                false,
            ));
        }
        self.engine.cancel_turn(session_id, turn_id)?;
        self.authority
            .lock()
            .unwrap()
            .cancel(session_id, turn_id, expected_version)
    }
}

fn agent_session_status(status: &AgentRunStatus) -> AgentSessionStatus {
    match status {
        AgentRunStatus::WaitingApproval => AgentSessionStatus::WaitingApproval,
        AgentRunStatus::Cancelled => AgentSessionStatus::Cancelled,
        AgentRunStatus::Failed | AgentRunStatus::BudgetExceeded => AgentSessionStatus::Failed,
        AgentRunStatus::Completed | AgentRunStatus::WaitingInteraction => {
            AgentSessionStatus::Active
        }
    }
}

fn agent_turn_status(status: &AgentRunStatus) -> AgentTurnStatus {
    match status {
        AgentRunStatus::WaitingApproval => AgentTurnStatus::WaitingApproval,
        AgentRunStatus::Cancelled => AgentTurnStatus::Cancelled,
        AgentRunStatus::Failed | AgentRunStatus::BudgetExceeded => AgentTurnStatus::Failed,
        AgentRunStatus::Completed => AgentTurnStatus::Completed,
        AgentRunStatus::WaitingInteraction => AgentTurnStatus::RunningTools,
    }
}

impl InProcessAgentService for LocalAgentRuntimeService {
    fn dispatch(
        &mut self,
        request: AgentWireRequestEnvelope,
    ) -> Result<AgentWireResponseEnvelope, AgentWireError> {
        self.authority.lock().unwrap().dispatch(request)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LocalAgentSessionSummary {
    pub session_id: String,
    pub title: Option<String>,
    pub turn_count: u64,
    pub message_count: usize,
    pub status: AgentSessionStatus,
    pub total_tokens: u64,
    pub cost_microunits: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LocalAgentSessionPage {
    pub items: Vec<LocalAgentSessionSummary>,
    pub next_session_id: Option<String>,
}

#[derive(Clone)]
pub struct LocalAgentManagementService {
    service: LocalAgentRuntimeService,
    registry: AgentConnectionRegistry,
}

impl LocalAgentManagementService {
    fn new(service: LocalAgentRuntimeService, registry: AgentConnectionRegistry) -> Self {
        Self { service, registry }
    }

    pub fn list_sessions(
        &self,
        after_session_id: Option<&str>,
        limit: usize,
    ) -> Result<LocalAgentSessionPage, AgentWireError> {
        let requested = limit.clamp(1, 100);
        let sessions = self
            .service
            .list_sessions(after_session_id, requested.saturating_add(1))?;
        let has_more = sessions.len() > requested;
        let mut items = Vec::with_capacity(sessions.len().min(requested));
        for session in sessions.into_iter().take(requested) {
            let state = self.session_state(&session.session_id)?;
            items.push(LocalAgentSessionSummary {
                session_id: session.session_id,
                title: session.title,
                turn_count: session.turn_count,
                message_count: session.messages.len(),
                status: state.status,
                total_tokens: state.usage.total_tokens,
                cost_microunits: state.cost_microunits,
            });
        }
        let next_session_id = has_more
            .then(|| items.last().map(|item| item.session_id.clone()))
            .flatten();
        Ok(LocalAgentSessionPage {
            items,
            next_session_id,
        })
    }

    pub fn test_provider(&self) -> Result<(), AgentWireError> {
        self.service.engine.test_provider()
    }

    pub fn session(&self, session_id: &str) -> Result<AgentSession, AgentWireError> {
        self.service.engine.session(session_id)
    }

    pub fn session_state(&self, session_id: &str) -> Result<AgentSessionState, AgentWireError> {
        self.service.session_state(session_id)
    }

    pub fn events_after(
        &self,
        session_id: &str,
        after_sequence: u64,
        limit: u32,
    ) -> Result<Vec<AgentEventEnvelope>, AgentWireError> {
        Ok(self
            .service
            .engine
            .events_after(session_id, after_sequence)?
            .into_iter()
            .take(limit.clamp(1, 100) as usize)
            .collect())
    }

    pub fn approve(&self, decision: PermissionDecision) -> Result<SessionVersion, AgentWireError> {
        let mut client = self.client()?;
        client.approve_action(decision)
    }

    pub fn reject(&self, decision: PermissionDecision) -> Result<SessionVersion, AgentWireError> {
        let mut client = self.client()?;
        client.reject_action(decision)
    }

    pub fn resolve_interaction(
        &self,
        resolution: InteractionResolution,
    ) -> Result<SessionVersion, AgentWireError> {
        let mut client = self.client()?;
        client.resolve_interaction(resolution)
    }

    pub fn cancel_turn(
        &self,
        session_id: &str,
        turn_id: &str,
        expected_version: SessionVersion,
    ) -> Result<SessionVersion, AgentWireError> {
        self.service
            .cancel_active_turn(session_id, turn_id, expected_version)
    }

    pub fn close_session(
        &self,
        session_id: &str,
        expected_version: SessionVersion,
    ) -> Result<(), AgentWireError> {
        let mut client = self.client()?;
        client.close_session(session_id, expected_version)
    }
}

impl mutsuki_agent_contracts::AgentSessionManagementApi for LocalAgentManagementService {
    fn test_provider(&self) -> Result<(), AgentWireError> {
        Self::test_provider(self)
    }

    fn list_sessions(
        &self,
        after_session_id: Option<&str>,
        limit: usize,
    ) -> Result<mutsuki_agent_contracts::AgentSessionPage, AgentWireError> {
        let page = Self::list_sessions(self, after_session_id, limit)?;
        Ok(mutsuki_agent_contracts::AgentSessionPage {
            items: page
                .items
                .into_iter()
                .map(|item| mutsuki_agent_contracts::AgentSessionSummary {
                    session_id: item.session_id,
                    title: item.title,
                    turn_count: item.turn_count,
                    message_count: item.message_count,
                    status: item.status,
                    total_tokens: item.total_tokens,
                    cost_microunits: item.cost_microunits,
                })
                .collect(),
            next_session_id: page.next_session_id,
        })
    }

    fn session(&self, session_id: &str) -> Result<AgentSession, AgentWireError> {
        Self::session(self, session_id)
    }

    fn session_state(&self, session_id: &str) -> Result<AgentSessionState, AgentWireError> {
        Self::session_state(self, session_id)
    }

    fn events_after(
        &self,
        session_id: &str,
        after_sequence: u64,
        limit: u32,
    ) -> Result<Vec<AgentEventEnvelope>, AgentWireError> {
        Self::events_after(self, session_id, after_sequence, limit)
    }

    fn approve(&self, decision: PermissionDecision) -> Result<SessionVersion, AgentWireError> {
        Self::approve(self, decision)
    }

    fn reject(&self, decision: PermissionDecision) -> Result<SessionVersion, AgentWireError> {
        Self::reject(self, decision)
    }

    fn resolve_interaction(
        &self,
        resolution: InteractionResolution,
    ) -> Result<SessionVersion, AgentWireError> {
        Self::resolve_interaction(self, resolution)
    }

    fn cancel_turn(
        &self,
        session_id: &str,
        turn_id: &str,
        expected_version: SessionVersion,
    ) -> Result<SessionVersion, AgentWireError> {
        Self::cancel_turn(self, session_id, turn_id, expected_version)
    }

    fn close_session(
        &self,
        session_id: &str,
        expected_version: SessionVersion,
    ) -> Result<(), AgentWireError> {
        Self::close_session(self, session_id, expected_version)
    }
}

impl LocalAgentManagementService {
    fn client(&self) -> Result<AgentClient<Box<dyn AgentClientBackend + Send>>, AgentWireError> {
        let connection_id = LOCAL_AGENT_CONNECTION_ID
            .parse()
            .map_err(|error| wire_error("agent.connection.invalid", format!("{error}"), false))?;
        let mut client = AgentClient::new(self.registry.client_backend(&connection_id));
        client.negotiate()?;
        Ok(client)
    }
}

struct LocalConnectionHook {
    service: LocalAgentRuntimeService,
    registry: AgentConnectionRegistry,
    prepared: Mutex<Option<(Box<dyn AgentClientBackend + Send>, AgentWireNegotiation)>>,
}

impl LoadPlanLifecycleHook for LocalConnectionHook {
    fn validate(&self, _plan: &mutsuki_runtime_contracts::RuntimeLoadPlan) -> Result<(), String> {
        let backend: Box<dyn AgentClientBackend + Send> =
            Box::new(InProcessAgentClient::new(self.service.clone()));
        let mut client = AgentClient::new(backend);
        let negotiation = client
            .negotiate()
            .cloned()
            .map_err(|error| format!("{}: {}", error.code, error.message))?;
        *self.prepared.lock().unwrap() = Some((client.into_backend(), negotiation));
        Ok(())
    }

    fn activate(&self, _plan: &mutsuki_runtime_contracts::RuntimeLoadPlan) {
        if let Some((backend, negotiation)) = self.prepared.lock().unwrap().take() {
            let connection_id = LOCAL_AGENT_CONNECTION_ID
                .parse()
                .expect("fixed local Agent connection id is valid");
            self.registry
                .install_internal(connection_id, backend, negotiation);
        }
    }

    fn deactivate(&self) -> Result<(), String> {
        let connection_id = LOCAL_AGENT_CONNECTION_ID
            .parse()
            .expect("fixed local Agent connection id is valid");
        self.registry.remove_internal(&connection_id);
        Ok(())
    }
}

pub struct ConfiguredLocalAgentPlugin {
    registry: AgentConnectionRegistry,
    extensions: Vec<LocalAgentRuntimeExtension>,
}

impl ConfiguredLocalAgentPlugin {
    pub fn new(registry: AgentConnectionRegistry) -> Self {
        Self {
            registry,
            extensions: Vec::new(),
        }
    }

    /// Installs bot-agnostic tool extensions into the embedded runtime; see
    /// [`LocalAgentRuntimeExtension`].
    #[must_use]
    pub fn with_extensions(mut self, extensions: Vec<LocalAgentRuntimeExtension>) -> Self {
        self.extensions = extensions;
        self
    }
}

impl ConfiguredPluginFactory for ConfiguredLocalAgentPlugin {
    fn plugin_id(&self) -> &str {
        LOCAL_AGENT_PLUGIN_ID
    }

    fn prepare(
        &self,
        config: &Value,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        let config: LocalAgentConfig = if config.is_null() {
            LocalAgentConfig::default()
        } else {
            serde_json::from_value(config.clone()).map_err(|error| error.to_string())?
        };
        let engine = LocalAgentEngine::new(
            &config,
            builder.host_secret_store(),
            &builder.data_dir().join("agent/local/state.sqlite3"),
            &self.extensions,
        )?;
        let service = LocalAgentRuntimeService::new(engine)?;
        let management = Arc::new(LocalAgentManagementService::new(
            service.clone(),
            self.registry.clone(),
        ));
        let runtime_service = Arc::new(service.clone());
        let manifest = PluginBuilder::new(LOCAL_AGENT_PLUGIN_ID)
            .host_service(
                LOCAL_AGENT_SERVICE_ID,
                runtime_service.clone(),
                "agent.runtime.local",
            )
            .host_service(
                LOCAL_AGENT_MANAGEMENT_SERVICE_ID,
                management.clone(),
                "agent.session.manage",
            )
            .build()
            .manifest;
        let loaded_manifest = manifest.clone();
        let hook = Arc::new(LocalConnectionHook {
            service,
            registry: self.registry.clone(),
            prepared: Mutex::new(None),
        });
        Ok(builder
            .register_builtin_loaded_plugin_factory(manifest, move || {
                Ok::<LoadedPlugin, String>(LoadedPlugin {
                    manifest: loaded_manifest.clone(),
                    runners: Vec::new(),
                    async_handlers: Vec::new(),
                    host_services: vec![
                        RuntimeBootstrapperService::new(
                            LOCAL_AGENT_SERVICE_ID,
                            runtime_service.clone(),
                            "agent.runtime.local",
                        ),
                        RuntimeBootstrapperService::new(
                            LOCAL_AGENT_MANAGEMENT_SERVICE_ID,
                            management.clone(),
                            "agent.session.manage",
                        ),
                    ],
                    resource_providers: Vec::new(),
                    async_resource_providers: Vec::new(),
                    host_effects: Vec::new(),
                })
            })
            .register_health_probe(LOCAL_AGENT_PLUGIN_ID, {
                let registry = self.registry.clone();
                move || {
                    let connection_id = LOCAL_AGENT_CONNECTION_ID
                        .parse()
                        .expect("fixed local Agent connection id is valid");
                    json!({
                        "ready": registry.is_healthy(&connection_id),
                        "connection": LOCAL_AGENT_CONNECTION_ID,
                    })
                }
            })
            .register_load_plan_hook(LOCAL_AGENT_PLUGIN_ID, hook))
    }
}

fn provider_descriptor(config: &LocalAgentConfig) -> ProviderInstanceDescriptor {
    ProviderInstanceDescriptor {
        provider_id: LOCAL_AGENT_PROVIDER_ID.into(),
        adapter_id: OpenAiCompatibleAdapter::default_descriptor().adapter_id,
        endpoint: config.endpoint.clone(),
        credential: CredentialRef {
            credential_id: config.api_key_key.clone(),
            revision: 1,
        },
        models: BTreeMap::from([(
            config.model.clone(),
            OpenAiCompatibleAdapter::default_descriptor().capability,
        )]),
        headers: BTreeMap::new(),
        compatibility: BTreeMap::from([
            ("timeout_ms".into(), json!(8_000)),
            ("max_retries".into(), json!(0)),
        ]),
        remote_execution_allowed: false,
    }
}

fn preflight_provider(
    adapter: Arc<dyn ModelProtocolAdapter>,
    provider: ProviderInstanceDescriptor,
    model: &str,
) -> Result<(), String> {
    let request = ModelGenerateRequest {
        request: AgentModelGenerateRequest {
            model: model.into(),
            messages: vec![AgentMessage::user("Reply with OK.")],
            temperature: Some(0.0),
            max_output_tokens: Some(1),
            provider_hint: Some(provider.provider_id.clone()),
            metadata: None,
            result_protocol_id: None,
            result_context: None,
            session_id: None,
        },
        tools: Vec::new(),
        structured_output: None,
        reasoning: None,
    };
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    thread::Builder::new()
        .name("mutsuki-agent-preflight".into())
        .spawn(move || {
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| error.to_string())
                .and_then(|runtime| {
                    runtime
                        .block_on(adapter.generate(provider, request))
                        .map(|_| ())
                        .map_err(|error| format!("{}: {}", error.code, error.message))
                });
            let _ = sender.send(result);
        })
        .map_err(|error| error.to_string())?;
    receiver
        .recv_timeout(Duration::from_secs(10))
        .map_err(|_| "model connection test timed out".to_string())?
}

fn agent_wire_error(error: mutsuki_agent_contracts::AgentError) -> AgentWireError {
    wire_error(&error.code, error.message, false)
}

fn runtime_wire_error(error: RuntimeFailure) -> AgentWireError {
    wire_error(&error.error().code, error.to_string(), false)
}

fn wire_error(code: &str, message: String, retryable: bool) -> AgentWireError {
    AgentWireError {
        code: code.into(),
        message,
        retryable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use tempfile::tempdir;

    #[test]
    fn local_config_rejects_plaintext_remote_endpoints() {
        let error = LocalAgentConfig {
            endpoint: "http://api.example.com/v1".into(),
            ..Default::default()
        }
        .validate()
        .unwrap_err();
        assert!(error.contains("HTTPS"));
        assert!(
            LocalAgentConfig {
                endpoint: "http://127.0.0.1:8080/v1".into(),
                ..Default::default()
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn sqlite_repository_restores_transcript_and_wire_state() {
        let root = tempdir().unwrap();
        let path = root.path().join("state.sqlite3");
        let repository = SqliteAgentRepository::open(&path).unwrap();
        let store = SessionStore::with_persistence(Arc::new(repository.clone())).unwrap();
        let session = store
            .create(AgentSessionCreateRequest {
                session_id: Some("session-1".into()),
                profile_id: LOCAL_AGENT_PROFILE_ID.into(),
                title: Some("恢复测试".into()),
            })
            .unwrap();
        store
            .append(mutsuki_agent_contracts::AgentSessionAppendRequest {
                session_id: session.session_id.clone(),
                messages: vec![AgentMessage::user("hello")],
                events: Vec::new(),
                advance_turn: true,
            })
            .unwrap();
        AgentWireStateStore::store(&repository, "session-1", &json!({"version": 2})).unwrap();

        let reopened = SqliteAgentRepository::open(&path).unwrap();
        let restored = SessionStore::with_persistence(Arc::new(reopened.clone())).unwrap();
        assert_eq!(
            restored
                .get(AgentSessionGetRequest {
                    session_id: "session-1".into(),
                })
                .unwrap()
                .messages[0]
                .content,
            "hello"
        );
        assert_eq!(AgentWireStateStore::load(&reopened).unwrap().len(), 1);
    }

    /// The extension seam must make an added tool reachable from the loop
    /// inside the embedded runtime: the scripted model answers with a tool
    /// call, the tool router routes it to the extension runner, and the echoed
    /// output feeds the final model turn without any approval round-trip for a
    /// read-only tool.
    #[test]
    fn local_engine_boots_with_runtime_extensions_and_executes_their_tools() {
        struct EchoProtocol;
        impl mutsuki_runtime_sdk::SdkProtocol for EchoProtocol {
            const PROTOCOL_ID: &'static str = "mutsuki.agent.local.test/echo@1";
        }
        impl mutsuki_runtime_sdk::ProtocolSpec for EchoProtocol {}

        const PREFLIGHT_OK: &str = r#"{"choices":[{"message":{"role":"assistant","content":"OK"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}"#;
        const TOOL_CALL: &str = r#"{"choices":[{"message":{"role":"assistant","content":"","tool_calls":[{"id":"call-1","type":"function","function":{"name":"test.echo","arguments":"{\"value\":\"ping\"}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}"#;
        const FINAL: &str = r#"{"choices":[{"message":{"role":"assistant","content":"done"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}"#;

        fn read_request_text(stream: &mut std::net::TcpStream) -> String {
            let mut bytes = Vec::new();
            let mut chunk = [0_u8; 4096];
            loop {
                let count = stream.read(&mut chunk).unwrap();
                bytes.extend_from_slice(&chunk[..count]);
                let text = String::from_utf8_lossy(&bytes).to_string();
                if let Some(header_end) = text.find("\r\n\r\n") {
                    let length = text
                        .split("\r\n")
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|value| value.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if bytes.len() >= header_end + 4 + length {
                        return text;
                    }
                }
                if count == 0 {
                    return text;
                }
            }
        }

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
        let executions = Arc::new(Mutex::new(Vec::<Value>::new()));
        let boot_executions = executions.clone();
        let runner_descriptor = mutsuki_agent_sdk::orchestration_runner(
            "mutsuki.agent.local.test.runner",
            "mutsuki.agent.local.test",
        )
        .accepts::<EchoProtocol>()
        .build();
        let extension = {
            let manifest_descriptor = runner_descriptor.clone();
            LocalAgentRuntimeExtension {
                manifests: vec![
                    PluginBuilder::new("mutsuki.agent.local.test")
                        .protocol::<EchoProtocol>()
                        .runner_descriptor(manifest_descriptor)
                        .build()
                        .manifest,
                ],
                runners: vec![{
                    let boot_executions = boot_executions.clone();
                    Arc::new(move |client: RuntimeClientRef| {
                        let descriptor = runner_descriptor.clone();
                        let executions = boot_executions.clone();
                        Box::new(mutsuki_runtime_sdk::TaskAwaitRunnerAdapter::new(
                            descriptor,
                            client,
                            Box::new(move |_ctx, task| {
                                let executions = executions.clone();
                                Box::pin(async move {
                                    let request: mutsuki_agent_contracts::AgentToolExecuteRequest =
                                        serde_json::from_value(task.payload.into()).unwrap();
                                    executions.lock().push(request.input.clone());
                                    let mut result =
                                        mutsuki_runtime_sdk::contracts::RunnerResult::completed(
                                            task.task_id,
                                        );
                                    result.output = Some(request.input);
                                    Ok(result)
                                })
                            }),
                        )) as Box<dyn Runner>
                    }) as LocalAgentRunnerFactory
                }],
                tools: vec![{
                    let mut tool = AgentToolDescriptor::new(
                        "test.echo",
                        "mutsuki.agent.local.test/echo@1",
                        "Echoes its structured input",
                    );
                    tool.side_effect = mutsuki_agent_contracts::ToolSideEffect::ExternalRead;
                    tool.target_payload_mode =
                        mutsuki_agent_contracts::ToolTargetPayloadMode::ExecutionRequest;
                    tool
                }],
            }
        };

        let root = tempdir().unwrap();
        let config_path = root.path().join("local.toml");
        std::fs::write(
            &config_path,
            "[security]\nsecret_file = \"local.secret.toml\"\n[ipc]\nenabled = false\n",
        )
        .unwrap();
        std::fs::write(
            root.path().join("local.secret.toml"),
            "[secrets]\nOPENAI_API_KEY = \"test-secret\"\n",
        )
        .unwrap();
        let secrets =
            mutsuki_service_config::ServiceConfig::load(mutsuki_service_config::ConfigOverrides {
                config_file: Some(config_path),
                home_dir: Some(root.path().to_path_buf()),
                ..Default::default()
            })
            .unwrap()
            .host_secret_store();
        let server = thread::spawn(move || {
            for payload in [PREFLIGHT_OK, TOOL_CALL, FINAL] {
                let (mut stream, _) = listener.accept().unwrap();
                let _request = read_request_text(&mut stream);
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    payload.len(),
                    payload
                )
                .unwrap();
            }
        });
        let engine = LocalAgentEngine::new(
            &LocalAgentConfig {
                endpoint,
                model: "test-model".into(),
                ..Default::default()
            },
            secrets,
            &root.path().join("agent.sqlite3"),
            std::slice::from_ref(&extension),
        )
        .unwrap();

        let service = LocalAgentRuntimeService::new(engine).unwrap();
        let mut client = AgentClient::new(InProcessAgentClient::new(service));
        client.negotiate().unwrap();
        client
            .start_session(AgentSessionCreateRequest {
                session_id: Some("extension-session".into()),
                profile_id: LOCAL_AGENT_PROFILE_ID.into(),
                title: Some("extension".into()),
            })
            .unwrap();
        let version = client
            .submit_turn(
                "extension-session",
                SessionVersion(1),
                "turn-1",
                vec![AgentMessage::user("echo ping")],
                "turn-1",
            )
            .unwrap();
        assert_eq!(version, SessionVersion(2));
        server.join().unwrap();
        let executions = executions.lock();
        assert_eq!(executions.len(), 1);
        assert_eq!(executions[0]["value"], "ping");
    }

    #[test]
    fn sqlite_run_results_migrate_and_preserve_multiple_turn_usage() {
        let root = tempdir().unwrap();
        let path = root.path().join("state.sqlite3");
        let first = AgentRunResult {
            status: AgentRunStatus::Completed,
            messages: Vec::new(),
            steps: Vec::new(),
            usage: AgentUsage {
                input_tokens: 2,
                output_tokens: 3,
                total_tokens: 5,
            },
            cost_microunits: 7,
            output_resource: None,
            pending_approvals: Vec::new(),
            pending_interactions: Vec::new(),
            events: Vec::new(),
        };
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE agent_run_results (
                    session_id TEXT PRIMARY KEY,
                    turn_id TEXT NOT NULL,
                    payload TEXT NOT NULL
                 );",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO agent_run_results(session_id, turn_id, payload) VALUES(?1, ?2, ?3)",
                params!["session", "turn-1", serde_json::to_string(&first).unwrap()],
            )
            .unwrap();
        drop(connection);

        let repository = SqliteAgentRepository::open(&path).unwrap();
        let mut second = first.clone();
        second.usage = AgentUsage {
            input_tokens: 11,
            output_tokens: 13,
            total_tokens: 24,
        };
        second.cost_microunits = 17;
        repository
            .store_run_result("session", "turn-2", &second)
            .unwrap();
        let results = repository.load_run_results("session").unwrap();
        assert_eq!(
            results
                .iter()
                .map(|(_, result)| result.usage.total_tokens)
                .sum::<u64>(),
            29
        );
        assert_eq!(
            results
                .iter()
                .map(|(_, result)| result.cost_microunits)
                .sum::<u64>(),
            24
        );
        assert_eq!(
            results
                .into_iter()
                .map(|(turn_id, _)| turn_id)
                .collect::<Vec<_>>(),
            vec!["turn-1", "turn-2"]
        );
    }

    #[test]
    fn provider_preflight_executes_a_bounded_minimal_request() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut bytes = Vec::new();
            let mut chunk = [0_u8; 4096];
            loop {
                let count = stream.read(&mut chunk).unwrap();
                bytes.extend_from_slice(&chunk[..count]);
                if bytes.windows(4).any(|window| window == b"\r\n\r\n")
                    && String::from_utf8_lossy(&bytes).contains("Reply with OK")
                {
                    break;
                }
            }
            let payload = r#"{"choices":[{"message":{"role":"assistant","content":"OK"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                payload.len(),
                payload
            )
            .unwrap();
        });
        let root = tempdir().unwrap();
        let config_path = root.path().join("local.toml");
        std::fs::write(
            &config_path,
            "[service]\ndata_dir = \"data\"\nlog_dir = \"logs\"\nplugin_dir = \"plugins\"\nrun_dir = \"run\"\n[security]\nsecret_file = \"local.secret.toml\"\n[ipc]\nenabled = false\n",
        )
        .unwrap();
        std::fs::write(
            root.path().join("local.secret.toml"),
            "[secrets]\nOPENAI_API_KEY = \"test-secret\"\n",
        )
        .unwrap();
        let secrets =
            mutsuki_service_config::ServiceConfig::load(mutsuki_service_config::ConfigOverrides {
                config_file: Some(config_path),
                home_dir: Some(root.path().to_path_buf()),
                ..Default::default()
            })
            .unwrap()
            .host_secret_store();
        let broker: Arc<dyn CredentialBroker> = Arc::new(HostSecretCredentialBroker {
            secrets,
            key: LOCAL_AGENT_API_KEY.into(),
        });
        let adapter: Arc<dyn ModelProtocolAdapter> = Arc::new(
            OpenAiCompatibleAdapter::new(OpenAiCompatibleAdapter::default_descriptor(), broker)
                .unwrap(),
        );
        let config = LocalAgentConfig {
            endpoint,
            model: "test-model".into(),
            ..Default::default()
        };
        preflight_provider(adapter, provider_descriptor(&config), &config.model).unwrap();
        server.join().unwrap();
    }

    #[test]
    fn active_turn_can_be_cancelled_without_waiting_for_the_model_request() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
        let (turn_started_tx, turn_started_rx) = mpsc::sync_channel(1);
        let server = thread::spawn(move || {
            let (mut preflight, _) = listener.accept().unwrap();
            read_http_request(&mut preflight);
            write_model_response(&mut preflight, "OK");

            let (mut turn, _) = listener.accept().unwrap();
            read_http_request(&mut turn);
            turn_started_tx.send(()).unwrap();
            turn.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let mut byte = [0_u8; 1];
            let _ = turn.read(&mut byte);
        });

        let root = tempdir().unwrap();
        let config_path = root.path().join("local.toml");
        std::fs::write(
            &config_path,
            "[security]\nsecret_file = \"local.secret.toml\"\n[ipc]\nenabled = false\n",
        )
        .unwrap();
        std::fs::write(
            root.path().join("local.secret.toml"),
            "[secrets]\nOPENAI_API_KEY = \"test-secret\"\n",
        )
        .unwrap();
        let secrets =
            mutsuki_service_config::ServiceConfig::load(mutsuki_service_config::ConfigOverrides {
                config_file: Some(config_path),
                home_dir: Some(root.path().to_path_buf()),
                ..Default::default()
            })
            .unwrap()
            .host_secret_store();
        let engine = LocalAgentEngine::new(
            &LocalAgentConfig {
                endpoint,
                model: "test-model".into(),
                ..Default::default()
            },
            secrets,
            &root.path().join("agent.sqlite3"),
            &[],
        )
        .unwrap();
        let service = LocalAgentRuntimeService::new(engine).unwrap();
        let management =
            LocalAgentManagementService::new(service.clone(), AgentConnectionRegistry::new());
        let mut client = AgentClient::new(InProcessAgentClient::new(service));
        client.negotiate().unwrap();
        client
            .start_session(AgentSessionCreateRequest {
                session_id: Some("cancel-session".into()),
                profile_id: LOCAL_AGENT_PROFILE_ID.into(),
                title: Some("取消测试".into()),
            })
            .unwrap();

        let submit = thread::spawn(move || {
            client.submit_turn(
                "cancel-session",
                SessionVersion(1),
                "turn-1",
                vec![AgentMessage::user("hello")],
                "turn-1",
            )
        });
        turn_started_rx
            .recv_timeout(Duration::from_secs(3))
            .unwrap();
        let state = management.session_state("cancel-session").unwrap();
        assert!(state.turns.iter().any(|turn| {
            turn.turn_id == "turn-1" && turn.status == AgentTurnStatus::Generating
        }));
        assert_eq!(
            management
                .cancel_turn("cancel-session", "turn-1", state.version)
                .unwrap(),
            SessionVersion(2)
        );
        assert_eq!(
            submit.join().unwrap().unwrap_err().code,
            "agent.turn.cancelled"
        );
        server.join().unwrap();
    }

    fn read_http_request(stream: &mut std::net::TcpStream) {
        let mut bytes = Vec::new();
        let mut chunk = [0_u8; 4096];
        let mut expected = None;
        loop {
            let count = stream.read(&mut chunk).unwrap();
            bytes.extend_from_slice(&chunk[..count]);
            if expected.is_none()
                && let Some(headers_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n")
            {
                let headers = String::from_utf8_lossy(&bytes[..headers_end]);
                let content_length = headers.lines().find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                });
                expected = Some(headers_end + 4 + content_length.unwrap_or(0));
            }
            if expected.is_some_and(|length| bytes.len() >= length) {
                break;
            }
        }
    }

    fn write_model_response(stream: &mut std::net::TcpStream, content: &str) {
        let payload = json!({
            "choices": [{"message": {"role": "assistant", "content": content}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        })
        .to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            payload.len(),
            payload
        )
        .unwrap();
    }
}
