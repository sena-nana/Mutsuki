//! ServiceHost integration for configured Agent wire connections.
//!
//! Connector implementations own endpoint and authentication config. Consumers receive only a
//! validated connection id and an Agent wire backend proxy whose active generation can be swapped
//! after a successful handshake.

#![forbid(unsafe_code)]

mod local_runtime;

pub use local_runtime::*;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use async_trait::async_trait;
pub use mutsuki_agent_client::AgentConnectionId;
use mutsuki_agent_client::{AgentClient, AgentClientBackend, AgentLinkClient};
use mutsuki_agent_contracts::{
    AgentWireError, AgentWireNegotiation, AgentWireRequestEnvelope, AgentWireResponseEnvelope,
};
use mutsuki_config_service::{
    ConfigActivation, ConfigApplyRequest, ConfigConstraints, ConfigContext, ConfigDescriptor,
    ConfigError, ConfigKey, ConfigMutability, ConfigNode, ConfigPresentation, ConfigProvider,
    ConfigProviderId, ConfigRevision, ConfigScope, ConfigService, ConfigSnapshot, ConfigValue,
    ConfigValueType, LocalizedText, MapKeyStrategy, PreparedConfigActivation, RestartPolicy,
    ValidationCode, ValidationIssue, ValidationResult, ValidationSeverity, capability,
};
use mutsuki_link_core::{ConnectContext, EndpointId, TransportBudget};
use mutsuki_link_local::{LocalAddress, connect};
use mutsuki_runtime_sdk::{LoadedPlugin, PluginBuilder, RuntimeBootstrapperService};
use mutsuki_service_config::HostSecretStore;
use mutsuki_service_runtime::{
    ConfiguredPluginCatalog, ConfiguredPluginFactory, ServiceRuntimeBuilder, ServiceRuntimeResult,
};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub const AGENT_CONNECTIONS_PLUGIN_ID: &str = "mutsuki.agent.connections";
pub const AGENT_CONNECTION_REGISTRY_SERVICE_ID: &str = "mutsuki.agent.connection.registry";
pub const AGENT_CONNECTION_MANAGEMENT_SERVICE_ID: &str = "mutsuki.agent.connection.management";
pub const LOCAL_LINK_CONNECTOR_ID: &str = "mutsuki.agent.connector.link.local";
pub const IN_PROCESS_CONNECTOR_ID: &str = "mutsuki.agent.connector.in-process";

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentConnectionConfig {
    pub connection_id: AgentConnectionId,
    pub connector_id: String,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    #[serde(default)]
    pub config: Value,
}

impl AgentConnectionConfig {
    pub fn validate(&self) -> Result<(), AgentConnectionError> {
        if self.connector_id.trim().is_empty()
            || self.connector_id.len() > 256
            || self.connector_id.chars().any(char::is_control)
        {
            return Err(AgentConnectionError::InvalidConnectorId(
                self.connector_id.clone(),
            ));
        }
        Ok(())
    }
}

const fn enabled_by_default() -> bool {
    true
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentConnectionsConfig {
    pub connections: Vec<AgentConnectionConfig>,
}

impl AgentConnectionsConfig {
    pub fn validate(&self) -> Result<(), AgentConnectionError> {
        let mut ids = BTreeSet::new();
        for connection in &self.connections {
            connection.validate()?;
            if !ids.insert(connection.connection_id.clone()) {
                return Err(AgentConnectionError::DuplicateConnectionId(
                    connection.connection_id.clone(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentConnectionState {
    Disabled,
    Healthy,
    Unavailable,
    Reconnecting,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AgentConnectionStatus {
    pub connection_id: AgentConnectionId,
    pub connector_id: String,
    pub generation: u64,
    pub state: AgentConnectionState,
    pub negotiated_version: Option<u16>,
    pub enabled_features: Vec<String>,
    pub last_error_code: Option<String>,
}

/// Connector-owned factory. The opaque config and Host secret store never enter Bot config.
pub trait AgentConnectorFactory: Send + Sync {
    fn connector_id(&self) -> &str;

    fn connect(
        &self,
        config: &Value,
        secrets: &HostSecretStore,
    ) -> Result<Box<dyn AgentClientBackend + Send>, AgentConnectionError>;
}

#[derive(Clone, Default)]
pub struct AgentConnectorCatalog {
    factories: BTreeMap<String, Arc<dyn AgentConnectorFactory>>,
}

impl AgentConnectorCatalog {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<F>(&mut self, factory: F) -> Result<(), AgentConnectionError>
    where
        F: AgentConnectorFactory + 'static,
    {
        let connector_id = factory.connector_id().trim();
        if connector_id.is_empty() {
            return Err(AgentConnectionError::InvalidConnectorId(String::new()));
        }
        if self.factories.contains_key(connector_id) {
            return Err(AgentConnectionError::DuplicateConnectorId(
                connector_id.into(),
            ));
        }
        self.factories
            .insert(connector_id.into(), Arc::new(factory));
        Ok(())
    }

    #[must_use]
    pub fn standard() -> Self {
        let mut catalog = Self::new();
        catalog
            .register(LocalLinkConnectorFactory)
            .expect("standard connector ids are unique");
        catalog
    }

    fn factory(
        &self,
        connector_id: &str,
    ) -> Result<Arc<dyn AgentConnectorFactory>, AgentConnectionError> {
        self.factories
            .get(connector_id)
            .cloned()
            .ok_or_else(|| AgentConnectionError::ConnectorNotFound(connector_id.into()))
    }
}

#[derive(Clone)]
struct ActiveConnection {
    config: AgentConnectionConfig,
    generation: u64,
    state: AgentConnectionState,
    negotiation: Option<AgentWireNegotiation>,
    backend: Option<Arc<Mutex<Box<dyn AgentClientBackend + Send>>>>,
    last_error_code: Option<String>,
}

struct PreparedConnection {
    config: AgentConnectionConfig,
    negotiation: AgentWireNegotiation,
    backend: Box<dyn AgentClientBackend + Send>,
}

/// Shared generation-fenced connection owner.
#[derive(Clone, Default)]
pub struct AgentConnectionRegistry {
    connections: Arc<RwLock<BTreeMap<AgentConnectionId, ActiveConnection>>>,
}

impl AgentConnectionRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn statuses(&self) -> Vec<AgentConnectionStatus> {
        self.connections.read().values().map(status_of).collect()
    }

    pub fn status(
        &self,
        connection_id: &AgentConnectionId,
    ) -> Result<AgentConnectionStatus, AgentConnectionError> {
        self.connections
            .read()
            .get(connection_id)
            .map(status_of)
            .ok_or_else(|| AgentConnectionError::ConnectionNotFound(connection_id.clone()))
    }

    #[must_use]
    pub fn is_healthy(&self, connection_id: &AgentConnectionId) -> bool {
        self.connections
            .read()
            .get(connection_id)
            .is_some_and(|item| {
                item.state == AgentConnectionState::Healthy && item.backend.is_some()
            })
    }

    pub fn client_backend(
        &self,
        connection_id: &AgentConnectionId,
    ) -> Box<dyn AgentClientBackend + Send> {
        Box::new(RegistryAgentBackend {
            registry: self.clone(),
            connection_id: connection_id.clone(),
        })
    }

    fn prepare(
        config: AgentConnectionConfig,
        connectors: &AgentConnectorCatalog,
        secrets: &HostSecretStore,
    ) -> Result<PreparedConnection, AgentConnectionError> {
        config.validate()?;
        if !config.enabled {
            return Err(AgentConnectionError::ConnectionDisabled(
                config.connection_id,
            ));
        }
        let factory = connectors.factory(&config.connector_id)?;
        let backend = factory.connect(&config.config, secrets)?;
        let mut client = AgentClient::new(backend);
        let negotiation = client
            .negotiate()
            .cloned()
            .map_err(AgentConnectionError::Handshake)?;
        Ok(PreparedConnection {
            config,
            negotiation,
            backend: client.into_backend(),
        })
    }

    fn replace_all(&self, config: &AgentConnectionsConfig, prepared: Vec<PreparedConnection>) {
        let mut prepared = prepared
            .into_iter()
            .map(|candidate| (candidate.config.connection_id.clone(), candidate))
            .collect::<BTreeMap<_, _>>();
        let mut current = self.connections.write();
        let mut next = BTreeMap::new();
        for item in &config.connections {
            let generation = current
                .get(&item.connection_id)
                .map_or(1, |connection| connection.generation.saturating_add(1));
            let connection = if item.enabled {
                let candidate = prepared
                    .remove(&item.connection_id)
                    .expect("enabled connection was prepared");
                ActiveConnection {
                    config: candidate.config,
                    generation,
                    state: AgentConnectionState::Healthy,
                    negotiation: Some(candidate.negotiation),
                    backend: Some(Arc::new(Mutex::new(candidate.backend))),
                    last_error_code: None,
                }
            } else {
                ActiveConnection {
                    config: item.clone(),
                    generation,
                    state: AgentConnectionState::Disabled,
                    negotiation: None,
                    backend: None,
                    last_error_code: None,
                }
            };
            next.insert(item.connection_id.clone(), connection);
        }
        *current = next;
    }

    fn commit(&self, candidate: PreparedConnection) -> AgentConnectionStatus {
        let mut connections = self.connections.write();
        let generation = connections
            .get(&candidate.config.connection_id)
            .map_or(1, |connection| connection.generation.saturating_add(1));
        let id = candidate.config.connection_id.clone();
        connections.insert(
            id.clone(),
            ActiveConnection {
                config: candidate.config,
                generation,
                state: AgentConnectionState::Healthy,
                negotiation: Some(candidate.negotiation),
                backend: Some(Arc::new(Mutex::new(candidate.backend))),
                last_error_code: None,
            },
        );
        status_of(connections.get(&id).expect("connection was inserted"))
    }

    fn install_internal(
        &self,
        connection_id: AgentConnectionId,
        backend: Box<dyn AgentClientBackend + Send>,
        negotiation: AgentWireNegotiation,
    ) -> AgentConnectionStatus {
        self.commit(PreparedConnection {
            config: AgentConnectionConfig {
                connection_id,
                connector_id: IN_PROCESS_CONNECTOR_ID.into(),
                enabled: true,
                config: Value::Null,
            },
            negotiation,
            backend,
        })
    }

    fn remove_internal(&self, connection_id: &AgentConnectionId) {
        let mut connections = self.connections.write();
        if connections
            .get(connection_id)
            .is_some_and(|connection| connection.config.connector_id == IN_PROCESS_CONNECTOR_ID)
        {
            connections.remove(connection_id);
        }
    }

    fn mark_unavailable(&self, id: &AgentConnectionId, generation: u64, error: &AgentWireError) {
        if let Some(connection) = self.connections.write().get_mut(id)
            && connection.generation == generation
        {
            connection.state = AgentConnectionState::Unavailable;
            connection.last_error_code = Some(error.code.clone());
        }
    }
}

fn status_of(connection: &ActiveConnection) -> AgentConnectionStatus {
    AgentConnectionStatus {
        connection_id: connection.config.connection_id.clone(),
        connector_id: connection.config.connector_id.clone(),
        generation: connection.generation,
        state: connection.state,
        negotiated_version: connection
            .negotiation
            .as_ref()
            .map(|negotiation| negotiation.version),
        enabled_features: connection
            .negotiation
            .as_ref()
            .map_or_else(Vec::new, |negotiation| negotiation.enabled_features.clone()),
        last_error_code: connection.last_error_code.clone(),
    }
}

struct RegistryAgentBackend {
    registry: AgentConnectionRegistry,
    connection_id: AgentConnectionId,
}

impl AgentClientBackend for RegistryAgentBackend {
    fn request(
        &mut self,
        request: AgentWireRequestEnvelope,
    ) -> Result<AgentWireResponseEnvelope, AgentWireError> {
        let (generation, backend) = {
            let connections = self.registry.connections.read();
            let connection = connections.get(&self.connection_id).ok_or_else(|| {
                wire_error(
                    "agent.connection.not_found",
                    format!("Agent connection `{}` was not found", self.connection_id),
                    false,
                )
            })?;
            if connection.state != AgentConnectionState::Healthy {
                return Err(wire_error(
                    "agent.connection.unavailable",
                    format!("Agent connection `{}` is unavailable", self.connection_id),
                    true,
                ));
            }
            let backend = connection.backend.clone().ok_or_else(|| {
                wire_error(
                    "agent.connection.unavailable",
                    format!(
                        "Agent connection `{}` has no active backend",
                        self.connection_id
                    ),
                    true,
                )
            })?;
            (connection.generation, backend)
        };
        let result = backend.lock().request(request);
        if let Err(error) = &result {
            self.registry
                .mark_unavailable(&self.connection_id, generation, error);
        }
        result
    }
}

fn wire_error(code: &str, message: String, retryable: bool) -> AgentWireError {
    AgentWireError {
        code: code.into(),
        message,
        retryable,
    }
}

/// Revision-fenced connection management service used by Config Web or another product backend.
pub struct AgentConnectionManager {
    registry: AgentConnectionRegistry,
    connectors: AgentConnectorCatalog,
    secrets: HostSecretStore,
    config_service: Arc<ConfigService>,
    state: Arc<Mutex<AgentConnectionsState>>,
}

#[derive(Clone, Default)]
struct AgentConnectionsState {
    revision: ConfigRevision,
    config: AgentConnectionsConfig,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AgentConnectionManagementSnapshot {
    pub revision: u64,
    pub connections: Vec<AgentConnectionStatus>,
}

impl AgentConnectionManager {
    fn new(
        registry: AgentConnectionRegistry,
        connectors: AgentConnectorCatalog,
        secrets: HostSecretStore,
        config_service: Arc<ConfigService>,
        state: Arc<Mutex<AgentConnectionsState>>,
    ) -> Self {
        Self {
            registry,
            connectors,
            secrets,
            config_service,
            state,
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> AgentConnectionManagementSnapshot {
        AgentConnectionManagementSnapshot {
            revision: self.state.lock().revision.0,
            connections: self.registry.statuses(),
        }
    }

    pub fn test_connection(
        &self,
        config: AgentConnectionConfig,
    ) -> Result<AgentConnectionStatus, AgentConnectionError> {
        let candidate = AgentConnectionRegistry::prepare(config, &self.connectors, &self.secrets)?;
        Ok(AgentConnectionStatus {
            connection_id: candidate.config.connection_id,
            connector_id: candidate.config.connector_id,
            generation: 0,
            state: AgentConnectionState::Healthy,
            negotiated_version: Some(candidate.negotiation.version),
            enabled_features: candidate.negotiation.enabled_features,
            last_error_code: None,
        })
    }

    /// Validates and handshakes a candidate, persists it, then atomically swaps the live backend.
    pub fn upsert(
        &self,
        expected_revision: u64,
        config: AgentConnectionConfig,
    ) -> Result<AgentConnectionStatus, AgentConnectionError> {
        config.validate()?;
        let mut next = {
            let current = self.state.lock();
            ensure_revision(current.revision, expected_revision)?;
            current.config.clone()
        };
        if let Some(index) = next
            .connections
            .iter()
            .position(|item| item.connection_id == config.connection_id)
        {
            next.connections[index] = config.clone();
        } else {
            next.connections.push(config.clone());
        }
        next.connections
            .sort_by(|left, right| left.connection_id.cmp(&right.connection_id));
        let service = self.config_service.clone();
        block_on_config(async move {
            service
                .apply(
                    AGENT_CONNECTIONS_PLUGIN_ID,
                    ConfigApplyRequest {
                        candidate: ConfigValue::from_json(
                            &serde_json::to_value(next).expect("Agent config serializes"),
                        ),
                        expected_revision: ConfigRevision(expected_revision),
                        dry_run: false,
                    },
                    ConfigContext::global(),
                    &[capability::VALUE_WRITE.into(), capability::APPLY.into()],
                )
                .await
        })
        .map_err(config_service_error)?;
        self.registry.status(&config.connection_id)
    }

    pub fn reconnect(
        &self,
        expected_revision: u64,
        connection_id: &AgentConnectionId,
    ) -> Result<AgentConnectionStatus, AgentConnectionError> {
        let config = {
            let current = self.state.lock();
            ensure_revision(current.revision, expected_revision)?;
            current
                .config
                .connections
                .iter()
                .find(|item| &item.connection_id == connection_id)
                .cloned()
                .ok_or_else(|| AgentConnectionError::ConnectionNotFound(connection_id.clone()))?
        };
        if !config.enabled {
            return Err(AgentConnectionError::ConnectionDisabled(
                connection_id.clone(),
            ));
        }
        let candidate = AgentConnectionRegistry::prepare(config, &self.connectors, &self.secrets)?;
        let current = self.state.lock();
        ensure_revision(current.revision, expected_revision)?;
        let status = self.registry.commit(candidate);
        Ok(status)
    }
}

fn ensure_revision(
    current: ConfigRevision,
    expected_revision: u64,
) -> Result<(), AgentConnectionError> {
    if current.0 == expected_revision {
        Ok(())
    } else {
        Err(AgentConnectionError::RevisionConflict {
            expected: expected_revision,
            actual: current.0,
        })
    }
}

fn config_service_error(error: ConfigError) -> AgentConnectionError {
    match error {
        ConfigError::RevisionConflict {
            expected, current, ..
        } => AgentConnectionError::RevisionConflict {
            expected,
            actual: current,
        },
        other => AgentConnectionError::Persistence(other.to_string()),
    }
}

#[must_use]
pub fn agent_connections_config_descriptor() -> ConfigDescriptor {
    ConfigDescriptor {
        provider_id: ConfigProviderId::new(AGENT_CONNECTIONS_PLUGIN_ID),
        schema_version: 1,
        value_version: 1,
        title: LocalizedText::new("Agent connections"),
        description: None,
        scopes: vec![ConfigScope::global()],
        root: ConfigNode {
            key: ConfigKey::new("agent_connections"),
            value_type: ConfigValueType::Map {
                key_strategy: MapKeyStrategy::FreeString,
                value: Box::new(ConfigValueType::Object),
            },
            title: LocalizedText::new("Agent connections"),
            description: None,
            default_value: None,
            constraints: ConfigConstraints::default(),
            presentation: ConfigPresentation::default(),
            visibility: None,
            enabled_if: None,
            mutability: ConfigMutability::ReadWrite,
            restart_policy: RestartPolicy::None,
            children: Vec::new(),
        },
        groups: Vec::new(),
    }
}

#[must_use]
pub fn agent_connections_config_value(config: &AgentConnectionsConfig) -> ConfigValue {
    ConfigValue::from_json(&serde_json::to_value(config).expect("Agent config serializes"))
}

struct AgentConnectionsConfigProvider {
    registry: AgentConnectionRegistry,
    connectors: AgentConnectorCatalog,
    secrets: HostSecretStore,
    state: Arc<Mutex<AgentConnectionsState>>,
}

struct AgentConnectionsActivation {
    registry: AgentConnectionRegistry,
    state: Arc<Mutex<AgentConnectionsState>>,
    before_state: AgentConnectionsState,
    before_connections: BTreeMap<AgentConnectionId, ActiveConnection>,
    candidate: AgentConnectionsConfig,
    next_revision: ConfigRevision,
    prepared: Option<Vec<PreparedConnection>>,
    activated: bool,
}

impl ConfigActivation for AgentConnectionsActivation {
    fn activate(&mut self) -> Result<(), ConfigError> {
        self.registry.replace_all(
            &self.candidate,
            self.prepared
                .take()
                .ok_or_else(|| ConfigError::ApplyRejected {
                    reason: "Agent connection activation was already consumed".into(),
                })?,
        );
        *self.state.lock() = AgentConnectionsState {
            revision: self.next_revision,
            config: self.candidate.clone(),
        };
        self.activated = true;
        Ok(())
    }

    fn commit(&mut self) -> Result<(), ConfigError> {
        Ok(())
    }

    fn rollback(&mut self) -> Result<(), ConfigError> {
        if self.activated {
            *self.registry.connections.write() = self.before_connections.clone();
            *self.state.lock() = self.before_state.clone();
            self.activated = false;
        }
        Ok(())
    }
}

#[async_trait]
impl ConfigProvider for AgentConnectionsConfigProvider {
    fn descriptor(&self) -> ConfigDescriptor {
        agent_connections_config_descriptor()
    }

    fn default_value(&self, _context: &ConfigContext) -> Result<ConfigValue, ConfigError> {
        Ok(agent_connections_config_value(
            &AgentConnectionsConfig::default(),
        ))
    }

    async fn validate(
        &self,
        candidate: ConfigValue,
        _context: ConfigContext,
    ) -> Result<ValidationResult, ConfigError> {
        let result = serde_json::from_value::<AgentConnectionsConfig>(candidate.to_json())
            .map_err(|error| error.to_string())
            .and_then(|config| config.validate().map_err(|error| error.to_string()));
        Ok(match result {
            Ok(()) => ValidationResult::success(),
            Err(reason) => ValidationResult::from_issues(vec![ValidationIssue {
                path: mutsuki_config_service::ConfigPath::root(),
                code: ValidationCode::BusinessRule,
                severity: ValidationSeverity::Error,
                message: LocalizedText::new(reason),
            }]),
        })
    }

    async fn prepare_activation(
        &self,
        candidate: ConfigValue,
        _current: ConfigSnapshot,
        next_revision: ConfigRevision,
        _context: ConfigContext,
    ) -> Result<PreparedConfigActivation, ConfigError> {
        let config: AgentConnectionsConfig =
            serde_json::from_value(candidate.to_json()).map_err(|error| {
                ConfigError::ApplyRejected {
                    reason: error.to_string(),
                }
            })?;
        config
            .validate()
            .map_err(|error| ConfigError::ApplyRejected {
                reason: error.to_string(),
            })?;
        let prepared = config
            .connections
            .iter()
            .filter(|item| item.enabled)
            .cloned()
            .map(|item| AgentConnectionRegistry::prepare(item, &self.connectors, &self.secrets))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| ConfigError::ApplyRejected {
                reason: error.to_string(),
            })?;
        Ok(PreparedConfigActivation::new(
            candidate,
            Box::new(AgentConnectionsActivation {
                registry: self.registry.clone(),
                state: self.state.clone(),
                before_state: self.state.lock().clone(),
                before_connections: self.registry.connections.read().clone(),
                candidate: config,
                next_revision,
                prepared: Some(prepared),
                activated: false,
            }),
        ))
    }
}

fn block_on_config<F, T>(future: F) -> T
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    match tokio::runtime::Handle::try_current() {
        Ok(handle)
            if matches!(
                handle.runtime_flavor(),
                tokio::runtime::RuntimeFlavor::MultiThread
            ) =>
        {
            tokio::task::block_in_place(|| handle.block_on(future))
        }
        Ok(_) => std::thread::spawn(move || futures_executor::block_on(future))
            .join()
            .expect("Agent config worker"),
        Err(_) => futures_executor::block_on(future),
    }
}

pub struct ConfiguredAgentConnectionsPlugin {
    registry: AgentConnectionRegistry,
    connectors: AgentConnectorCatalog,
    config_service: Arc<ConfigService>,
}

impl ConfiguredAgentConnectionsPlugin {
    #[must_use]
    pub fn new(
        registry: AgentConnectionRegistry,
        connectors: AgentConnectorCatalog,
        config_service: Arc<ConfigService>,
    ) -> Self {
        Self {
            registry,
            connectors,
            config_service,
        }
    }
}

impl ConfiguredPluginFactory for ConfiguredAgentConnectionsPlugin {
    fn plugin_id(&self) -> &str {
        AGENT_CONNECTIONS_PLUGIN_ID
    }

    fn prepare(
        &self,
        config: &Value,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        let seed: AgentConnectionsConfig =
            serde_json::from_value(config.clone()).map_err(|error| error.to_string())?;
        let state = Arc::new(Mutex::new(AgentConnectionsState::default()));
        self.config_service
            .registry()
            .register(Arc::new(AgentConnectionsConfigProvider {
                registry: self.registry.clone(),
                connectors: self.connectors.clone(),
                secrets: builder.host_secret_store(),
                state: state.clone(),
            }))
            .map_err(|error| error.to_string())?;
        let config_service = self.config_service.clone();
        block_on_config(async move {
            config_service
                .create_if_absent(
                    AGENT_CONNECTIONS_PLUGIN_ID,
                    agent_connections_config_value(&seed),
                    ConfigContext::global(),
                )
                .await?;
            config_service
                .restore(AGENT_CONNECTIONS_PLUGIN_ID, ConfigContext::global())
                .await
        })
        .map_err(|error: ConfigError| error.to_string())?;
        let config = state.lock().config.clone();
        let manager = Arc::new(AgentConnectionManager::new(
            self.registry.clone(),
            self.connectors.clone(),
            builder.host_secret_store(),
            self.config_service.clone(),
            state,
        ));
        let mut manifest = PluginBuilder::new(AGENT_CONNECTIONS_PLUGIN_ID)
            .build()
            .manifest;
        manifest.provides.capabilities = config
            .connections
            .iter()
            .map(|connection| connection.connection_id.capability())
            .collect();
        let loaded_manifest = manifest.clone();
        let registry_service = Arc::new(self.registry.clone());
        let management_service = manager;
        Ok(
            builder.register_builtin_loaded_plugin_factory(manifest, move || {
                Ok::<LoadedPlugin, String>(LoadedPlugin {
                    manifest: loaded_manifest.clone(),
                    runners: Vec::new(),
                    async_handlers: Vec::new(),
                    host_services: vec![
                        RuntimeBootstrapperService {
                            service_id: AGENT_CONNECTION_REGISTRY_SERVICE_ID.into(),
                            capability: Some("agent.connection.read".into()),
                            service: registry_service.clone(),
                        },
                        RuntimeBootstrapperService {
                            service_id: AGENT_CONNECTION_MANAGEMENT_SERVICE_ID.into(),
                            capability: Some("agent.connection.write".into()),
                            service: management_service.clone(),
                        },
                    ],
                    resource_providers: Vec::new(),
                    async_resource_providers: Vec::new(),
                })
            }),
        )
    }
}

pub fn configured_agent_plugin_catalog(
    registry: AgentConnectionRegistry,
    connectors: AgentConnectorCatalog,
    config_service: Arc<ConfigService>,
) -> ServiceRuntimeResult<ConfiguredPluginCatalog> {
    let mut catalog = ConfiguredPluginCatalog::new();
    catalog.register(ConfiguredAgentConnectionsPlugin::new(
        registry,
        connectors,
        config_service,
    ))?;
    Ok(catalog)
}

pub fn configured_standard_agent_plugin_catalog(
    registry: AgentConnectionRegistry,
    config_service: Arc<ConfigService>,
) -> ServiceRuntimeResult<ConfiguredPluginCatalog> {
    configured_agent_plugin_catalog(registry, AgentConnectorCatalog::standard(), config_service)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalLinkConnectorConfig {
    address: String,
    local_endpoint_id: String,
    remote_endpoint_id: String,
    #[serde(default = "default_connect_timeout_ms")]
    connect_timeout_ms: u64,
    #[serde(default = "default_response_timeout_ms")]
    response_timeout_ms: u64,
}

const fn default_connect_timeout_ms() -> u64 {
    5_000
}

const fn default_response_timeout_ms() -> u64 {
    5_000
}

pub struct LocalLinkConnectorFactory;

impl AgentConnectorFactory for LocalLinkConnectorFactory {
    fn connector_id(&self) -> &str {
        LOCAL_LINK_CONNECTOR_ID
    }

    fn connect(
        &self,
        config: &Value,
        _secrets: &HostSecretStore,
    ) -> Result<Box<dyn AgentClientBackend + Send>, AgentConnectionError> {
        let config: LocalLinkConnectorConfig = serde_json::from_value(config.clone())
            .map_err(|error| AgentConnectionError::Config(error.to_string()))?;
        if config.address.trim().is_empty()
            || config.connect_timeout_ms == 0
            || config.response_timeout_ms == 0
        {
            return Err(AgentConnectionError::Config(
                "local Link address and positive timeouts are required".into(),
            ));
        }
        let local_endpoint = endpoint_id(&config.local_endpoint_id)?;
        let remote_endpoint = endpoint_id(&config.remote_endpoint_id)?;
        let (requests, receiver) = mpsc::channel::<LinkBackendRequest>();
        let (started, startup) = mpsc::sync_channel(1);
        let address = LocalAddress(config.address);
        let connect_timeout = Duration::from_millis(config.connect_timeout_ms);
        let response_timeout = Duration::from_millis(config.response_timeout_ms);
        thread::Builder::new()
            .name("mutsuki-agent-link".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ =
                            started.send(Err(AgentConnectionError::Connector(error.to_string())));
                        return;
                    }
                };
                let context = ConnectContext {
                    deadline: Some(Instant::now() + connect_timeout),
                    ..ConnectContext::default()
                };
                let connection = match runtime.block_on(connect(
                    &address,
                    local_endpoint,
                    remote_endpoint,
                    TransportBudget::default(),
                    &context,
                )) {
                    Ok(connection) => connection,
                    Err(error) => {
                        let _ =
                            started.send(Err(AgentConnectionError::Connector(error.to_string())));
                        return;
                    }
                };
                let mut client =
                    AgentLinkClient::new(connection).with_response_timeout(response_timeout);
                let _ = started.send(Ok(()));
                while let Ok((request, reply)) = receiver.recv() {
                    let _ = reply.send(client.request(request));
                }
                drop(runtime);
            })
            .map_err(|error| AgentConnectionError::Connector(error.to_string()))?;
        startup
            .recv()
            .map_err(|_| AgentConnectionError::Connector("local Link worker stopped".into()))??;
        Ok(Box::new(ThreadedAgentLinkBackend { requests }))
    }
}

type LinkBackendRequest = (
    AgentWireRequestEnvelope,
    mpsc::SyncSender<Result<AgentWireResponseEnvelope, AgentWireError>>,
);

struct ThreadedAgentLinkBackend {
    requests: mpsc::Sender<LinkBackendRequest>,
}

impl AgentClientBackend for ThreadedAgentLinkBackend {
    fn request(
        &mut self,
        request: AgentWireRequestEnvelope,
    ) -> Result<AgentWireResponseEnvelope, AgentWireError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests.send((request, reply)).map_err(|_| {
            wire_error(
                "agent.transport.disconnected",
                "local Link connection worker stopped".into(),
                true,
            )
        })?;
        response.recv().map_err(|_| {
            wire_error(
                "agent.transport.disconnected",
                "local Link connection worker stopped".into(),
                true,
            )
        })?
    }
}

fn endpoint_id(value: &str) -> Result<EndpointId, AgentConnectionError> {
    let bytes =
        hex::decode(value).map_err(|_| AgentConnectionError::InvalidEndpointId(value.into()))?;
    let bytes: [u8; 16] = bytes
        .try_into()
        .map_err(|_| AgentConnectionError::InvalidEndpointId(value.into()))?;
    Ok(EndpointId::from_bytes(bytes))
}

#[derive(Debug, Error)]
pub enum AgentConnectionError {
    #[error("duplicate Agent connection id `{0}`")]
    DuplicateConnectionId(AgentConnectionId),
    #[error("invalid Agent connector id `{0}`")]
    InvalidConnectorId(String),
    #[error("duplicate Agent connector id `{0}`")]
    DuplicateConnectorId(String),
    #[error("Agent connector `{0}` is not registered")]
    ConnectorNotFound(String),
    #[error("Agent connection `{0}` was not found")]
    ConnectionNotFound(AgentConnectionId),
    #[error("Agent connection `{0}` is disabled")]
    ConnectionDisabled(AgentConnectionId),
    #[error("Agent connector config is invalid: {0}")]
    Config(String),
    #[error("Agent connector failed: {0}")]
    Connector(String),
    #[error("Agent handshake failed: {}: {}", .0.code, .0.message)]
    Handshake(AgentWireError),
    #[error("invalid Link endpoint id `{0}`; expected 32 hexadecimal characters")]
    InvalidEndpointId(String),
    #[error("expected connection revision {expected}, current revision is {actual}")]
    RevisionConflict { expected: u64, actual: u64 },
    #[error("configured plugin persistence failed: {0}")]
    Persistence(String),
}

impl AgentConnectionError {
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidConnectorId(_) | Self::Config(_) => "agent.connection.invalid_config",
            Self::DuplicateConnectionId(_) | Self::DuplicateConnectorId(_) => {
                "agent.connection.duplicate"
            }
            Self::ConnectorNotFound(_) => "agent.connection.connector_not_found",
            Self::ConnectionNotFound(_) => "agent.connection.not_found",
            Self::ConnectionDisabled(_) => "agent.connection.disabled",
            Self::Connector(_) | Self::InvalidEndpointId(_) => "agent.connection.connect_failed",
            Self::Handshake(_) => "agent.connection.handshake_failed",
            Self::RevisionConflict { .. } => "agent.connection.revision_conflict",
            Self::Persistence(_) => "agent.connection.persistence_failed",
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Condvar, Mutex as StdMutex};

    use mutsuki_agent_contracts::{
        AGENT_WIRE_SUPPORTED_FEATURES, AGENT_WIRE_VERSION, AgentWireRequest, AgentWireResponse,
    };
    use mutsuki_config_service::{
        ConfigCompareAndSetRequest, ConfigDocumentKey, ConfigDocumentSnapshot,
        ConfigProviderRegistry, ConfigRepository, InMemoryConfigRepository, PreparedConfigWrite,
    };
    use mutsuki_service_config::{ConfiguredPluginSelection, ServiceConfig};
    use serde_json::json;

    use super::*;

    #[derive(Clone)]
    struct FakeConnector {
        fail_requests: Arc<AtomicBool>,
        fail_handshakes: Arc<AtomicBool>,
        handshake_gate: Option<Arc<HandshakeGate>>,
    }

    impl FakeConnector {
        fn new(fail_requests: Arc<AtomicBool>) -> Self {
            Self {
                fail_requests,
                fail_handshakes: Arc::new(AtomicBool::new(false)),
                handshake_gate: None,
            }
        }
    }

    impl AgentConnectorFactory for FakeConnector {
        fn connector_id(&self) -> &str {
            "test.connector"
        }

        fn connect(
            &self,
            config: &Value,
            _secrets: &HostSecretStore,
        ) -> Result<Box<dyn AgentClientBackend + Send>, AgentConnectionError> {
            if config.get("reject") == Some(&Value::Bool(true)) {
                return Err(AgentConnectionError::Connector("rejected".into()));
            }
            Ok(Box::new(FakeBackend {
                fail_requests: self.fail_requests.clone(),
                fail_handshakes: self.fail_handshakes.clone(),
                handshake_gate: self.handshake_gate.clone(),
            }))
        }
    }

    struct FakeBackend {
        fail_requests: Arc<AtomicBool>,
        fail_handshakes: Arc<AtomicBool>,
        handshake_gate: Option<Arc<HandshakeGate>>,
    }

    impl AgentClientBackend for FakeBackend {
        fn request(
            &mut self,
            request: AgentWireRequestEnvelope,
        ) -> Result<AgentWireResponseEnvelope, AgentWireError> {
            if matches!(request.request, AgentWireRequest::Negotiate) {
                if let Some(gate) = &self.handshake_gate {
                    gate.wait_if_blocked();
                }
                if self.fail_handshakes.load(Ordering::SeqCst) {
                    return Err(wire_error(
                        "agent.handshake.rejected",
                        "handshake rejected".into(),
                        false,
                    ));
                }
            }
            if self.fail_requests.load(Ordering::SeqCst)
                && !matches!(request.request, AgentWireRequest::Negotiate)
            {
                return Err(wire_error(
                    "agent.transport.disconnected",
                    "disconnected".into(),
                    true,
                ));
            }
            let response = match request.request {
                AgentWireRequest::Negotiate => {
                    AgentWireResponse::Negotiated(AgentWireNegotiation {
                        version: AGENT_WIRE_VERSION,
                        enabled_features: AGENT_WIRE_SUPPORTED_FEATURES
                            .into_iter()
                            .map(str::to_owned)
                            .collect(),
                    })
                }
                _ => AgentWireResponse::Capabilities(BTreeMap::new()),
            };
            Ok(AgentWireResponseEnvelope {
                request_id: request.request_id,
                response: Ok(response),
            })
        }
    }

    #[derive(Default)]
    struct HandshakeGate {
        state: StdMutex<HandshakeGateState>,
        changed: Condvar,
    }

    #[derive(Default)]
    struct HandshakeGateState {
        blocked: bool,
        started: bool,
    }

    impl HandshakeGate {
        fn block(&self) {
            let mut state = self.state.lock().unwrap();
            state.blocked = true;
            state.started = false;
        }

        fn wait_until_started(&self) -> bool {
            let state = self.state.lock().unwrap();
            let (state, _) = self
                .changed
                .wait_timeout_while(state, Duration::from_secs(5), |state| !state.started)
                .unwrap();
            state.started
        }

        fn release(&self) {
            self.state.lock().unwrap().blocked = false;
            self.changed.notify_all();
        }

        fn wait_if_blocked(&self) {
            let mut state = self.state.lock().unwrap();
            if !state.blocked {
                return;
            }
            state.started = true;
            self.changed.notify_all();
            while state.blocked {
                state = self.changed.wait(state).unwrap();
            }
        }
    }

    fn config(reject: bool) -> AgentConnectionConfig {
        AgentConnectionConfig {
            connection_id: AgentConnectionId::new("primary").unwrap(),
            connector_id: "test.connector".into(),
            enabled: true,
            config: json!({ "reject": reject }),
        }
    }

    fn manager(fail_requests: Arc<AtomicBool>) -> Arc<AgentConnectionManager> {
        manager_with_connector(FakeConnector::new(fail_requests), config(false)).1
    }

    fn manager_with_connector(
        connector: FakeConnector,
        initial: AgentConnectionConfig,
    ) -> (Arc<ConfigService>, Arc<AgentConnectionManager>) {
        manager_with_repository(
            connector,
            initial,
            Arc::new(InMemoryConfigRepository::default()),
        )
    }

    fn manager_with_repository(
        connector: FakeConnector,
        initial: AgentConnectionConfig,
        repository: Arc<dyn ConfigRepository>,
    ) -> (Arc<ConfigService>, Arc<AgentConnectionManager>) {
        let mut connectors = AgentConnectorCatalog::new();
        connectors.register(connector).unwrap();
        let registry = AgentConnectionRegistry::new();
        let secrets = ServiceConfig::default().host_secret_store();
        let state = Arc::new(Mutex::new(AgentConnectionsState::default()));
        let providers = Arc::new(ConfigProviderRegistry::default());
        providers
            .register(Arc::new(AgentConnectionsConfigProvider {
                registry: registry.clone(),
                connectors: connectors.clone(),
                secrets: secrets.clone(),
                state: state.clone(),
            }))
            .unwrap();
        let service = Arc::new(ConfigService::new(providers, repository).unwrap());
        let service_for_seed = service.clone();
        block_on_config(async move {
            service_for_seed
                .create_if_absent(
                    AGENT_CONNECTIONS_PLUGIN_ID,
                    agent_connections_config_value(&AgentConnectionsConfig {
                        connections: vec![initial],
                    }),
                    ConfigContext::global(),
                )
                .await
        })
        .unwrap();
        let manager = Arc::new(AgentConnectionManager::new(
            registry,
            connectors,
            secrets,
            service.clone(),
            state,
        ));
        (service, manager)
    }

    fn disabled_config() -> AgentConnectionConfig {
        AgentConnectionConfig {
            enabled: false,
            ..config(false)
        }
    }

    fn gated_connector(gate: Arc<HandshakeGate>) -> FakeConnector {
        FakeConnector {
            fail_requests: Arc::new(AtomicBool::new(false)),
            fail_handshakes: Arc::new(AtomicBool::new(false)),
            handshake_gate: Some(gate),
        }
    }

    fn stored_manager(
        connector: FakeConnector,
        initial: AgentConnectionConfig,
    ) -> (Arc<ConfigService>, Arc<AgentConnectionManager>) {
        manager_with_connector(connector, initial)
    }

    fn persisted_config(service: &Arc<ConfigService>) -> AgentConnectionsConfig {
        let service = service.clone();
        let snapshot = block_on_config(async move {
            service
                .read(
                    AGENT_CONNECTIONS_PLUGIN_ID,
                    ConfigContext::global(),
                    &[capability::VALUE_READ.into()],
                )
                .await
        })
        .unwrap();
        serde_json::from_value(snapshot.value.to_json()).unwrap()
    }

    #[test]
    fn connection_id_and_duplicate_config_are_rejected() {
        assert!(AgentConnectionId::new("").is_err());
        assert!(AgentConnectionId::new("contains:separator").is_err());
        let item = config(false);
        assert!(matches!(
            AgentConnectionsConfig {
                connections: vec![item.clone(), item],
            }
            .validate(),
            Err(AgentConnectionError::DuplicateConnectionId(_))
        ));
    }

    #[test]
    fn failed_candidate_keeps_the_healthy_generation() {
        let manager = manager(Arc::new(AtomicBool::new(false)));
        let before = manager.registry.statuses()[0].clone();
        assert!(manager.test_connection(config(true)).is_err());
        assert_eq!(manager.registry.statuses()[0], before);
    }

    #[test]
    fn runtime_disconnect_marks_only_the_active_generation_unavailable() {
        let fail = Arc::new(AtomicBool::new(false));
        let manager = manager(fail.clone());
        let id = AgentConnectionId::new("primary").unwrap();
        let mut backend = manager.registry.client_backend(&id);
        fail.store(true, Ordering::SeqCst);
        let error = backend
            .request(AgentWireRequestEnvelope {
                request_id: 9,
                hello: mutsuki_agent_contracts::AgentWireHello {
                    version: AGENT_WIRE_VERSION,
                    required_features: Vec::new(),
                    optional_features: Vec::new(),
                },
                request: AgentWireRequest::ListRuntimeCapabilities,
            })
            .unwrap_err();
        assert_eq!(error.code, "agent.transport.disconnected");
        assert_eq!(
            manager.registry.status(&id).unwrap().state,
            AgentConnectionState::Unavailable
        );
    }

    #[test]
    fn slow_upsert_handshake_does_not_block_snapshot() {
        let gate = Arc::new(HandshakeGate::default());
        let (_service, manager) = stored_manager(gated_connector(gate.clone()), disabled_config());

        gate.block();
        let updating = {
            let manager = manager.clone();
            thread::spawn(move || manager.upsert(1, config(false)))
        };
        let started = gate.wait_until_started();
        let (snapshot_tx, snapshot_rx) = mpsc::sync_channel(1);
        let reading = {
            let manager = manager.clone();
            thread::spawn(move || {
                let snapshot = manager.snapshot();
                let _ = snapshot_tx.send(snapshot.clone());
                snapshot
            })
        };
        let timely_snapshot = snapshot_rx.recv_timeout(Duration::from_secs(2));
        gate.release();
        let update = updating.join().unwrap();
        let completed_snapshot = reading.join().unwrap();

        assert!(started, "upsert handshake did not reach the gate");
        assert_eq!(timely_snapshot.unwrap(), completed_snapshot);
        assert_eq!(completed_snapshot.revision, 1);
        assert_eq!(
            completed_snapshot.connections[0].state,
            AgentConnectionState::Disabled
        );
        assert_eq!(update.unwrap().generation, 2);
    }

    #[test]
    fn slow_reconnect_handshake_does_not_block_snapshot_or_change_revision() {
        let gate = Arc::new(HandshakeGate::default());
        let (_service, manager) =
            manager_with_connector(gated_connector(gate.clone()), config(false));

        gate.block();
        let reconnecting = {
            let manager = manager.clone();
            thread::spawn(move || manager.reconnect(1, &AgentConnectionId::new("primary").unwrap()))
        };
        let started = gate.wait_until_started();
        let (snapshot_tx, snapshot_rx) = mpsc::sync_channel(1);
        let reading = {
            let manager = manager.clone();
            thread::spawn(move || snapshot_tx.send(manager.snapshot()).unwrap())
        };
        let timely_snapshot = snapshot_rx.recv_timeout(Duration::from_secs(2));
        gate.release();
        let reconnect = reconnecting.join().unwrap();
        reading.join().unwrap();

        assert!(started, "reconnect handshake did not reach the gate");
        let snapshot = timely_snapshot.unwrap();
        assert_eq!(snapshot.revision, 1);
        assert_eq!(snapshot.connections[0].generation, 1);
        assert_eq!(snapshot.connections[0].state, AgentConnectionState::Healthy);
        assert_eq!(reconnect.unwrap().generation, 2);
        assert_eq!(manager.snapshot().revision, 1);
    }

    #[test]
    fn concurrent_upserts_commit_once_and_discard_the_stale_candidate() {
        let gate = Arc::new(HandshakeGate::default());
        let (service, manager) = stored_manager(gated_connector(gate.clone()), disabled_config());

        gate.block();
        let stale_update = {
            let manager = manager.clone();
            thread::spawn(move || {
                let mut candidate = config(false);
                candidate.config = json!({ "writer": "stale" });
                manager.upsert(1, candidate)
            })
        };
        let started = gate.wait_until_started();
        let mut winner = disabled_config();
        winner.config = json!({ "writer": "winner" });
        let winner_status = manager.upsert(1, winner).unwrap();
        gate.release();
        let stale_result = stale_update.join().unwrap();

        assert!(started, "stale upsert handshake did not reach the gate");
        assert!(matches!(
            stale_result,
            Err(AgentConnectionError::RevisionConflict {
                expected: 1,
                actual: 2,
            })
        ));
        assert_eq!(winner_status.state, AgentConnectionState::Disabled);
        let snapshot = manager.snapshot();
        assert_eq!(snapshot.revision, 2);
        assert_eq!(snapshot.connections[0], winner_status);
        assert_eq!(
            persisted_config(&service).connections[0].config["writer"],
            "winner"
        );
    }

    #[test]
    fn stale_reconnect_candidate_cannot_replace_a_newer_config_revision() {
        let gate = Arc::new(HandshakeGate::default());
        let (_service, manager) = stored_manager(gated_connector(gate.clone()), config(false));

        gate.block();
        let stale_reconnect = {
            let manager = manager.clone();
            thread::spawn(move || manager.reconnect(1, &AgentConnectionId::new("primary").unwrap()))
        };
        let started = gate.wait_until_started();
        let winner_status = manager.upsert(1, disabled_config()).unwrap();
        gate.release();
        let stale_result = stale_reconnect.join().unwrap();

        assert!(started, "stale reconnect handshake did not reach the gate");
        assert!(matches!(
            stale_result,
            Err(AgentConnectionError::RevisionConflict {
                expected: 1,
                actual: 2,
            })
        ));
        assert_eq!(winner_status.state, AgentConnectionState::Disabled);
        let snapshot = manager.snapshot();
        assert_eq!(snapshot.revision, 2);
        assert_eq!(snapshot.connections[0], winner_status);
    }

    #[test]
    fn failed_handshake_leaves_config_store_and_registry_unchanged() {
        let fail_handshakes = Arc::new(AtomicBool::new(false));
        let connector = FakeConnector {
            fail_requests: Arc::new(AtomicBool::new(false)),
            fail_handshakes: fail_handshakes.clone(),
            handshake_gate: None,
        };
        let (service, manager) = stored_manager(connector, config(false));
        let before_snapshot = manager.snapshot();
        let before_config = persisted_config(&service);

        fail_handshakes.store(true, Ordering::SeqCst);
        assert!(manager.upsert(1, config(false)).is_err());
        assert!(matches!(
            manager.reconnect(1, &AgentConnectionId::new("primary").unwrap()),
            Err(AgentConnectionError::Handshake(_))
        ));

        assert_eq!(manager.snapshot(), before_snapshot);
        assert_eq!(persisted_config(&service), before_config);
    }

    struct ToggleFailRepository {
        inner: InMemoryConfigRepository,
        fail: Arc<AtomicBool>,
    }

    impl ConfigRepository for ToggleFailRepository {
        fn read(
            &self,
            key: &ConfigDocumentKey,
        ) -> Result<Option<ConfigDocumentSnapshot>, ConfigError> {
            self.inner.read(key)
        }

        fn prepare_compare_and_set(
            &self,
            request: ConfigCompareAndSetRequest,
        ) -> Result<Box<dyn PreparedConfigWrite>, ConfigError> {
            if self.fail.load(Ordering::SeqCst) {
                return Err(ConfigError::PersistenceFailed {
                    reason: "injected repository failure".into(),
                });
            }
            self.inner.prepare_compare_and_set(request)
        }

        fn recover(&self) -> Result<(), ConfigError> {
            self.inner.recover()
        }
    }

    #[test]
    fn persistence_failure_does_not_publish_the_prepared_candidate() {
        let fail = Arc::new(AtomicBool::new(false));
        let repository = Arc::new(ToggleFailRepository {
            inner: InMemoryConfigRepository::default(),
            fail: fail.clone(),
        });
        let (_service, manager) = manager_with_repository(
            FakeConnector::new(Arc::new(AtomicBool::new(false))),
            disabled_config(),
            repository,
        );
        let before = manager.snapshot();
        fail.store(true, Ordering::SeqCst);

        assert!(matches!(
            manager.upsert(1, config(false)),
            Err(AgentConnectionError::Persistence(_))
        ));
        assert_eq!(manager.snapshot(), before);
    }

    #[test]
    fn successful_management_update_is_revision_fenced_persisted_and_atomic() {
        let (service, manager) = stored_manager(
            FakeConnector::new(Arc::new(AtomicBool::new(false))),
            disabled_config(),
        );

        let status = manager.upsert(1, config(false)).unwrap();
        assert_eq!(status.generation, 2);
        assert_eq!(manager.snapshot().revision, 2);
        assert_eq!(persisted_config(&service).connections, vec![config(false)]);

        let before = persisted_config(&service);
        assert!(manager.upsert(2, config(true)).is_err());
        assert_eq!(persisted_config(&service), before);
        assert_eq!(manager.snapshot().revision, 2);

        assert!(matches!(
            manager.upsert(1, config(false)),
            Err(AgentConnectionError::RevisionConflict {
                expected: 1,
                actual: 2,
            })
        ));
    }

    #[tokio::test]
    async fn configured_manifest_provides_each_declared_connection_capability() {
        let fail = Arc::new(AtomicBool::new(false));
        let mut connectors = AgentConnectorCatalog::new();
        connectors.register(FakeConnector::new(fail)).unwrap();
        let registry = AgentConnectionRegistry::new();
        let root = tempfile::tempdir().unwrap();
        let mut service = ServiceConfig::default();
        service.ipc.enabled = false;
        service.observe.console = false;
        service.plugins.dynamic_dirs.clear();
        service.service.home_dir = root.path().into();
        service.service.data_dir = root.path().join("data");
        service.service.log_dir = root.path().join("logs");
        service.service.run_dir = root.path().join("run");
        std::fs::create_dir_all(&service.service.log_dir).unwrap();
        service.plugins.configured = vec![ConfiguredPluginSelection {
            id: AGENT_CONNECTIONS_PLUGIN_ID.into(),
            enabled: true,
            config: serde_json::to_value(AgentConnectionsConfig {
                connections: vec![config(false)],
            })
            .unwrap(),
        }];
        let config_service = Arc::new(
            ConfigService::new(
                Arc::new(ConfigProviderRegistry::default()),
                Arc::new(InMemoryConfigRepository::default()),
            )
            .unwrap(),
        );
        let catalog =
            configured_agent_plugin_catalog(registry.clone(), connectors, config_service).unwrap();
        let runtime = ServiceRuntimeBuilder::new(service)
            .with_configured_plugin_catalog(catalog)
            .start()
            .await
            .unwrap();
        let id = AgentConnectionId::new("primary").unwrap();
        assert_eq!(id.capability(), "agent_connection:primary");
        assert!(registry.is_healthy(&id));
        let hosted = runtime
            .host_service::<AgentConnectionRegistry>(AGENT_CONNECTION_REGISTRY_SERVICE_ID)
            .unwrap();
        assert!(hosted.is_healthy(&id));
        runtime.shutdown().await;
    }
}
