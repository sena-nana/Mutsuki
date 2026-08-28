use std::sync::Arc;
use std::time::Duration;

use mutsuki_agent_contracts::MediaService;
use mutsuki_bot_conversation::{ConversationRepository, ConversationService};
use mutsuki_bot_delivery::{
    ActiveDeliveryService, DeliveryGateway, DeliveryPolicyResolver, ReplyDeliveryRepository,
    ScheduledAgentDeliveryBridge, ScheduledDeliveryPolicyProvider, ScheduledDeliveryTargetResolver,
};
use mutsuki_bot_interaction::{
    InteractionConditionMatcher, InteractionRepository, InteractionService,
};
use mutsuki_bot_protocol::{ConversationPolicy, DeliveryPolicy, QqStreamingStrategy};
use mutsuki_plugin_bot_agent::{
    AgentBridgeClient, BOT_AGENT_BRIDGE_RUNNER_ID, BOT_AGENT_CONFIG_SERVICE_ID, BotAgentBridge,
    BotAgentConfig, BotAgentConfigError, BotAgentConfigHandle,
    agent_bridge_runner_with_delivery_policy, bot_agent_bridge_manifest,
};
use mutsuki_plugin_bot_command::{BotCommandNodeRunner, bot_command_manifest};
use mutsuki_plugin_bot_conversation_context::{
    ConversationContextRunner, ConversationContextStore, bot_conversation_context_manifest,
};
use mutsuki_plugin_bot_delivery::{
    bot_delivery_manifest, bot_reply_delivery_manifest, bot_scheduled_delivery_manifest,
    delivery_runner, reply_delivery_runner, scheduled_delivery_runner,
};
use mutsuki_plugin_bot_interaction::{
    InteractionCreateRunner, InteractionMatchRunner, bot_interaction_manifest, interaction_runner,
};
use mutsuki_plugin_bot_media::{bot_media_bridge_manifest, media_bridge_runner};
use mutsuki_plugin_bot_persona::{PersonaRunner, PersonaStore, bot_persona_manifest};
use mutsuki_plugin_bot_reply::{BotReplyRunner, bot_reply_manifest};
use mutsuki_runtime_sdk::{LoadedPlugin, RuntimeBootstrapperService};
use mutsuki_service_runtime::ServiceRuntimeBuilder;

use crate::BotReplyDeliveryRecoveryEventSource;

/// Explicit product assembly for the QQ AI pipeline.
///
/// Every stateful or external capability is injected. Constructing this bundle cannot silently
/// fall back to process-local state, a fake Agent client, or an embedded media codec.
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
    scheduled_delivery: Option<(
        Arc<dyn ScheduledDeliveryTargetResolver>,
        Arc<dyn ScheduledDeliveryPolicyProvider>,
    )>,
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
            scheduled_delivery: None,
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

    pub fn with_streaming(self, streaming: QqStreamingStrategy) -> Self {
        let mut config = self.agent_config.snapshot();
        config.streaming = match streaming {
            QqStreamingStrategy::FinalOnly => "final_only",
            QqStreamingStrategy::SegmentMessages => "segment_messages",
        }
        .into();
        self.agent_config
            .replace(config)
            .expect("streaming strategy must produce a valid Bot Agent config");
        self
    }

    pub fn with_agent_config(
        mut self,
        config: BotAgentConfig,
    ) -> Result<Self, BotAgentConfigError> {
        self.agent_config = BotAgentConfigHandle::new(config)?;
        Ok(self)
    }

    #[must_use]
    pub fn with_agent_config_handle(mut self, handle: BotAgentConfigHandle) -> Self {
        self.agent_config = handle;
        self
    }

    pub fn with_scheduled_delivery(
        mut self,
        targets: Arc<dyn ScheduledDeliveryTargetResolver>,
        policies: Arc<dyn ScheduledDeliveryPolicyProvider>,
    ) -> Self {
        self.scheduled_delivery = Some((targets, policies));
        self
    }

    #[must_use]
    pub fn with_reply_delivery_policy(mut self, policy: DeliveryPolicy) -> Self {
        self.reply_delivery_policy = policy;
        self
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
        let scheduled_delivery = self.scheduled_delivery.map(|(targets, policies)| {
            ScheduledAgentDeliveryBridge::new(delivery.clone(), targets, policies)
        });
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
        let builder = builder
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
            });
        if let Some(scheduled_delivery) = scheduled_delivery {
            builder
                .register_builtin_plugin(bot_scheduled_delivery_manifest())
                .register_runtime_client_runner(move |client| {
                    scheduled_delivery_runner(client, scheduled_delivery.clone())
                })
        } else {
            builder
        }
    }
}
