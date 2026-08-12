use mutsuki_agent_client::{AgentClient, AgentClientBackend};
use mutsuki_agent_contracts::{
    AgentWireError, AgentWireRequestEnvelope, AgentWireResponseEnvelope,
};
use mutsuki_agent_service_host_integration::AgentConnectionRegistry;
use mutsuki_bot_conversation::ConversationService;
use mutsuki_bot_delivery::{bot_reply_delivery_manifest_for, reply_delivery_runner_for};
use mutsuki_bot_flow::BotFlowRegistry;
use mutsuki_bot_protocol::ConversationPolicy;
use mutsuki_bot_state_db::BotStateDbRepository;
use mutsuki_plugin_bot_adapter_qqbot::{QQBOT_ADAPTER_PLUGIN_ID, QqBotConfig};
use mutsuki_plugin_bot_agent::{
    BOT_AGENT_BRIDGE_PLUGIN_ID, BOT_AGENT_BRIDGE_RUNNER_ID, BOT_AGENT_CONFIG_SERVICE_ID,
    BotAgentBridge, BotAgentConfig, BotAgentConfigHandle, agent_bridge_runner,
    bot_agent_bridge_manifest,
};
use mutsuki_plugin_bot_command::{
    BOT_COMMAND_PLUGIN_ID, BotCommandNodeRunner, bot_command_manifest,
};
use mutsuki_plugin_bot_event_router::{
    BOT_FLOW_REGISTRY_SERVICE_ID, BOT_FLOW_ROUTER_PLUGIN_ID, BotFlowMatchRunner,
    flow_ingress_runner, flow_node_runner, flow_router_manifest,
};
use mutsuki_runtime_contracts::{PluginManifest, RuntimeLoadPlan};
use mutsuki_runtime_sdk::{LoadedPlugin, PluginBuilder, RuntimeBootstrapperService};
use mutsuki_service_config::{ConfiguredPluginStore, HostSecretStore};
use mutsuki_service_runtime::{
    ConfiguredPluginCatalog, ConfiguredPluginFactory, LoadPlanLifecycleHook, ServiceRuntimeBuilder,
    ServiceRuntimeResult,
};
use serde::Deserialize;
use serde_json::Value;

use crate::{
    BILIBILI_MANAGEMENT_SERVICE_ID, BilibiliPollingCredentials, BilibiliPollingEventSource,
    BotReplyDeliveryRecoveryEventSource, QqBotPluginBundle,
};
use mutsuki_plugin_bot_bilibili::{
    BilibiliBackendConfig, BilibiliConfig, BilibiliConfigStore, BilibiliCredentialStore,
    BilibiliManagementService, BilibiliRunner, BilibiliSecretPresence, CredentialSecretState,
    PLUGIN_ID as BILIBILI_PLUGIN_ID, ReqwestBilibiliOpenPlatformTransport,
    ReqwestBilibiliTransport, RuntimeBilibiliQrRenderer, SharedBilibiliConfig,
    SharedBilibiliCredential, SqliteBilibiliRepository,
};
use mutsuki_plugin_bot_bilibili_workshop::{
    PLUGIN_ID as WORKSHOP_PLUGIN_ID, ReqwestWorkshopTransport, WorkshopRunner,
};
use mutsuki_plugin_bot_mihuashi::PLUGIN_ID as MIHUASHI_PLUGIN_ID;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FlowRouterConfig {}

pub struct BotFlowRouterConfiguredPlugin;

struct LegacyBotEventRouterConfiguredPlugin;

impl ConfiguredPluginFactory for LegacyBotEventRouterConfiguredPlugin {
    fn plugin_id(&self) -> &str {
        "mutsuki.bot.router.event"
    }

    fn prepare(
        &self,
        _config: &Value,
        _builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        Err("legacy Bot event subscriptions are unsupported; configure mutsuki.bot.router.flow and publish a graph".into())
    }
}

struct BotFlowLoadPlanHook {
    registry: Arc<BotFlowRegistry>,
}

impl LoadPlanLifecycleHook for BotFlowLoadPlanHook {
    fn validate(&self, plan: &RuntimeLoadPlan) -> Result<(), String> {
        self.registry
            .validate_load_plan(plan)
            .map_err(|error| error.to_string())
    }

    fn activate(&self, plan: &RuntimeLoadPlan) {
        self.registry
            .activate_load_plan(plan)
            .expect("validated Bot Flow LoadPlan must activate");
    }
}

impl ConfiguredPluginFactory for BotFlowRouterConfiguredPlugin {
    fn plugin_id(&self) -> &str {
        BOT_FLOW_ROUTER_PLUGIN_ID
    }

    fn prepare(
        &self,
        config: &Value,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        let config = if config.is_null() {
            Value::Object(Default::default())
        } else {
            config.clone()
        };
        let _config: FlowRouterConfig =
            serde_json::from_value(config).map_err(|error| error.to_string())?;
        let state_dir = builder.data_dir().join("bot");
        std::fs::create_dir_all(&state_dir)
            .map_err(|error| format!("failed to create Bot flow state directory: {error}"))?;
        let repository = Arc::new(
            BotStateDbRepository::open(state_dir.join("state.sqlite3"))
                .map_err(|error| error.to_string())?,
        );
        let registry =
            Arc::new(BotFlowRegistry::restore(repository).map_err(|error| error.to_string())?);
        let manifest = flow_router_manifest();
        let loaded_manifest = manifest.clone();
        let ingress_registry = registry.clone();
        let node_registry = registry.clone();
        let service_registry = registry.clone();
        Ok(builder
            .register_builtin_loaded_plugin_factory(manifest, move || {
                Ok::<LoadedPlugin, String>(LoadedPlugin {
                    manifest: loaded_manifest.clone(),
                    runners: Vec::new(),
                    async_handlers: Vec::new(),
                    host_services: vec![RuntimeBootstrapperService {
                        service_id: BOT_FLOW_REGISTRY_SERVICE_ID.into(),
                        capability: Some("bot.flow".into()),
                        service: service_registry.clone(),
                    }],
                    resource_providers: Vec::new(),
                    async_resource_providers: Vec::new(),
                })
            })
            .register_builtin_runner(move || flow_ingress_runner(ingress_registry.clone()))
            .register_builtin_runner(move || Box::new(BotFlowMatchRunner::default()))
            .register_runtime_client_runner(move |client| {
                flow_node_runner(client, node_registry.clone())
            })
            .register_load_plan_hook(
                BOT_FLOW_REGISTRY_SERVICE_ID,
                Arc::new(BotFlowLoadPlanHook { registry }),
            ))
    }
}

pub struct BotCommandConfiguredPlugin;

impl ConfiguredPluginFactory for BotCommandConfiguredPlugin {
    fn plugin_id(&self) -> &str {
        BOT_COMMAND_PLUGIN_ID
    }

    fn prepare(
        &self,
        config: &Value,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        let config = if config.is_null() {
            Value::Object(Default::default())
        } else {
            config.clone()
        };
        let _config: FlowRouterConfig =
            serde_json::from_value(config).map_err(|error| error.to_string())?;
        Ok(builder
            .register_builtin_plugin(bot_command_manifest(1))
            .register_builtin_runner(move || Box::new(BotCommandNodeRunner::new(1))))
    }
}

const BOT_AGENT_REPLY_DELIVERY_RUNNER_ID: &str = "mutsuki.bot.agent.reply-delivery.runner";

struct ConfigSelectedAgentBackend {
    connections: AgentConnectionRegistry,
    config: BotAgentConfigHandle,
}

impl AgentClientBackend for ConfigSelectedAgentBackend {
    fn request(
        &mut self,
        request: AgentWireRequestEnvelope,
    ) -> Result<AgentWireResponseEnvelope, AgentWireError> {
        let connection_id = self
            .config
            .snapshot()
            .selected_connection_id()
            .map_err(|error| AgentWireError {
                code: "bot.agent.connection.invalid".into(),
                message: error.to_string(),
                retryable: false,
            })?
            .ok_or_else(|| AgentWireError {
                code: "bot.agent.disabled".into(),
                message: "Bot Agent is disabled".into(),
                retryable: false,
            })?;
        self.connections
            .client_backend(&connection_id)
            .request(request)
    }
}

pub struct BotAgentConfiguredPlugin {
    connections: AgentConnectionRegistry,
}

impl BotAgentConfiguredPlugin {
    #[must_use]
    pub fn new(connections: AgentConnectionRegistry) -> Self {
        Self { connections }
    }
}

impl ConfiguredPluginFactory for BotAgentConfiguredPlugin {
    fn plugin_id(&self) -> &str {
        BOT_AGENT_BRIDGE_PLUGIN_ID
    }

    fn prepare(
        &self,
        config: &Value,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        let config: BotAgentConfig =
            serde_json::from_value(config.clone()).map_err(|error| error.to_string())?;
        config.validate().map_err(|error| error.to_string())?;
        let config_handle =
            BotAgentConfigHandle::new(config.clone()).map_err(|error| error.to_string())?;
        if !config.enabled {
            return Ok(register_bot_agent_services(
                builder,
                PluginBuilder::new(BOT_AGENT_BRIDGE_PLUGIN_ID)
                    .build()
                    .manifest,
                config_handle,
            ));
        }
        let state_dir = builder.data_dir().join("bot");
        std::fs::create_dir_all(&state_dir).map_err(|error| {
            format!(
                "failed to create Bot Agent state directory {}: {error}",
                state_dir.display()
            )
        })?;
        let repository = Arc::new(
            BotStateDbRepository::open(state_dir.join("state.sqlite3"))
                .map_err(|error| error.to_string())?,
        );
        let connection_id = config
            .selected_connection_id()
            .map_err(|error| error.to_string())?
            .expect("enabled Bot Agent config has a connection id");
        let backend = ConfigSelectedAgentBackend {
            connections: self.connections.clone(),
            config: config_handle.clone(),
        };
        let client = AgentClient::new(backend);
        let conversations = ConversationService::new(
            repository.clone(),
            execution_product_policy(&config).map_err(|error| error.to_string())?,
        );
        let bridge =
            BotAgentBridge::new_with_config(conversations, Box::new(client), config_handle.clone());

        let mut manifest = merge_manifests(
            bot_agent_bridge_manifest(),
            bot_reply_delivery_manifest_for(
                BOT_AGENT_BRIDGE_PLUGIN_ID,
                BOT_AGENT_REPLY_DELIVERY_RUNNER_ID,
            ),
        );
        manifest.requires.push(connection_id.capability());
        let builder = register_bot_agent_services(builder, manifest, config_handle.clone());
        Ok(builder
            .register_event_source(Box::new(BotReplyDeliveryRecoveryEventSource::for_plugin(
                Duration::from_millis(250),
                BOT_AGENT_BRIDGE_PLUGIN_ID,
            )))
            .register_dynamic_runner_limit(BOT_AGENT_BRIDGE_RUNNER_ID, {
                let config = config_handle.clone();
                move || {
                    let settings = config.snapshot();
                    (Some(settings.max_concurrency), Some(settings.timeout_ms))
                }
            })
            .register_runtime_client_runner(move |client| {
                agent_bridge_runner(client, bridge.clone())
            })
            .register_runtime_client_runner(move |client| {
                reply_delivery_runner_for(
                    client,
                    repository.clone(),
                    BOT_AGENT_BRIDGE_PLUGIN_ID,
                    BOT_AGENT_REPLY_DELIVERY_RUNNER_ID,
                )
            }))
    }
}

fn register_bot_agent_services(
    builder: ServiceRuntimeBuilder,
    manifest: PluginManifest,
    config: BotAgentConfigHandle,
) -> ServiceRuntimeBuilder {
    let loaded_manifest = manifest.clone();
    let config = Arc::new(config);
    builder.register_builtin_loaded_plugin_factory(manifest, move || {
        Ok::<LoadedPlugin, String>(LoadedPlugin {
            manifest: loaded_manifest.clone(),
            runners: Vec::new(),
            async_handlers: Vec::new(),
            host_services: vec![RuntimeBootstrapperService {
                service_id: BOT_AGENT_CONFIG_SERVICE_ID.into(),
                capability: Some("bot.agent.config".into()),
                service: config.clone(),
            }],
            resource_providers: Vec::new(),
            async_resource_providers: Vec::new(),
        })
    })
}

fn merge_manifests(mut left: PluginManifest, right: PluginManifest) -> PluginManifest {
    debug_assert_eq!(left.plugin_id, right.plugin_id);
    left.provides
        .capabilities
        .extend(right.provides.capabilities);
    left.provides.runners.extend(right.provides.runners);
    left.provides.protocols.extend(right.provides.protocols);
    left.provides
        .protocol_classes
        .extend(right.provides.protocol_classes);
    left.provides
        .handler_bindings
        .extend(right.provides.handler_bindings);
    left.provides.extensions.extend(right.provides.extensions);
    left
}

fn execution_product_policy(config: &BotAgentConfig) -> Result<ConversationPolicy, String> {
    Ok(ConversationPolicy {
        revision: 0,
        session_scope: config.session_scope().map_err(|error| error.to_string())?,
        business_profile_binding_id: None,
        agent_runtime_profile_id: (!config.default_profile_id.trim().is_empty())
            .then(|| config.default_profile_id.clone()),
        stt_enabled: config.stt_enabled,
        tts_enabled: config.tts_enabled,
        speech_reply_policy: config
            .speech_reply_policy()
            .map_err(|error| error.to_string())?,
        stt_selector_id: (!config.stt_selector_id.trim().is_empty())
            .then(|| config.stt_selector_id.clone()),
        tts_selector_id: (!config.tts_selector_id.trim().is_empty())
            .then(|| config.tts_selector_id.clone()),
        active_delivery_enabled: false,
    })
}

pub struct QqBotConfiguredPlugin;

impl ConfiguredPluginFactory for QqBotConfiguredPlugin {
    fn plugin_id(&self) -> &str {
        QQBOT_ADAPTER_PLUGIN_ID
    }

    fn prepare(
        &self,
        config: &Value,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        let config: QqBotConfig =
            serde_json::from_value(config.clone()).map_err(|error| error.to_string())?;
        let media_provider_id = config.media_provider_id.clone();
        let mut bundle =
            QqBotPluginBundle::new(config).map_err(|error| error.redacted_message())?;
        if let Some(provider_id) = media_provider_id {
            bundle = bundle.with_resource_media_provider(provider_id);
        }
        bundle
            .install(builder)
            .map_err(|error| error.redacted_message())
    }
}

pub struct BilibiliConfiguredPlugin;

struct HostBilibiliCredentialStore {
    host: HostSecretStore,
    shared: SharedBilibiliCredential,
}

impl BilibiliCredentialStore for HostBilibiliCredentialStore {
    fn rotate(&self, key: &str, credential: String) -> Result<(), String> {
        self.host
            .rotate(key, credential.clone())
            .map_err(|error| error.to_string())?;
        self.shared.set(credential);
        Ok(())
    }
}

struct HostBilibiliConfigStore(ConfiguredPluginStore);

impl BilibiliConfigStore for HostBilibiliConfigStore {
    fn replace(&self, config: &BilibiliConfig) -> Result<(), String> {
        let value = serde_json::to_value(config).map_err(|error| error.to_string())?;
        self.0
            .replace_config(BILIBILI_PLUGIN_ID, value)
            .map_err(|error| error.to_string())
    }
}

struct HostSecretPresence(HostSecretStore);

impl BilibiliSecretPresence for HostSecretPresence {
    fn inspect(&self, key: &str) -> CredentialSecretState {
        match self.0.resolve(key) {
            None => CredentialSecretState::Absent,
            Some(value) if value.trim().is_empty() => CredentialSecretState::Invalid,
            Some(_) => CredentialSecretState::Present,
        }
    }
}

impl ConfiguredPluginFactory for BilibiliConfiguredPlugin {
    fn plugin_id(&self) -> &str {
        BILIBILI_PLUGIN_ID
    }

    fn prepare(
        &self,
        config: &Value,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        let config: BilibiliConfig =
            serde_json::from_value(config.clone()).map_err(|error| error.to_string())?;
        config.validate()?;
        let host_secret_store = builder.host_secret_store();
        let configured_plugin_store = builder.configured_plugin_store();
        if matches!(config.backend, BilibiliBackendConfig::OpenPlatform { .. })
            && !host_secret_store.rotation_available()
        {
            return Err(
                "Bilibili Open Platform requires a Host security.secret_file for OAuth refresh"
                    .into(),
            );
        }
        if config.management.enabled {
            if !matches!(config.backend, BilibiliBackendConfig::WebCookie { .. }) {
                return Err("Bilibili management requires backend.type = web_cookie".into());
            }
            if !host_secret_store.rotation_available() {
                return Err("Bilibili management requires a Host security.secret_file".into());
            }
        }
        let configured_plugin_store = if config.management.enabled {
            Some(configured_plugin_store.ok_or_else(|| {
                "Bilibili management requires a loaded product config file".to_string()
            })?)
        } else {
            None
        };
        let repository = Arc::new(
            SqliteBilibiliRepository::open(builder.data_dir().join("bilibili/state.sqlite3"))
                .map_err(|error| error.to_string())?,
        );
        let web_credential = SharedBilibiliCredential::default();
        let app_secret = SharedBilibiliCredential::default();
        let oauth_credential = SharedBilibiliCredential::default();
        let shared_config = SharedBilibiliConfig::new(config);
        let runner_config = shared_config.clone();
        let runner_repository = repository.clone();
        let runner_web_credential = web_credential.clone();
        let runner_app_secret = app_secret.clone();
        let runner_oauth_credential = oauth_credential.clone();
        let source_credentials = match &shared_config.snapshot().backend {
            BilibiliBackendConfig::WebCookie { cookie_secret_key } => {
                BilibiliPollingCredentials::WebCookie {
                    secret_key: cookie_secret_key.clone(),
                    credential: web_credential.clone(),
                    required: !shared_config.snapshot().management.enabled,
                }
            }
            BilibiliBackendConfig::OpenPlatform {
                app_secret_key,
                oauth_credential_key,
                ..
            } => BilibiliPollingCredentials::OpenPlatform {
                app_secret_key: app_secret_key.clone(),
                app_secret: app_secret.clone(),
                oauth_credential_key: oauth_credential_key.clone(),
                oauth_credential: oauth_credential.clone(),
            },
        };
        let source = BilibiliPollingEventSource::new(shared_config.clone(), source_credentials);
        let manifest_config = runner_config.snapshot();
        let mut manifest = mutsuki_plugin_bot_bilibili::manifest_for_config(&manifest_config);
        manifest.requires.push(format!(
            "resource_strategy:{}",
            runner_config.snapshot().media_provider_id
        ));

        let management_service = if let Some(store) = configured_plugin_store {
            let service = Arc::new(BilibiliManagementService::new(
                runner_config.clone(),
                web_credential.clone(),
                Box::new(ReqwestBilibiliTransport::new(
                    web_credential.clone(),
                    Duration::from_secs(15),
                )),
                repository.clone(),
                Arc::new(HostBilibiliCredentialStore {
                    host: host_secret_store.clone(),
                    shared: web_credential.clone(),
                }),
                Arc::new(HostBilibiliConfigStore(store)),
                Arc::new(HostSecretPresence(host_secret_store.clone())),
            ));
            Some(service)
        } else {
            None
        };

        let builder = if let Some(service) = management_service.clone() {
            let loaded_manifest = manifest.clone();
            builder.register_builtin_loaded_plugin_factory(manifest, move || {
                Ok::<LoadedPlugin, String>(LoadedPlugin {
                    manifest: loaded_manifest.clone(),
                    runners: Vec::new(),
                    async_handlers: Vec::new(),
                    host_services: vec![RuntimeBootstrapperService {
                        service_id: BILIBILI_MANAGEMENT_SERVICE_ID.into(),
                        capability: None,
                        service: service.clone(),
                    }],
                    resource_providers: Vec::new(),
                    async_resource_providers: Vec::new(),
                })
            })
        } else {
            builder.register_builtin_plugin(manifest)
        };

        Ok(builder
            .register_fallible_runtime_services_runner(move |client, resources| {
                let snapshot = runner_config.snapshot();
                let transport: Box<dyn mutsuki_plugin_bot_bilibili::BilibiliTransport> =
                    match &snapshot.backend {
                        BilibiliBackendConfig::WebCookie { .. } => {
                            Box::new(ReqwestBilibiliTransport::new(
                                runner_web_credential.clone(),
                                Duration::from_secs(15),
                            ))
                        }
                        BilibiliBackendConfig::OpenPlatform {
                            client_id,
                            oauth_credential_key,
                            authorized_uid,
                            ..
                        } => Box::new(ReqwestBilibiliOpenPlatformTransport::new(
                            client_id,
                            *authorized_uid,
                            runner_app_secret.clone(),
                            runner_oauth_credential.clone(),
                            oauth_credential_key,
                            Arc::new(HostBilibiliCredentialStore {
                                host: host_secret_store.clone(),
                                shared: runner_oauth_credential.clone(),
                            }),
                            Duration::from_secs(15),
                        )),
                    };
                let mut runner = BilibiliRunner::new_for_backend(
                    transport,
                    runner_repository.clone(),
                    resources.clone(),
                    snapshot.media_provider_id.clone(),
                    snapshot.backend.kind(),
                );
                if snapshot.management.enabled {
                    let management = management_service.clone().ok_or_else(|| {
                        mutsuki_plugin_bot_bilibili::BilibiliError::ManagementUnavailable(
                            "Bilibili management service is unavailable".into(),
                        )
                    })?;
                    management.bind_qr_renderer(Arc::new(RuntimeBilibiliQrRenderer::new(
                        client.clone(),
                        resources,
                    )));
                    runner = runner.with_management(management);
                }
                Ok::<
                    Box<dyn mutsuki_runtime_core::Runner>,
                    mutsuki_plugin_bot_bilibili::BilibiliError,
                >(runner.into_runtime_runner(client, snapshot.risk_control.clone()))
            })
            .register_event_source(Box::new(source)))
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LinkCardPluginConfig {
    media_provider_id: String,
}

impl LinkCardPluginConfig {
    fn validate(&self) -> Result<(), String> {
        if self.media_provider_id.trim().is_empty() {
            return Err("media_provider_id is required".into());
        }
        Ok(())
    }
}

pub struct WorkshopConfiguredPlugin;

impl ConfiguredPluginFactory for WorkshopConfiguredPlugin {
    fn plugin_id(&self) -> &str {
        WORKSHOP_PLUGIN_ID
    }

    fn prepare(
        &self,
        config: &Value,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        let config: LinkCardPluginConfig =
            serde_json::from_value(config.clone()).map_err(|error| error.to_string())?;
        config.validate()?;
        let mut manifest = mutsuki_plugin_bot_bilibili_workshop::manifest();
        manifest
            .requires
            .push(format!("resource_strategy:{}", config.media_provider_id));
        Ok(builder
            .register_builtin_plugin(manifest)
            .register_fallible_runtime_services_runner(move |_client, resources| {
                let transport = ReqwestWorkshopTransport::new();
                Ok::<Box<dyn mutsuki_runtime_core::Runner>, String>(Box::new(WorkshopRunner::new(
                    Box::new(transport),
                    resources,
                    config.media_provider_id.clone(),
                )))
            }))
    }
}

pub struct MihuashiConfiguredPlugin;

impl ConfiguredPluginFactory for MihuashiConfiguredPlugin {
    fn plugin_id(&self) -> &str {
        MIHUASHI_PLUGIN_ID
    }

    fn prepare(
        &self,
        config: &Value,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        let config: LinkCardPluginConfig =
            serde_json::from_value(config.clone()).map_err(|error| error.to_string())?;
        config.validate()?;
        let mut manifest = mutsuki_plugin_bot_mihuashi::manifest();
        manifest
            .requires
            .push(format!("resource_strategy:{}", config.media_provider_id));
        manifest
            .requires
            .push("task_protocol:mutsuki.browser.snapshot".into());
        Ok(builder
            .register_builtin_plugin(manifest)
            .register_runtime_services_runner(move |client, resources| {
                mutsuki_plugin_bot_mihuashi::runner(
                    client,
                    resources,
                    config.media_provider_id.clone(),
                )
            }))
    }
}

/// Catalog of production Bot plugins that can be selected by ServiceHost configuration.
/// Media upload is intentionally absent until a product registers an explicit provider-backed
/// QQ factory of its own.
pub fn configured_bot_plugin_catalog() -> ServiceRuntimeResult<ConfiguredPluginCatalog> {
    let mut catalog = ConfiguredPluginCatalog::new();
    catalog.register(LegacyBotEventRouterConfiguredPlugin)?;
    catalog.register(BotFlowRouterConfiguredPlugin)?;
    catalog.register(BotCommandConfiguredPlugin)?;
    catalog.register(QqBotConfiguredPlugin)?;
    catalog.register(BilibiliConfiguredPlugin)?;
    catalog.register(WorkshopConfiguredPlugin)?;
    catalog.register(MihuashiConfiguredPlugin)?;
    Ok(catalog)
}

/// Production Bot catalog with configurable Agent nodes wired to a shared Agent owner
/// registry. The base catalog intentionally remains Agent-free for products that do not opt in.
pub fn configured_bot_plugin_catalog_with_agent(
    connections: AgentConnectionRegistry,
) -> ServiceRuntimeResult<ConfiguredPluginCatalog> {
    let mut catalog = configured_bot_plugin_catalog()?;
    catalog.register(BotAgentConfiguredPlugin::new(connections))?;
    Ok(catalog)
}

#[cfg(test)]
mod tests {
    use mutsuki_service_config::{ConfiguredPluginSelection, ServiceConfig};
    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn configured_qq_plugin_fails_preflight_without_host_secret() {
        let mut service = ServiceConfig::default();
        service.ipc.enabled = false;
        service.observe.console = false;
        service.plugins.dynamic_dirs.clear();
        service.plugins.configured = vec![ConfiguredPluginSelection {
            id: QQBOT_ADAPTER_PLUGIN_ID.into(),
            enabled: true,
            config: json!({
                "account_id": "configured",
                "app_id": "APP_ID",
                "client_secret_key": "MISSING_CONFIGURED_QQ_SECRET"
            }),
        }];

        let error = match ServiceRuntimeBuilder::new(service)
            .with_configured_plugin_catalog(configured_bot_plugin_catalog().unwrap())
            .start()
            .await
        {
            Ok(runtime) => {
                runtime.shutdown().await;
                panic!("configured QQBot unexpectedly started")
            }
            Err(error) => error,
        };
        assert!(error.to_string().contains("MISSING_CONFIGURED_QQ_SECRET"));
    }

    #[test]
    fn graph_only_plugins_accept_omitted_empty_config() {
        let root = tempfile::tempdir().unwrap();
        let mut service = ServiceConfig::default();
        service.service.data_dir = root.path().join("data");

        BotFlowRouterConfiguredPlugin
            .prepare(&Value::Null, ServiceRuntimeBuilder::new(service.clone()))
            .expect("Flow Router should accept a graph-only selection without a config table");
        BotCommandConfiguredPlugin
            .prepare(&Value::Null, ServiceRuntimeBuilder::new(service))
            .expect(
                "Command node plugin should accept a graph-only selection without a config table",
            );
    }

    #[test]
    fn configured_bilibili_management_requires_host_persistence_boundaries() {
        let config = json!({
            "backend": {"type": "web_cookie", "cookie_secret_key": "BILIBILI_COOKIE"},
            "live_interval_ms": 1000,
            "dynamic_interval_ms": 1000,
            "video_interval_ms": 1000,
            "retry": {"max_attempts": 3, "initial_backoff_ms": 10, "max_backoff_ms": 100},
            "subscriptions": [],
            "link_resolver": {"enabled": false, "cooldown_ms": 1000, "account_to_binding": {}},
            "media_provider_id": "memory",
            "management": {
                "enabled": true,
                "allow_self_binding": true,
                "admin_user_ids": ["admin"],
                "self_binding_notifications": ["dynamic"],
                "self_binding_outbound_binding": "qq-main"
            }
        });
        let error = match BilibiliConfiguredPlugin.prepare(
            &config,
            ServiceRuntimeBuilder::new(ServiceConfig::default()),
        ) {
            Ok(_) => panic!("Bilibili management unexpectedly accepted missing Host stores"),
            Err(error) => error,
        };
        assert!(error.contains("security.secret_file"));
    }

    #[test]
    fn configured_bilibili_open_platform_requires_rotatable_oauth_store() {
        let config = json!({
            "backend": {
                "type": "open_platform",
                "client_id": "client",
                "app_secret_key": "BILIBILI_OPEN_APP_SECRET",
                "oauth_credential_key": "BILIBILI_OPEN_OAUTH",
                "authorized_uid": 42
            },
            "live_interval_ms": 1000,
            "dynamic_interval_ms": 1000,
            "video_interval_ms": 1000,
            "retry": {"max_attempts": 3, "initial_backoff_ms": 10, "max_backoff_ms": 100},
            "subscriptions": [],
            "link_resolver": {"enabled": false, "cooldown_ms": 1000, "account_to_binding": {}},
            "media_provider_id": "memory",
            "management": {
                "enabled": false,
                "allow_self_binding": false,
                "admin_user_ids": [],
                "self_binding_notifications": ["live", "video"],
                "self_binding_outbound_binding": ""
            }
        });
        let error = BilibiliConfiguredPlugin
            .prepare(
                &config,
                ServiceRuntimeBuilder::new(ServiceConfig::default()),
            )
            .err()
            .expect("Open Platform unexpectedly accepted a non-rotatable secret store");
        assert!(error.contains("OAuth refresh"));
    }

    #[test]
    fn legacy_orchestration_configuration_is_rejected_at_the_owner_boundary() {
        let builder = ServiceRuntimeBuilder::new(ServiceConfig::default());
        let error = LegacyBotEventRouterConfiguredPlugin
            .prepare(&json!({"subscriptions": []}), builder)
            .err()
            .expect("legacy event router must be rejected");
        assert!(error.contains("publish a graph"));

        let error = BotCommandConfiguredPlugin
            .prepare(
                &json!({"prefixes": ["/"], "commands": []}),
                ServiceRuntimeBuilder::new(ServiceConfig::default()),
            )
            .err()
            .expect("legacy command config must be rejected");
        assert!(error.contains("unknown field"));

        assert!(
            serde_json::from_value::<mutsuki_plugin_bot_bilibili::BilibiliManagementConfig>(
                json!({
                    "enabled": false,
                    "allow_self_binding": false,
                    "command": "bili",
                    "admin_user_ids": [],
                    "self_binding_notifications": [],
                    "self_binding_outbound_binding": ""
                }),
            )
            .is_err()
        );
    }
}
