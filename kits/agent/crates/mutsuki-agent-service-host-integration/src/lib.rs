//! ServiceHost integration for configured Agent wire connections.
//!
//! Connector implementations own endpoint and authentication config. Consumers receive only a
//! validated connection id and an Agent wire backend proxy whose active generation can be swapped
//! after a successful handshake.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

pub use mutsuki_agent_client::AgentConnectionId;
use mutsuki_agent_client::{AgentClient, AgentClientBackend, AgentLinkClient};
use mutsuki_agent_contracts::{
    AgentWireError, AgentWireNegotiation, AgentWireRequestEnvelope, AgentWireResponseEnvelope,
};
use mutsuki_link_core::{ConnectContext, EndpointId, TransportBudget};
use mutsuki_link_local::{LocalAddress, connect};
use mutsuki_runtime_sdk::{LoadedPlugin, PluginBuilder, RuntimeBootstrapperService};
use mutsuki_service_config::{ConfiguredPluginStore, HostSecretStore};
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
    pub revision: u64,
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
    ) -> Result<Box<dyn AgentClientBackend + Send>, AgentConnectionError> {
        if !self.is_healthy(connection_id) {
            return Err(AgentConnectionError::ConnectionUnavailable(
                connection_id.clone(),
            ));
        }
        Ok(Box::new(RegistryAgentBackend {
            registry: self.clone(),
            connection_id: connection_id.clone(),
        }))
    }

    /// Creates a registry-backed client before catalog installation has finished.
    ///
    /// The runtime load-plan capability check remains the startup authority. Calls still fail
    /// structurally unless the selected generation is healthy, so this removes configured-plugin
    /// list ordering as a semantic input without creating an implicit fallback.
    #[must_use]
    pub fn deferred_client_backend(
        &self,
        connection_id: AgentConnectionId,
    ) -> Box<dyn AgentClientBackend + Send> {
        Box::new(RegistryAgentBackend {
            registry: self.clone(),
            connection_id,
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

    fn commit_disabled(&self, config: AgentConnectionConfig) -> AgentConnectionStatus {
        let mut connections = self.connections.write();
        let generation = connections
            .get(&config.connection_id)
            .map_or(1, |connection| connection.generation.saturating_add(1));
        let id = config.connection_id.clone();
        connections.insert(
            id.clone(),
            ActiveConnection {
                config,
                generation,
                state: AgentConnectionState::Disabled,
                negotiation: None,
                backend: None,
                last_error_code: None,
            },
        );
        status_of(connections.get(&id).expect("connection was inserted"))
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
    store: Option<ConfiguredPluginStore>,
    config: Mutex<AgentConnectionsConfig>,
    audits: Mutex<Vec<AgentConnectionAuditEntry>>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AgentConnectionAuditEntry {
    pub audit_id: String,
    pub actor_id: String,
    pub action: String,
    pub revision: u64,
    pub connection_id: AgentConnectionId,
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
        store: Option<ConfiguredPluginStore>,
    ) -> Self {
        Self {
            registry,
            connectors,
            secrets,
            store,
            config: Mutex::new(AgentConnectionsConfig::default()),
            audits: Mutex::new(Vec::new()),
        }
    }

    fn bootstrap(&self, config: AgentConnectionsConfig) -> Result<(), AgentConnectionError> {
        config.validate()?;
        let prepared = config
            .connections
            .iter()
            .filter(|item| item.enabled)
            .cloned()
            .map(|item| AgentConnectionRegistry::prepare(item, &self.connectors, &self.secrets))
            .collect::<Result<Vec<_>, _>>()?;
        self.registry.replace_all(&config, prepared);
        *self.config.lock() = config;
        Ok(())
    }

    #[must_use]
    pub fn snapshot(&self) -> AgentConnectionManagementSnapshot {
        AgentConnectionManagementSnapshot {
            revision: self.config.lock().revision,
            connections: self.registry.statuses(),
        }
    }

    #[must_use]
    pub fn audits(&self) -> Vec<AgentConnectionAuditEntry> {
        self.audits.lock().clone()
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
        actor_id: &str,
        expected_revision: u64,
        config: AgentConnectionConfig,
    ) -> Result<AgentConnectionStatus, AgentConnectionError> {
        config.validate()?;
        let candidate = if config.enabled {
            Some(AgentConnectionRegistry::prepare(
                config.clone(),
                &self.connectors,
                &self.secrets,
            )?)
        } else {
            None
        };
        let mut current = self.config.lock();
        if current.revision != expected_revision {
            return Err(AgentConnectionError::RevisionConflict {
                expected: expected_revision,
                actual: current.revision,
            });
        }
        let mut next = current.clone();
        if let Some(existing) = next
            .connections
            .iter_mut()
            .find(|item| item.connection_id == config.connection_id)
        {
            *existing = config.clone();
        } else {
            return Err(AgentConnectionError::ConnectionNotFound(
                config.connection_id.clone(),
            ));
        }
        next.connections
            .sort_by(|left, right| left.connection_id.cmp(&right.connection_id));
        next.revision = next.revision.saturating_add(1);
        self.persist(&next)?;
        let status = match candidate {
            Some(candidate) => self.registry.commit(candidate),
            None => self.registry.commit_disabled(config.clone()),
        };
        *current = next;
        self.record_audit(actor_id, "upsert", current.revision, config.connection_id);
        Ok(status)
    }

    pub fn reconnect(
        &self,
        actor_id: &str,
        expected_revision: u64,
        connection_id: &AgentConnectionId,
    ) -> Result<AgentConnectionStatus, AgentConnectionError> {
        let current = self.config.lock();
        if current.revision != expected_revision {
            return Err(AgentConnectionError::RevisionConflict {
                expected: expected_revision,
                actual: current.revision,
            });
        }
        let config = current
            .connections
            .iter()
            .find(|item| &item.connection_id == connection_id)
            .cloned()
            .ok_or_else(|| AgentConnectionError::ConnectionNotFound(connection_id.clone()))?;
        drop(current);
        if !config.enabled {
            return Err(AgentConnectionError::ConnectionDisabled(
                connection_id.clone(),
            ));
        }
        if let Some(connection) = self.registry.connections.write().get_mut(connection_id) {
            connection.state = AgentConnectionState::Reconnecting;
        }
        let candidate =
            match AgentConnectionRegistry::prepare(config, &self.connectors, &self.secrets) {
                Ok(candidate) => candidate,
                Err(error) => {
                    if let Some(connection) =
                        self.registry.connections.write().get_mut(connection_id)
                    {
                        connection.state = AgentConnectionState::Unavailable;
                        connection.last_error_code = Some(error.code().into());
                    }
                    return Err(error);
                }
            };
        let status = self.registry.commit(candidate);
        self.record_audit(
            actor_id,
            "reconnect",
            expected_revision,
            connection_id.clone(),
        );
        Ok(status)
    }

    fn persist(&self, config: &AgentConnectionsConfig) -> Result<(), AgentConnectionError> {
        let store = self
            .store
            .as_ref()
            .ok_or(AgentConnectionError::PersistenceUnavailable)?;
        let value = serde_json::to_value(config)
            .map_err(|error| AgentConnectionError::Config(error.to_string()))?;
        store
            .replace_config(AGENT_CONNECTIONS_PLUGIN_ID, value)
            .map_err(|error| AgentConnectionError::Persistence(error.to_string()))
    }

    fn record_audit(
        &self,
        actor_id: &str,
        action: &str,
        revision: u64,
        connection_id: AgentConnectionId,
    ) {
        self.audits.lock().push(AgentConnectionAuditEntry {
            audit_id: format!("agent-connection-audit-{revision}"),
            actor_id: actor_id.into(),
            action: action.into(),
            revision,
            connection_id,
        });
    }
}

pub struct ConfiguredAgentConnectionsPlugin {
    registry: AgentConnectionRegistry,
    connectors: AgentConnectorCatalog,
}

impl ConfiguredAgentConnectionsPlugin {
    #[must_use]
    pub fn new(registry: AgentConnectionRegistry, connectors: AgentConnectorCatalog) -> Self {
        Self {
            registry,
            connectors,
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
        let config: AgentConnectionsConfig =
            serde_json::from_value(config.clone()).map_err(|error| error.to_string())?;
        let manager = Arc::new(AgentConnectionManager::new(
            self.registry.clone(),
            self.connectors.clone(),
            builder.host_secret_store(),
            builder.configured_plugin_store(),
        ));
        manager
            .bootstrap(config.clone())
            .map_err(|error| error.to_string())?;
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
) -> ServiceRuntimeResult<ConfiguredPluginCatalog> {
    let mut catalog = ConfiguredPluginCatalog::new();
    catalog.register(ConfiguredAgentConnectionsPlugin::new(registry, connectors))?;
    Ok(catalog)
}

pub fn configured_standard_agent_plugin_catalog(
    registry: AgentConnectionRegistry,
) -> ServiceRuntimeResult<ConfiguredPluginCatalog> {
    configured_agent_plugin_catalog(registry, AgentConnectorCatalog::standard())
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
    #[error("invalid Agent connection id `{0}`")]
    InvalidConnectionId(String),
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
    #[error("Agent connection `{0}` is unavailable")]
    ConnectionUnavailable(AgentConnectionId),
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
    #[error("configured plugin persistence is unavailable")]
    PersistenceUnavailable,
    #[error("configured plugin persistence failed: {0}")]
    Persistence(String),
}

impl AgentConnectionError {
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidConnectionId(_) | Self::InvalidConnectorId(_) | Self::Config(_) => {
                "agent.connection.invalid_config"
            }
            Self::DuplicateConnectionId(_) | Self::DuplicateConnectorId(_) => {
                "agent.connection.duplicate"
            }
            Self::ConnectorNotFound(_) => "agent.connection.connector_not_found",
            Self::ConnectionNotFound(_) => "agent.connection.not_found",
            Self::ConnectionDisabled(_) => "agent.connection.disabled",
            Self::ConnectionUnavailable(_) => "agent.connection.unavailable",
            Self::Connector(_) | Self::InvalidEndpointId(_) => "agent.connection.connect_failed",
            Self::Handshake(_) => "agent.connection.handshake_failed",
            Self::RevisionConflict { .. } => "agent.connection.revision_conflict",
            Self::PersistenceUnavailable | Self::Persistence(_) => {
                "agent.connection.persistence_failed"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use mutsuki_agent_contracts::{
        AGENT_WIRE_SUPPORTED_FEATURES, AGENT_WIRE_VERSION, AgentWireRequest, AgentWireResponse,
    };
    use mutsuki_service_config::{ConfiguredPluginSelection, ServiceConfig};
    use serde_json::json;

    use super::*;

    #[derive(Clone)]
    struct FakeConnector {
        fail_requests: Arc<AtomicBool>,
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
            }))
        }
    }

    struct FakeBackend {
        fail_requests: Arc<AtomicBool>,
    }

    impl AgentClientBackend for FakeBackend {
        fn request(
            &mut self,
            request: AgentWireRequestEnvelope,
        ) -> Result<AgentWireResponseEnvelope, AgentWireError> {
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

    fn config(reject: bool) -> AgentConnectionConfig {
        AgentConnectionConfig {
            connection_id: AgentConnectionId::new("primary").unwrap(),
            connector_id: "test.connector".into(),
            enabled: true,
            config: json!({ "reject": reject }),
        }
    }

    fn manager(fail_requests: Arc<AtomicBool>) -> AgentConnectionManager {
        let mut connectors = AgentConnectorCatalog::new();
        connectors
            .register(FakeConnector { fail_requests })
            .unwrap();
        AgentConnectionManager::new(
            AgentConnectionRegistry::new(),
            connectors,
            ServiceConfig::default().host_secret_store(),
            None,
        )
    }

    #[test]
    fn connection_id_and_duplicate_config_are_rejected() {
        assert!(AgentConnectionId::new("").is_err());
        assert!(AgentConnectionId::new("contains:separator").is_err());
        let item = config(false);
        assert!(matches!(
            AgentConnectionsConfig {
                revision: 0,
                connections: vec![item.clone(), item],
            }
            .validate(),
            Err(AgentConnectionError::DuplicateConnectionId(_))
        ));
    }

    #[test]
    fn failed_candidate_keeps_the_healthy_generation() {
        let manager = manager(Arc::new(AtomicBool::new(false)));
        manager
            .bootstrap(AgentConnectionsConfig {
                revision: 7,
                connections: vec![config(false)],
            })
            .unwrap();
        let before = manager.registry.statuses()[0].clone();
        assert!(manager.test_connection(config(true)).is_err());
        assert_eq!(manager.registry.statuses()[0], before);
    }

    #[test]
    fn runtime_disconnect_marks_only_the_active_generation_unavailable() {
        let fail = Arc::new(AtomicBool::new(false));
        let manager = manager(fail.clone());
        manager
            .bootstrap(AgentConnectionsConfig {
                revision: 0,
                connections: vec![config(false)],
            })
            .unwrap();
        let id = AgentConnectionId::new("primary").unwrap();
        let mut backend = manager.registry.client_backend(&id).unwrap();
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
    fn successful_management_update_is_revision_fenced_persisted_and_atomic() {
        let root = tempfile::tempdir().unwrap();
        let product = root.path().join("local.toml");
        std::fs::write(
            &product,
            r#"
[[plugins.configured]]
id = "mutsuki.agent.connections"
enabled = true
config = { revision = 0, connections = [{ connection_id = "primary", connector_id = "test.connector", enabled = false, config = {} }] }
"#,
        )
        .unwrap();
        let mut connectors = AgentConnectorCatalog::new();
        connectors
            .register(FakeConnector {
                fail_requests: Arc::new(AtomicBool::new(false)),
            })
            .unwrap();
        let manager = AgentConnectionManager::new(
            AgentConnectionRegistry::new(),
            connectors,
            ServiceConfig::default().host_secret_store(),
            Some(ConfiguredPluginStore::open(&product)),
        );
        let mut declared = config(false);
        declared.enabled = false;
        manager
            .bootstrap(AgentConnectionsConfig {
                revision: 0,
                connections: vec![declared],
            })
            .unwrap();

        let status = manager.upsert("admin", 0, config(false)).unwrap();
        assert_eq!(status.generation, 2);
        assert_eq!(manager.snapshot().revision, 1);
        let persisted: toml::Value =
            toml::from_str(&std::fs::read_to_string(&product).unwrap()).unwrap();
        assert_eq!(
            persisted["plugins"]["configured"][0]["config"]["revision"].as_integer(),
            Some(1)
        );

        let before = std::fs::read_to_string(&product).unwrap();
        assert!(manager.upsert("admin", 1, config(true)).is_err());
        assert_eq!(std::fs::read_to_string(&product).unwrap(), before);
        assert_eq!(manager.snapshot().revision, 1);
        assert_eq!(manager.audits().len(), 1);

        let unknown = AgentConnectionConfig {
            connection_id: AgentConnectionId::new("not-declared").unwrap(),
            ..config(false)
        };
        assert!(matches!(
            manager.upsert("admin", 1, unknown),
            Err(AgentConnectionError::ConnectionNotFound(_))
        ));
    }

    #[tokio::test]
    async fn configured_manifest_provides_each_declared_connection_capability() {
        let fail = Arc::new(AtomicBool::new(false));
        let mut connectors = AgentConnectorCatalog::new();
        connectors
            .register(FakeConnector {
                fail_requests: fail,
            })
            .unwrap();
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
                revision: 0,
                connections: vec![config(false)],
            })
            .unwrap(),
        }];
        let catalog = configured_agent_plugin_catalog(registry.clone(), connectors).unwrap();
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
