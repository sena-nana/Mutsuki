use std::sync::Arc;
use std::time::Duration;

use mutsuki_agent_contracts::MediaService;
use mutsuki_bot_conversation::{
    ConversationContextStore, ConversationRepository, ConversationService, PersonaStore,
};
use mutsuki_bot_delivery::{
    ActiveDeliveryService, DeliveryGateway, DeliveryPolicyResolver, ReplyDeliveryRepository,
};
use mutsuki_bot_interaction::{
    InteractionConditionMatcher, InteractionRepository, InteractionService,
};
use mutsuki_bot_protocol::{ConversationPolicy, DeliveryPolicy};
use mutsuki_plugin_bot_agent::{
    AgentBridgeClient, BOT_AGENT_BRIDGE_RUNNER_ID, BOT_AGENT_CONFIG_SERVICE_ID, BotAgentBridge,
    BotAgentConfig, BotAgentConfigHandle, agent_bridge_runner_with_delivery_policy,
    bot_agent_bridge_manifest,
};
use mutsuki_plugin_bot_command::{BotCommandNodeRunner, bot_command_manifest};
use mutsuki_plugin_bot_conversation_context::{
    ConversationContextRunner, bot_conversation_context_manifest,
};
use mutsuki_plugin_bot_delivery::{
    bot_delivery_manifest, bot_reply_delivery_manifest, delivery_runner, reply_delivery_runner,
};
use mutsuki_plugin_bot_interaction::{
    InteractionCreateRunner, InteractionMatchRunner, bot_interaction_manifest, interaction_runner,
};
use mutsuki_plugin_bot_media::{bot_media_bridge_manifest, media_bridge_runner};
use mutsuki_plugin_bot_persona::{PersonaRunner, bot_persona_manifest};
use mutsuki_plugin_bot_reply::{BotReplyRunner, bot_reply_manifest};
use mutsuki_runtime_sdk::{LoadedPlugin, RuntimeBootstrapperService};
use mutsuki_service_runtime::ServiceRuntimeBuilder;

use crate::BotReplyDeliveryRecoveryEventSource;

/// Fully-injected assembly of the QQ AI pipeline, for deterministic end-to-end tests.
///
/// This is **not** the production path. Products assemble through
/// `configured_bot_plugin_catalog_with_agent_and_flow`, whose factories build their
/// own SQLite handle, Agent client and delivery gateway from saved configuration.
/// That is exactly why this seam exists: an E2E needs to substitute the Agent
/// backend, the delivery gateway and the media service, and the configured
/// factories deliberately offer no injection point for them.
///
/// Keep the two in step through `bot_agent_chain_manifests`, which both this
/// bundle and `BotAgentConfiguredPlugin` register, rather than by listing
/// manifests here a second time.
pub struct QqAiBotPluginBundle {
    conversations: Arc<dyn ConversationRepository>,
    deliveries: Arc<dyn ReplyDeliveryRepository>,
    interactions: Arc<dyn InteractionRepository>,
    default_policy: ConversationPolicy,
    agent: Box<dyn AgentBridgeClient>,
    media: Arc<dyn MediaService>,
    delivery_gateway: Arc<dyn DeliveryGateway>,
    delivery_policy: Arc<dyn DeliveryPolicyResolver>,
    interaction_matcher: Arc<dyn InteractionConditionMatcher>,
    agent_config: BotAgentConfigHandle,
    reply_delivery_policy: DeliveryPolicy,
    reply_delivery_recovery_interval: Duration,
    conversation_context: Arc<dyn ConversationContextStore>,
    persona_store: Arc<dyn PersonaStore>,
}

impl QqAiBotPluginBundle {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        conversations: Arc<dyn ConversationRepository>,
        deliveries: Arc<dyn ReplyDeliveryRepository>,
        interactions: Arc<dyn InteractionRepository>,
        default_policy: ConversationPolicy,
        agent: Box<dyn AgentBridgeClient>,
        media: Arc<dyn MediaService>,
        delivery_gateway: Arc<dyn DeliveryGateway>,
        delivery_policy: Arc<dyn DeliveryPolicyResolver>,
        interaction_matcher: Arc<dyn InteractionConditionMatcher>,
        conversation_context: Arc<dyn ConversationContextStore>,
        persona_store: Arc<dyn PersonaStore>,
    ) -> Self {
        let mut agent_config = BotAgentConfig::default();
        agent_config.enabled = true;
        agent_config.connection_id = "injected".into();
        Self {
            conversations,
            deliveries,
            interactions,
            default_policy,
            agent,
            media,
            delivery_gateway,
            delivery_policy,
            interaction_matcher,
            agent_config: BotAgentConfigHandle::new(agent_config)
                .expect("explicitly injected Agent config is valid"),
            reply_delivery_policy: DeliveryPolicy {
                max_attempts: 3,
                initial_backoff_ms: 1_000,
                max_backoff_ms: 60_000,
                not_before_unix_ms: None,
                expires_at_unix_ms: None,
            },
            reply_delivery_recovery_interval: Duration::from_millis(250),
            conversation_context,
            persona_store,
        }
    }

    #[must_use]
    pub fn with_reply_delivery_recovery_interval(mut self, interval: Duration) -> Self {
        self.reply_delivery_recovery_interval = interval;
        self
    }

    pub fn install(self, builder: ServiceRuntimeBuilder) -> ServiceRuntimeBuilder {
        let agent_config = self.agent_config;
        let reply_delivery_policy = self.reply_delivery_policy;
        let reply_delivery_recovery_interval = self.reply_delivery_recovery_interval;
        let reply_repository = self.deliveries.clone();
        let conversations = ConversationService::new(self.conversations, self.default_policy);
        let agent =
            BotAgentBridge::new_with_config(conversations, self.agent, agent_config.clone());
        let delivery = ActiveDeliveryService::new(
            self.deliveries,
            self.delivery_gateway,
            self.delivery_policy,
        );
        let interaction = InteractionService::new(self.interactions, self.interaction_matcher);
        let media = self.media;
        let conversation_context = self.conversation_context;
        let persona_store = self.persona_store;
        let mut agent_manifest = bot_agent_bridge_manifest();
        agent_manifest
            .provides
            .services
            .push(BOT_AGENT_CONFIG_SERVICE_ID.into());
        agent_manifest
            .provides
            .capabilities
            .push("bot.agent.config".into());
        let loaded_agent_manifest = agent_manifest.clone();
        let config_service = Arc::new(agent_config.clone());
        // Same list the production factory registers, so the E2E cannot pass against
        // a node catalog that production would never build.
        let mut builder = builder;
        for manifest in
            crate::configured::bot_agent_chain_manifests().expect("chain manifests are in-surface")
        {
            builder = builder.register_builtin_plugin(manifest);
        }
        builder
            .register_dynamic_runner_limit(BOT_AGENT_BRIDGE_RUNNER_ID, {
                let config = agent_config.clone();
                move || {
                    let settings = config.snapshot();
                    (Some(settings.max_concurrency), Some(settings.timeout_ms))
                }
            })
            .register_builtin_plugin(bot_command_manifest(1))
            .register_builtin_loaded_plugin_factory(agent_manifest, move || {
                Ok::<LoadedPlugin, String>(LoadedPlugin {
                    manifest: loaded_agent_manifest.clone(),
                    runners: Vec::new(),
                    async_handlers: Vec::new(),
                    host_services: vec![RuntimeBootstrapperService::new(
                        BOT_AGENT_CONFIG_SERVICE_ID,
                        config_service.clone(),
                        "bot.agent.config",
                    )],
                    resource_providers: Vec::new(),
                    async_resource_providers: Vec::new(),
                    host_effects: Vec::new(),
                })
            })
            .register_builtin_plugin(bot_media_bridge_manifest())
            .register_builtin_plugin(bot_delivery_manifest())
            .register_builtin_plugin(bot_reply_delivery_manifest())
            .register_event_source(Box::new(BotReplyDeliveryRecoveryEventSource::new(
                reply_delivery_recovery_interval,
            )))
            .register_builtin_plugin(bot_interaction_manifest())
            .register_builtin_plugin(bot_conversation_context_manifest())
            .register_builtin_plugin(bot_reply_manifest())
            .register_builtin_plugin(bot_persona_manifest())
            .register_builtin_runner(move || Box::new(BotCommandNodeRunner::new(1)))
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
            .register_runtime_client_runner(move |client| {
                agent_bridge_runner_with_delivery_policy(
                    client,
                    agent.clone(),
                    reply_delivery_policy.clone(),
                )
            })
            .register_runtime_client_runner(move |client| {
                media_bridge_runner(client, media.clone())
            })
            .register_runtime_client_runner(move |client| delivery_runner(client, delivery.clone()))
            .register_runtime_client_runner(move |client| {
                reply_delivery_runner(client, reply_repository.clone())
            })
            .register_runtime_client_runner(move |client| {
                interaction_runner(client, interaction.clone())
            })
    }
}
