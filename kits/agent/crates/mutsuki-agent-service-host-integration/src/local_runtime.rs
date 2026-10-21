//! ServiceHost wiring for the embedded local Agent engine (`mutsuki-agent-local-engine`):
//! Host secret credentials, the configured plugin factory, health probe and the load-plan hook
//! that installs the local connection.

use std::sync::{Arc, Mutex};

use mutsuki_agent_adapter_api::{CredentialBroker, CredentialFuture, CredentialValue};
use mutsuki_agent_client::{AgentClient, AgentClientBackend, InProcessAgentClient};
use mutsuki_agent_contracts::{AgentWireNegotiation, CredentialRef};
use mutsuki_agent_local_engine::{
    LOCAL_AGENT_CONNECTION_ID, LOCAL_AGENT_MANAGEMENT_SERVICE_ID, LOCAL_AGENT_PLUGIN_ID,
    LOCAL_AGENT_SERVICE_ID, LocalAgentConfig, LocalAgentManagementService,
    LocalAgentRuntimeExtension, LocalAgentRuntimeService,
};
use mutsuki_runtime_sdk::{LoadedPlugin, PluginBuilder, RuntimeBootstrapperService};
use mutsuki_service_config::HostSecretStore;
use mutsuki_service_runtime::{
    ConfiguredPluginFactory, LoadPlanLifecycleHook, ServiceRuntimeBuilder,
};
use serde_json::{Value, json};

use crate::AgentConnectionRegistry;

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
        let credentials: Arc<dyn CredentialBroker> = Arc::new(HostSecretCredentialBroker {
            secrets: builder.host_secret_store(),
            key: config.api_key_key.clone(),
        });
        let service = LocalAgentRuntimeService::boot(
            &config,
            credentials,
            &builder.data_dir().join("agent/local/state.sqlite3"),
            &self.extensions,
        )?;
        let management = Arc::new(LocalAgentManagementService::new(service.clone(), {
            let registry = self.registry.clone();
            Arc::new(move || {
                let connection_id = LOCAL_AGENT_CONNECTION_ID
                    .parse()
                    .expect("fixed local Agent connection id is valid");
                registry.client_backend(&connection_id)
            })
        }));
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
