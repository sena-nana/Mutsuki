use mutsuki_agent_client::{AgentClient, AgentClientBackend};
use mutsuki_agent_contracts::{
    AgentWireError, AgentWireRequestEnvelope, AgentWireResponseEnvelope,
};
use mutsuki_agent_service_host_integration::AgentConnectionRegistry;
use mutsuki_bot_conversation::{ConversationContextStore, ConversationService, PersonaStore};
use mutsuki_bot_flow::{
    BOT_FLOW_CONFIG_PROVIDER_ID, BotFlowConfigProvider, BotFlowRegistry, BotNodeCatalog,
};
use mutsuki_bot_interaction::{InteractionService, UnavailableInteractionMatcher};

use mutsuki_bot_protocol::{BotFlowDocument, ConversationPolicy};
use mutsuki_bot_sandbox::{SANDBOX_SERVICE_ID, SandboxService};
use mutsuki_bot_sdk::{BotManifestSurface, BotSubmissionGate};
use mutsuki_bot_state_db::BotStateDbRepository;
use mutsuki_config_service::{
    ConfigConstraints, ConfigContext, ConfigDescriptor, ConfigDocumentKey, ConfigKey,
    ConfigMutability, ConfigNode, ConfigPresentation, ConfigProviderId, ConfigProviderRegistration,
    ConfigScope, ConfigService, ConfigValue, ConfigValueType, LocalizedText, RestartPolicy,
};
use mutsuki_plugin_bot_agent::{
    BOT_AGENT_BRIDGE_PLUGIN_ID, BOT_AGENT_BRIDGE_RUNNER_ID, BOT_AGENT_CONFIG_SERVICE_ID,
    BotAgentBridge, BotAgentConfig, BotAgentConfigHandle, agent_bridge_runner,
    bot_agent_bridge_manifest,
};
use mutsuki_plugin_bot_command::{
    BOT_COMMAND_PLUGIN_ID, BotCommandNodeRunner, bot_command_manifest,
};
use mutsuki_plugin_bot_conversation_context::{
    ConversationContextRunner, bot_conversation_context_manifest,
};
use mutsuki_plugin_bot_delivery::{bot_reply_delivery_manifest_for, reply_delivery_runner_for};
use mutsuki_plugin_bot_event_router::{
    BOT_FLOW_REGISTRY_SERVICE_ID, BOT_FLOW_ROUTER_PLUGIN_ID, BotFlowMatchRunner,
    flow_ingress_runner, flow_node_runner,
};
use mutsuki_plugin_bot_interaction::{
    BOT_INTERACTION_PLUGIN_ID, InteractionCreateRunner, InteractionMatchRunner,
    bot_interaction_manifest, interaction_runner,
};
use mutsuki_plugin_bot_persona::{PersonaRunner, bot_persona_manifest};

use mutsuki_plugin_bot_reply::{BotReplyRunner, bot_reply_manifest};
use mutsuki_runtime_contracts::{
    ContractSurfaceKind, PluginManifest, RuntimeLoadPlan, SurfaceRequirement,
};
use mutsuki_runtime_sdk::{
    HostEffect, HostEffectFuture, HostEffectKind, LoadedPlugin, PluginBuilder,
    RuntimeBootstrapperEffect, RuntimeBootstrapperService,
};
use mutsuki_service_runtime::{
    ConfiguredPluginCatalog, ConfiguredPluginFactory, LoadPlanObserver, ServiceRuntimeBuilder,
    ServiceRuntimeResult,
};
use serde_json::Value;

use crate::BotTaskRecoveryEventSource;
use mutsuki_plugin_bot_bilibili_workshop::PLUGIN_ID as WORKSHOP_PLUGIN_ID;
use mutsuki_plugin_bot_mihuashi::PLUGIN_ID as MIHUASHI_PLUGIN_ID;

/// Media resource provider id for assemblies that do not inject their own
/// binding (tests, benchmarks and examples). Products pass the persistent
/// SQLite provider explicitly.
pub const DEFAULT_MEDIA_PROVIDER_ID: &str = mutsuki_plugin_resource_sqlite::PROVIDER_ID;
use std::sync::Arc;

/// Owns a staged config provider registration as a reversible Host effect.
pub struct ConfigProviderEffect(pub ConfigProviderRegistration);

impl HostEffect for ConfigProviderEffect {
    fn dispose(&mut self) -> HostEffectFuture<'_> {
        Box::pin(async move {
            let _ = self.0.dispose();
            Ok(())
        })
    }
}

pub struct BotFlowRouterConfiguredPlugin {
    config: Arc<ConfigService>,
    registry: Option<Arc<BotFlowRegistry>>,
    seed: Option<BotFlowDocument>,
}

impl BotFlowRouterConfiguredPlugin {
    #[must_use]
    pub fn new(config: Arc<ConfigService>) -> Self {
        Self {
            config,
            registry: None,
            seed: None,
        }
    }

    #[must_use]
    pub fn with_registry(config: Arc<ConfigService>, registry: Arc<BotFlowRegistry>) -> Self {
        Self {
            config,
            registry: Some(registry),
            seed: None,
        }
    }
}

struct BotFlowLoadPlanObserver {
    registry: Arc<BotFlowRegistry>,
    config: Arc<ConfigService>,
    seed: Option<BotFlowDocument>,
    status: Arc<BotFlowSeedStatus>,
}

impl LoadPlanObserver for BotFlowLoadPlanObserver {
    fn validate(&self, plan: &RuntimeLoadPlan) -> Result<(), String> {
        self.registry
            .validate_load_plan(plan)
            .map_err(|error| error.to_string())
    }

    fn activate(&self, plan: &RuntimeLoadPlan) {
        self.registry
            .activate_load_plan(plan)
            .expect("validated Bot Flow LoadPlan catalog must activate");
        let key = ConfigDocumentKey::new(BOT_FLOW_CONFIG_PROVIDER_ID, ConfigContext::global());
        let config = self.config.clone();
        if self.config.repository().read(&key).ok().flatten().is_some() {
            let status = self.status.clone();
            tokio::spawn(async move {
                if let Err(error) = config
                    .restore(BOT_FLOW_CONFIG_PROVIDER_ID, ConfigContext::global())
                    .await
                {
                    status.record_failure("restore", &error.to_string());
                    tracing::error!(
                        error = %error,
                        "stored Bot Flow could not be restored after startup; routing stays on an empty graph"
                    );
                }
            });
            return;
        }
        let Some(seed) = self.seed.clone() else {
            return;
        };
        let status = self.status.clone();
        tokio::spawn(async move {
            let candidate = ConfigValue::from_json(&serde_json::json!({ "flow": seed }));
            if let Err(error) = config
                .create_if_absent(
                    BOT_FLOW_CONFIG_PROVIDER_ID,
                    candidate,
                    ConfigContext::global(),
                )
                .await
            {
                status.record_failure("seed", &error.to_string());
                tracing::error!(
                    error = %error,
                    "Bot Flow seed was not applied; routing stays on an empty graph"
                );
            }
        });
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
        mutsuki_bot_sdk::config::decode_empty(self.plugin_id(), &config)?;
        let registry = self
            .registry
            .clone()
            .unwrap_or_else(|| Arc::new(BotFlowRegistry::new(BotNodeCatalog::default())));
        let provider = Arc::new(BotFlowConfigProvider::new(registry.clone()));
        let mut manifest =
            mutsuki_plugin_bot_event_router::flow_router_manifest_for_catalog(&registry.catalog());
        manifest
            .provides
            .services
            .push(BOT_FLOW_REGISTRY_SERVICE_ID.into());
        manifest.provides.capabilities.push("bot.flow".into());
        let loaded_manifest = manifest.clone();
        let ingress_registry = registry.clone();
        let node_registry = registry.clone();
        let service_registry = registry.clone();
        let ingress_stats_registry = registry.clone();
        let seed_status = Arc::new(BotFlowSeedStatus::default());
        let probe_status = seed_status.clone();
        let config_service = self.config.clone();
        Ok(builder
            .register_builtin_loaded_plugin_factory(manifest, move || {
                let registration = config_service
                    .register_provider_staged(provider.clone())
                    .map_err(|error| error.to_string())?;
                Ok::<LoadedPlugin, String>(LoadedPlugin {
                    manifest: loaded_manifest.clone(),
                    runners: Vec::new(),
                    async_handlers: Vec::new(),
                    host_services: vec![RuntimeBootstrapperService::new(
                        BOT_FLOW_REGISTRY_SERVICE_ID,
                        service_registry.clone(),
                        "bot.flow",
                    )],
                    resource_providers: Vec::new(),
                    async_resource_providers: Vec::new(),
                    host_effects: vec![RuntimeBootstrapperEffect {
                        kind: HostEffectKind::HostLocal,
                        effect: Box::new(ConfigProviderEffect(registration)),
                    }],
                })
            })
            .register_builtin_runner(move || flow_ingress_runner(ingress_registry.clone()))
            .register_health_probe("mutsuki.bot.flow.ingress", move || {
                let stats = ingress_stats_registry.ingress_stats();
                let accepted_total = stats.accepted_total();
                let dropped_total = stats.dropped_total();
                let active_nodes = ingress_stats_registry.active().flow.nodes.len();
                let seed_failure = probe_status.failure();
                // An empty active graph freezes every business behavior behind
                // Flow. That is a legitimate state before the operator saves a
                // graph, so it only counts as degraded once ingress has actually
                // seen traffic it had nowhere to route.
                let frozen = active_nodes == 0 && accepted_total > 0;
                let health = if seed_failure.is_some() || frozen {
                    "degraded"
                } else {
                    "ok"
                };
                let mut snapshot = serde_json::json!({
                    "status": health,
                    "accepted_total": accepted_total,
                    "dropped_total": dropped_total,
                    "active_flow_nodes": active_nodes,
                });
                if let Some(failure) = seed_failure {
                    snapshot["seed_failure"] = serde_json::Value::String(failure);
                }
                if frozen {
                    snapshot["frozen_reason"] = serde_json::Value::String(
                        "ingress accepted events while the active Flow graph is empty".into(),
                    );
                }
                snapshot
            })
            .register_builtin_runner(move || Box::new(BotFlowMatchRunner::default()))
            .register_runtime_client_runner(move |client| {
                flow_node_runner(client, node_registry.clone())
            })
            .register_load_plan_observer(
                BOT_FLOW_REGISTRY_SERVICE_ID,
                Arc::new(BotFlowLoadPlanObserver {
                    registry,
                    config: self.config.clone(),
                    seed: self.seed.clone(),
                    status: seed_status,
                }),
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
        mutsuki_bot_sdk::config::decode_empty(self.plugin_id(), &config)?;
        Ok(builder
            .register_builtin_plugin(mutsuki_bot_sdk::manifest::checked(
                bot_command_manifest(1),
                BotManifestSurface::Business,
            )?)
            .register_builtin_runner(move || Box::new(BotCommandNodeRunner::new(1))))
    }
}

struct BotInteractionConfiguredPlugin;

impl ConfiguredPluginFactory for BotInteractionConfiguredPlugin {
    fn plugin_id(&self) -> &str {
        BOT_INTERACTION_PLUGIN_ID
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
        mutsuki_bot_sdk::config::decode_empty(self.plugin_id(), &config)?;
        let (builder, repository) = bot_state_db(builder)?;
        let interaction =
            InteractionService::new(repository, Arc::new(UnavailableInteractionMatcher));
        Ok(builder
            .register_builtin_plugin(mutsuki_bot_sdk::manifest::checked(
                bot_interaction_manifest(),
                BotManifestSurface::Business,
            )?)
            .register_builtin_runner({
                let interaction = interaction.clone();
                move || Box::new(InteractionMatchRunner::new(interaction.clone()))
            })
            .register_builtin_runner({
                let interaction = interaction.clone();
                move || Box::new(InteractionCreateRunner::new(interaction.clone()))
            })
            .register_runtime_client_runner({
                let interaction = interaction.clone();
                move |client| interaction_runner(client, interaction.clone())
            })
            .register_event_source(Box::new(BotTaskRecoveryEventSource::interaction(
                BotTaskRecoveryEventSource::default_interval(),
                BOT_INTERACTION_PLUGIN_ID,
            ))))
    }
}

/// Outcome of the last Flow document seed or restore attempt.
///
/// Both run on a detached task after LoadPlan activation, so a failure there used
/// to reach the operator as a log line only while the process reported healthy and
/// routed on an empty graph -- every business behavior frozen, nothing structurally
/// failed. The health probe reads this back so the freeze is visible.
#[derive(Default)]
struct BotFlowSeedStatus {
    failure: std::sync::Mutex<Option<String>>,
}

impl BotFlowSeedStatus {
    fn record_failure(&self, stage: &str, error: &str) {
        if let Ok(mut slot) = self.failure.lock() {
            *slot = Some(format!("{stage}: {error}"));
        }
    }

    fn failure(&self) -> Option<String> {
        self.failure.lock().ok().and_then(|slot| slot.clone())
    }
}

/// Plain manifests the first-party AI chain registers once the Bot Agent is enabled.
///
/// Production registration iterates this list and the catalog tests validate the
/// seeded `qq.business.full` reference graph against it, so a node type that the
/// reference graph uses cannot lose its registration without a test failing.
/// Manifests whose registration also carries runner-specific wiring (the merged
/// Agent + reply-delivery manifest) stay outside the list.
pub fn bot_agent_chain_manifests() -> Result<Vec<PluginManifest>, String> {
    [
        bot_conversation_context_manifest(),
        bot_reply_manifest(),
        bot_persona_manifest(),
        bot_interaction_manifest(),
    ]
    .into_iter()
    // Every one of these is a plain graph-invoked business node: none may reach
    // the platform, the delivery service or the Agent on its own.
    .map(|manifest| mutsuki_bot_sdk::manifest::checked(manifest, BotManifestSurface::Business))
    .collect()
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
        let (builder, repository) = bot_state_db(builder)?;
        let conversation_context: Arc<dyn ConversationContextStore> = repository.clone();
        let persona_store: Arc<dyn PersonaStore> = repository.clone();
        // The interaction waiter nodes are part of the first-party AI chain that this
        // factory owns, and they need the same open BotStateDb handle as the stores
        // above. Registering them anywhere else would either reopen the database or
        // leave `mutsuki.bot.interaction.*` out of the node catalog, which silently
        // fails validation of any graph that uses them -- including the seeded
        // `qq.business.full` reference graph.
        let interaction =
            InteractionService::new(repository.clone(), Arc::new(UnavailableInteractionMatcher));
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

        // The two halves carry different entitlements, so each is checked before
        // the merge rather than at the widest tier of the merged result. The Agent
        // bridge only hands an already-produced turn to the durable delivery
        // service (`mutsuki.bot.delivery/reply@1` on its node reply port); the
        // reply-delivery runner is the drain that performs the real platform send.
        let mut manifest = mutsuki_bot_sdk::manifest::merge(
            mutsuki_bot_sdk::manifest::checked(
                bot_agent_bridge_manifest(),
                BotManifestSurface::DurableReplyProducer,
            )?,
            mutsuki_bot_sdk::manifest::checked(
                bot_reply_delivery_manifest_for(
                    BOT_AGENT_BRIDGE_PLUGIN_ID,
                    BOT_AGENT_REPLY_DELIVERY_RUNNER_ID,
                ),
                BotManifestSurface::EffectDrain,
            )?,
        );
        manifest.requires.push(SurfaceRequirement::new(
            ContractSurfaceKind::Capability,
            connection_id.capability(),
        ));
        let mut builder = register_bot_agent_services(builder, manifest, config_handle.clone());
        for manifest in bot_agent_chain_manifests()? {
            builder = builder.register_builtin_plugin(manifest);
        }
        Ok(builder
            .register_event_source(Box::new(BotTaskRecoveryEventSource::reply_delivery(
                BotTaskRecoveryEventSource::default_interval(),
                BOT_AGENT_BRIDGE_PLUGIN_ID,
            )))
            .register_builtin_runner({
                let store = conversation_context.clone();
                move || Box::new(ConversationContextRunner::new(store.clone()))
            })
            .register_builtin_runner(|| Box::new(BotReplyRunner::default()))
            .register_builtin_runner({
                let store = persona_store.clone();
                move || Box::new(PersonaRunner::new(store.clone()))
            })
            .register_builtin_runner({
                let interaction = interaction.clone();
                move || Box::new(InteractionMatchRunner::new(interaction.clone()))
            })
            .register_builtin_runner({
                let interaction = interaction.clone();
                move || Box::new(InteractionCreateRunner::new(interaction.clone()))
            })
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
                interaction_runner(client, interaction.clone())
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
    mut manifest: PluginManifest,
    config: BotAgentConfigHandle,
) -> ServiceRuntimeBuilder {
    manifest
        .provides
        .services
        .push(BOT_AGENT_CONFIG_SERVICE_ID.into());
    manifest
        .provides
        .capabilities
        .push("bot.agent.config".into());
    let loaded_manifest = manifest.clone();
    let config = Arc::new(config);
    builder.register_builtin_loaded_plugin_factory(manifest, move || {
        Ok::<LoadedPlugin, String>(LoadedPlugin {
            manifest: loaded_manifest.clone(),
            runners: Vec::new(),
            async_handlers: Vec::new(),
            host_services: vec![RuntimeBootstrapperService::new(
                BOT_AGENT_CONFIG_SERVICE_ID,
                config.clone(),
                "bot.agent.config",
            )],
            resource_providers: Vec::new(),
            async_resource_providers: Vec::new(),
            host_effects: Vec::new(),
        })
    })
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

/// Application service holding the single Bot state database actor of this Host.
pub const BOT_STATE_SERVICE_ID: &str = "mutsuki.bot.state";
/// Application service holding the sandbox simulator shared by the sandbox plugin and the QQ
/// adapter's sandbox-aware runners.
const BOT_SANDBOX_STATE_SERVICE_ID: &str = "mutsuki.bot.sandbox.state";

/// Opens `data/bot/state.sqlite3` once per Host; every consumer shares the same actor.
pub fn bot_state_db(
    builder: ServiceRuntimeBuilder,
) -> Result<(ServiceRuntimeBuilder, Arc<BotStateDbRepository>), String> {
    let state_dir = builder.data_dir().join("bot");
    builder.ensure_application_service(BOT_STATE_SERVICE_ID, move || {
        std::fs::create_dir_all(&state_dir).map_err(|error| {
            format!(
                "failed to create Bot state directory {}: {error}",
                state_dir.display()
            )
        })?;
        BotStateDbRepository::open(state_dir.join("state.sqlite3"))
            .map(Arc::new)
            .map_err(|error| error.to_string())
    })
}

pub fn bot_sandbox(
    builder: ServiceRuntimeBuilder,
) -> Result<(ServiceRuntimeBuilder, Arc<SandboxService>), String> {
    let (builder, repository) = bot_state_db(builder)?;
    builder.ensure_application_service(BOT_SANDBOX_STATE_SERVICE_ID, move || {
        SandboxService::with_history("local", repository)
            .map(Arc::new)
            .map_err(|error| error.to_string())
    })
}

struct SandboxConfiguredPlugin;

impl ConfiguredPluginFactory for SandboxConfiguredPlugin {
    fn plugin_id(&self) -> &str {
        SANDBOX_SERVICE_ID
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
        mutsuki_bot_sdk::config::decode_empty(self.plugin_id(), &config)?;
        let (builder, sandbox) = bot_sandbox(builder)?;
        let mut manifest = PluginBuilder::new(SANDBOX_SERVICE_ID).build().manifest;
        manifest.provides.services.push(SANDBOX_SERVICE_ID.into());
        manifest.provides.capabilities.push("bot.sandbox".into());
        let loaded_manifest = manifest.clone();
        Ok(
            builder.register_builtin_loaded_plugin_factory(manifest, move || {
                Ok::<LoadedPlugin, String>(LoadedPlugin {
                    manifest: loaded_manifest.clone(),
                    runners: Vec::new(),
                    async_handlers: Vec::new(),
                    host_services: vec![RuntimeBootstrapperService::new(
                        SANDBOX_SERVICE_ID,
                        sandbox.clone(),
                        "bot.sandbox",
                    )],
                    resource_providers: Vec::new(),
                    async_resource_providers: Vec::new(),
                    host_effects: Vec::new(),
                })
            }),
        )
    }
}

pub struct WorkshopConfiguredPlugin {
    media_provider_id: String,
}

impl ConfiguredPluginFactory for WorkshopConfiguredPlugin {
    fn plugin_id(&self) -> &str {
        WORKSHOP_PLUGIN_ID
    }

    fn prepare(
        &self,
        config: &Value,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        // The owner document only carries the enable switch, which the catalog
        // already applied by calling this factory; the media provider binding
        // comes from the product assembly. The runner fetches through the std
        // HTTP effect, whose `response_provider_id` must be that media provider:
        // the cover body resource it stores is sent as the image segment.
        let _ = config;
        let mut manifest = mutsuki_plugin_bot_bilibili_workshop::manifest();
        manifest.requires.push(SurfaceRequirement::new(
            ContractSurfaceKind::ResourceProvider,
            self.media_provider_id.clone(),
        ));
        BotSubmissionGate::ensure_manifest_business_surface(&manifest)
            .map_err(|error| error.to_string())?;
        Ok(builder
            .register_builtin_plugin(manifest)
            .register_runtime_services_runner(mutsuki_plugin_bot_bilibili_workshop::runner))
    }
}

pub struct MihuashiConfiguredPlugin {
    media_provider_id: String,
}

impl ConfiguredPluginFactory for MihuashiConfiguredPlugin {
    fn plugin_id(&self) -> &str {
        MIHUASHI_PLUGIN_ID
    }

    fn prepare(
        &self,
        config: &Value,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        // The owner document only carries the enable switch, which the catalog
        // already applied by calling this factory; the media provider binding
        // comes from the product assembly.
        let _ = config;
        let mut manifest = mutsuki_plugin_bot_mihuashi::manifest();
        manifest.requires.push(SurfaceRequirement::new(
            ContractSurfaceKind::ResourceProvider,
            self.media_provider_id.clone(),
        ));
        manifest.requires.push(SurfaceRequirement::task_protocol(
            "mutsuki.browser.snapshot",
        ));
        BotSubmissionGate::ensure_manifest_business_surface(&manifest)
            .map_err(|error| error.to_string())?;
        let media_provider_id = self.media_provider_id.clone();
        Ok(builder
            .register_builtin_plugin(manifest)
            .register_runtime_services_runner(move |client, resources| {
                mutsuki_plugin_bot_mihuashi::runner(client, resources, media_provider_id.clone())
            }))
    }
}

/// Product-facing link-card plugin configuration shared by the Workshop and
/// Mihuashi factories. The owner document only exposes the enable switch; the
/// media resource provider binding is owned by the product assembly.
pub fn link_card_config_descriptor(plugin_id: &str, title: &str) -> ConfigDescriptor {
    ConfigDescriptor {
        provider_id: ConfigProviderId::new(plugin_id),
        schema_version: 1,
        value_version: 1,
        title: LocalizedText::new(title),
        description: None,
        scopes: vec![ConfigScope::global()],
        root: ConfigNode {
            key: ConfigKey::new("link_card"),
            value_type: ConfigValueType::Object,
            title: LocalizedText::new(title),
            description: None,
            default_value: None,
            constraints: ConfigConstraints::default(),
            presentation: ConfigPresentation::default(),
            visibility: None,
            enabled_if: None,
            mutability: ConfigMutability::ReadWrite,
            restart_policy: RestartPolicy::PluginReload,
            children: vec![bool_node("enabled", "启用", Some("关闭后不会加载该插件。"))],
        },
        groups: Vec::new(),
    }
}

/// Projects the link-card owner document shape.
pub fn link_card_config_value(enabled: bool) -> ConfigValue {
    ConfigValue::Object(
        [("enabled".into(), ConfigValue::Bool(enabled))]
            .into_iter()
            .collect(),
    )
}

fn bool_node(key: &str, title: &str, description: Option<&str>) -> ConfigNode {
    let mut node = field_node(
        key,
        title,
        ConfigValueType::Bool,
        ConfigConstraints::default(),
    );
    node.description = description.map(LocalizedText::new);
    node
}

fn field_node(
    key: &str,
    title: &str,
    value_type: ConfigValueType,
    constraints: ConfigConstraints,
) -> ConfigNode {
    ConfigNode {
        key: ConfigKey::new(key),
        value_type,
        title: LocalizedText::new(title),
        description: None,
        default_value: None,
        constraints,
        presentation: ConfigPresentation::default(),
        visibility: None,
        enabled_if: None,
        mutability: ConfigMutability::ReadWrite,
        restart_policy: RestartPolicy::PluginReload,
        children: Vec::new(),
    }
}

/// Catalog of production Bot plugins that can be selected by ServiceHost configuration.
/// Media upload is intentionally absent until a product registers an explicit provider-backed
/// QQ factory of its own.
///
/// Conversation-context, reply and persona runners are registered by
/// `BotAgentConfiguredPlugin` against the shared `BotStateDb` actor. Command and
/// interaction are independently selectable so a workspace graph can seed without
/// enabling Agent. They are not Memory-store factories.
pub fn configured_bot_plugin_catalog(
    media_provider_id: String,
) -> ServiceRuntimeResult<ConfiguredPluginCatalog> {
    configured_bot_plugin_catalog_inner(media_provider_id)
}

fn configured_bot_plugin_catalog_inner(
    media_provider_id: String,
) -> ServiceRuntimeResult<ConfiguredPluginCatalog> {
    let mut catalog = ConfiguredPluginCatalog::new();
    catalog.register(BotCommandConfiguredPlugin)?;
    catalog.register(BotInteractionConfiguredPlugin)?;
    catalog.register(SandboxConfiguredPlugin)?;
    catalog.register(WorkshopConfiguredPlugin {
        media_provider_id: media_provider_id.clone(),
    })?;
    catalog.register(MihuashiConfiguredPlugin { media_provider_id })?;
    Ok(catalog)
}

/// Adds the Flow Router only when product bootstrap supplies ConfigService.
pub fn configured_bot_plugin_catalog_with_config(
    config: Arc<ConfigService>,
    media_provider_id: String,
) -> ServiceRuntimeResult<ConfiguredPluginCatalog> {
    let flow_registry = Arc::new(BotFlowRegistry::new(BotNodeCatalog::default()));
    let mut catalog = configured_bot_plugin_catalog_inner(media_provider_id)?;
    catalog.register(BotFlowRouterConfiguredPlugin::with_registry(
        config,
        flow_registry,
    ))?;
    Ok(catalog)
}

/// Production Bot catalog with configurable Agent nodes wired to a shared Agent owner
/// registry. The base catalog intentionally remains Agent-free for products that do not opt in.
/// `seed_flow` is applied by LoadPlan activation into stores that never recorded a flow
/// document; existing records, including a graph the user cleared, are never overwritten.
pub fn configured_bot_plugin_catalog_with_agent_and_flow(
    config: Arc<ConfigService>,
    connections: AgentConnectionRegistry,
    flow_registry: Arc<BotFlowRegistry>,
    seed_flow: Option<BotFlowDocument>,
    media_provider_id: String,
) -> ServiceRuntimeResult<ConfiguredPluginCatalog> {
    let mut catalog = configured_bot_plugin_catalog_inner(media_provider_id)?;
    catalog.register(BotFlowRouterConfiguredPlugin {
        config,
        registry: Some(flow_registry),
        seed: seed_flow,
    })?;
    catalog.register(BotAgentConfiguredPlugin { connections })?;
    Ok(catalog)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::BTreeMap;

    use mutsuki_config_service::{
        ConfigCompareAndSetRequest, ConfigContext, ConfigDocumentKey, ConfigProviderRegistry,
        ConfigRepository, ConfigRevision, ConfigService, ConfigValue, InMemoryConfigRepository,
    };
    use mutsuki_plugin_bot_adapter_qqbot::qqbot_adapter_manifest;
    use mutsuki_plugin_bot_event_router::flow_router_manifest;
    use mutsuki_runtime_contracts::RuntimeLoadPlan;
    use mutsuki_service_config::{ConfiguredPluginSelection, ServiceConfig};
    use serde_json::json;

    use mutsuki_bot_flow::validate_flow;

    use super::*;

    /// Every manifest a fully-enabled first-party product registers.
    ///
    /// Shared with `orchestrated_flow`'s tests so there is one definition of "the
    /// first-party node catalog"; a second hand-written list is how the seeded graph
    /// came to reference node types production never registered.
    pub(crate) fn reference_catalog_manifests() -> Vec<PluginManifest> {
        vec![
            qqbot_adapter_manifest(1, false),
            flow_router_manifest(),
            bot_command_manifest(1),
            bot_agent_bridge_manifest(),
            mutsuki_plugin_bot_delivery::bot_reply_delivery_manifest(),
            mutsuki_plugin_bot_bilibili::manifest(),
            mutsuki_plugin_bot_mihuashi::manifest(),
        ]
        .into_iter()
        .chain(bot_agent_chain_manifests().expect("chain manifests satisfy the business surface"))
        .collect()
    }

    fn empty_load_plan(manifests: Vec<PluginManifest>) -> RuntimeLoadPlan {
        RuntimeLoadPlan {
            lock_version: 1,
            core_api_version: "1".into(),
            profile_id: "test".into(),
            profile_hash: "hash".into(),
            registry_generation: 1,
            plugins: manifests,
            load_order: Vec::new(),
            runner_bindings: BTreeMap::new(),
            plugin_deployments: BTreeMap::new(),
            observability: Default::default(),
            capability_graph: Default::default(),
            contract_surfaces: Vec::new(),
        }
    }

    #[test]
    fn agent_bridge_is_a_durable_reply_producer_and_nothing_wider() {
        let manifest = bot_agent_bridge_manifest();
        // This is the whole reason Rule 13's gate needs a tier rather than a
        // blanket allow: the Agent bridge legitimately declares
        // `mutsuki.bot.delivery/reply@1`, which the strict business surface denies.
        BotSubmissionGate::ensure_manifest_surface(&manifest, BotManifestSurface::Business)
            .expect_err("the Agent bridge declares a delivery surface a business plugin may not");
        BotSubmissionGate::ensure_manifest_surface(
            &manifest,
            BotManifestSurface::DurableReplyProducer,
        )
        .expect("handing an already-produced reply to the delivery service is the durable route");

        // The exemption stops there: the bridge must never gain a direct platform
        // send or the ability to originate an Agent turn outside a graph binding.
        for contract in manifest
            .provides
            .runners
            .iter()
            .flat_map(|runner| runner.contract_surfaces.iter())
        {
            if let Some(protocol_id) = contract.as_str().strip_prefix("requires:task_protocol:") {
                assert!(
                    !protocol_id.starts_with("mutsuki.bot.agent/")
                        && protocol_id != "mutsuki.bot.message/send@1"
                        && protocol_id != "mutsuki.bot.message/recall@1",
                    "Agent bridge must not declare {protocol_id}"
                );
            }
        }
    }

    #[test]
    fn production_catalog_declares_every_node_type_the_seed_graph_uses() {
        let catalog = BotNodeCatalog::from_manifests(&reference_catalog_manifests())
            .expect("reference catalogs merge");
        let seed = crate::orchestrated_flow::qq_full_business_flow();
        let missing: Vec<_> = seed
            .nodes
            .iter()
            .filter(|node| {
                catalog
                    .descriptor(&node.node_type_id, node.node_type_version)
                    .is_none()
            })
            .map(|node| format!("{}@{}", node.node_type_id, node.node_type_version))
            .collect();
        assert!(
            missing.is_empty(),
            "the seeded reference graph uses node types no production factory registers, \
             so BotFlowLoadPlanObserver would leave routing on an empty graph: {missing:?}"
        );
        let result = validate_flow(&seed, &catalog);
        assert!(result.valid, "{:#?}", result.issues);
    }

    #[tokio::test]
    async fn seed_failure_is_reported_instead_of_a_healthy_empty_graph() {
        // A catalog missing the node types the seed uses is exactly the P0 shape:
        // the seed cannot validate, routing stays on an empty graph, and the
        // process must not keep claiming it is healthy.
        let registry = Arc::new(BotFlowRegistry::new(BotNodeCatalog::default()));
        let config = Arc::new(
            ConfigService::new(
                Arc::new(ConfigProviderRegistry::default()),
                Arc::new(InMemoryConfigRepository::default()),
            )
            .expect("config service"),
        );
        config
            .registry()
            .register(Arc::new(BotFlowConfigProvider::new(registry.clone())))
            .expect("flow provider registers");
        let status = Arc::new(BotFlowSeedStatus::default());
        let observer = BotFlowLoadPlanObserver {
            registry: registry.clone(),
            config,
            seed: Some(crate::orchestrated_flow::qq_full_business_flow()),
            status: status.clone(),
        };

        observer.activate(&empty_load_plan(Vec::new()));

        let mut recorded = None;
        for _ in 0..100 {
            if let Some(failure) = status.failure() {
                recorded = Some(failure);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let recorded = recorded.expect("an unapplied seed must be recorded, not only logged");
        assert!(
            recorded.starts_with("seed: "),
            "failure must name the stage: {recorded}"
        );
        assert_eq!(registry.active().flow.nodes.len(), 0);
    }

    #[tokio::test]
    async fn flow_observer_seeds_reference_graph_when_store_has_no_record() {
        let manifests = reference_catalog_manifests();
        let registry = Arc::new(BotFlowRegistry::new(
            BotNodeCatalog::from_manifests(&manifests).expect("reference catalogs merge"),
        ));
        let config = Arc::new(
            ConfigService::new(
                Arc::new(ConfigProviderRegistry::default()),
                Arc::new(InMemoryConfigRepository::default()),
            )
            .expect("config service"),
        );
        config
            .registry()
            .register(Arc::new(BotFlowConfigProvider::new(registry.clone())))
            .expect("flow provider registers");
        let observer = BotFlowLoadPlanObserver {
            registry: registry.clone(),
            config: config.clone(),
            seed: Some(crate::orchestrated_flow::qq_full_business_flow()),
            status: Arc::new(BotFlowSeedStatus::default()),
        };

        observer.activate(&empty_load_plan(manifests));

        let key = ConfigDocumentKey::new(BOT_FLOW_CONFIG_PROVIDER_ID, ConfigContext::global());
        let mut seeded = false;
        for _ in 0..100 {
            let recorded = config
                .repository()
                .read(&key)
                .expect("repository readable")
                .is_some_and(|snapshot| {
                    snapshot.value.to_json()["flow"]["flow_id"] == "qq.business.full"
                });
            if recorded {
                seeded = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(seeded, "observer must seed the absent flow record");
        assert_eq!(registry.active().flow.flow_id, "qq.business.full");
    }

    fn seed_stored_flow(repo: &InMemoryConfigRepository, value: ConfigValue) {
        let mut write = repo
            .prepare_compare_and_set(ConfigCompareAndSetRequest {
                key: ConfigDocumentKey::new(BOT_FLOW_CONFIG_PROVIDER_ID, ConfigContext::global()),
                expected_revision: ConfigRevision::ABSENT,
                value,
                schema_version: 1,
                value_version: 2,
            })
            .unwrap();
        write.commit().unwrap();
        write.finish().unwrap();
    }

    #[test]
    fn graph_only_plugins_accept_omitted_empty_config() {
        let root = tempfile::tempdir().unwrap();
        let mut service = ServiceConfig::default();
        service.service.data_dir = root.path().join("data");
        let config = Arc::new(
            ConfigService::new(
                Arc::new(mutsuki_config_service::ConfigProviderRegistry::default()),
                Arc::new(mutsuki_config_service::InMemoryConfigRepository::default()),
            )
            .unwrap(),
        );

        BotFlowRouterConfiguredPlugin::new(config)
            .prepare(&Value::Null, ServiceRuntimeBuilder::new(service.clone()))
            .expect("Flow Router should accept a graph-only selection without a config table");
        BotCommandConfiguredPlugin
            .prepare(&Value::Null, ServiceRuntimeBuilder::new(service.clone()))
            .expect(
                "Command node plugin should accept a graph-only selection without a config table",
            );
        BotInteractionConfiguredPlugin
            .prepare(&Value::Null, ServiceRuntimeBuilder::new(service))
            .expect(
                "Interaction plugin should accept a graph-only selection without a config table",
            );
    }

    #[test]
    fn sandbox_agent_and_qq_share_one_state_db_actor() {
        let root = tempfile::tempdir().unwrap();
        let mut service = ServiceConfig::default();
        service.service.data_dir = root.path().to_path_buf();
        let builder = ServiceRuntimeBuilder::new(service);
        let (builder, first) = bot_state_db(builder).unwrap();
        let (builder, sandbox) = bot_sandbox(builder).unwrap();
        let (builder, second) = bot_state_db(builder).unwrap();
        let (_, sandbox_again) = bot_sandbox(builder).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert!(Arc::ptr_eq(&sandbox, &sandbox_again));
        assert_eq!(first.metrics().connection_open_count, 1);
        assert_eq!(first.journal_mode(), "wal");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stored_incompatible_flow_does_not_block_start() {
        let repo = InMemoryConfigRepository::default();
        seed_stored_flow(
            &repo,
            ConfigValue::from_json(&json!({
                "flow": {
                    "nodes": [{
                        "node_id": "missing",
                        "node_type_id": "does.not.exist",
                        "node_type_version": 1
                    }]
                }
            })),
        );
        let config = Arc::new(
            ConfigService::new(
                Arc::new(mutsuki_config_service::ConfigProviderRegistry::default()),
                Arc::new(repo),
            )
            .unwrap(),
        );
        let root = tempfile::tempdir().unwrap();
        let mut service = ServiceConfig::default();
        service.ipc.enabled = false;
        service.observe.console = false;
        service.plugins.dynamic_dirs.clear();
        service.service.home_dir = root.path().into();
        service.service.data_dir = root.path().join("data");
        service.service.log_dir = root.path().join("logs");
        service.service.run_dir = root.path().join("run");
        std::fs::create_dir_all(&service.service.data_dir).unwrap();
        std::fs::create_dir_all(&service.service.log_dir).unwrap();
        std::fs::create_dir_all(&service.service.run_dir).unwrap();
        service.plugins.configured = vec![ConfiguredPluginSelection {
            id: BOT_FLOW_ROUTER_PLUGIN_ID.into(),
            enabled: true,
            config: Value::Null,
        }];
        let mut catalog = ConfiguredPluginCatalog::new();
        catalog
            .register(BotFlowRouterConfiguredPlugin::new(config))
            .unwrap();
        let runtime = ServiceRuntimeBuilder::new(service)
            .with_configured_plugin_catalog(catalog)
            .start()
            .await
            .expect("stored Flow must not block start");
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        let snapshot = runtime
            .host_service::<BotFlowRegistry>(BOT_FLOW_REGISTRY_SERVICE_ID)
            .unwrap()
            .active();
        assert_eq!(snapshot.revision, 0);
        assert!(snapshot.flow.nodes.is_empty());
        runtime.shutdown().await;
    }

    #[test]
    fn legacy_orchestration_configuration_is_rejected_at_the_owner_boundary() {
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
