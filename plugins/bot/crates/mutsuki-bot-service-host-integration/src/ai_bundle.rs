use std::sync::Arc;

use mutsuki_agent_contracts::MediaService;
use mutsuki_bot_conversation::{ConversationRepository, ConversationService};
use mutsuki_bot_delivery::{
    ActiveDeliveryService, DeliveryPolicyResolver, DeliveryRepository, QqDeliveryGateway,
    ScheduledAgentDeliveryBridge, ScheduledDeliveryPolicyProvider, ScheduledDeliveryTargetResolver,
    bot_delivery_manifest, bot_scheduled_delivery_manifest, delivery_runner,
    scheduled_delivery_runner,
};
use mutsuki_bot_interaction::{
    BOT_INTERACTION_RUNNER_ID, InteractionConditionMatcher, InteractionRepository,
    InteractionService, bot_interaction_manifest, interaction_runner,
};
use mutsuki_bot_protocol::{
    BOT_AGENT_BRIDGE_PROTOCOL_ID, BOT_COMMAND_PARSE_PROTOCOL_ID,
    BOT_INTERACTION_SESSION_PROTOCOL_ID, BotHandlerDescriptor, BotPropagationPolicy,
    ConversationPolicy, QqStreamingStrategy,
};
use mutsuki_plugin_bot_agent::{
    AgentBridgeClient, BOT_AGENT_BRIDGE_RUNNER_ID, BotAgentBridge, agent_bridge_runner,
    bot_agent_bridge_manifest, bot_agent_command_descriptors,
};
use mutsuki_plugin_bot_command::{BOT_COMMAND_RUNNER_ID, BotCommandRunner, bot_command_manifest};
use mutsuki_plugin_bot_event_router::{
    BotPermissionAuthorizer, BotRateLimitService, bot_handler_guard_manifest,
    handler_pipeline_manifest, handler_pipeline_runner, permission_runner, rate_limit_runner,
};
use mutsuki_plugin_bot_media::{bot_media_bridge_manifest, media_bridge_runner};
use mutsuki_service_runtime::ServiceRuntimeBuilder;

/// Explicit product assembly for the QQ AI pipeline.
///
/// Every stateful or external capability is injected. Constructing this bundle cannot silently
/// fall back to process-local state, a fake Agent client, or an embedded media codec.
pub struct QqAiBotPluginBundle {
    conversations: Arc<dyn ConversationRepository>,
    deliveries: Arc<dyn DeliveryRepository>,
    interactions: Arc<dyn InteractionRepository>,
    default_policy: ConversationPolicy,
    agent: Box<dyn AgentBridgeClient>,
    media: Arc<dyn MediaService>,
    delivery_gateway: Arc<dyn QqDeliveryGateway>,
    delivery_policy: Arc<dyn DeliveryPolicyResolver>,
    interaction_matcher: Arc<dyn InteractionConditionMatcher>,
    permission_authorizer: Arc<dyn BotPermissionAuthorizer>,
    handlers: Vec<BotHandlerDescriptor>,
    streaming: QqStreamingStrategy,
    command_prefixes: Vec<String>,
    scheduled_delivery: Option<(
        Arc<dyn ScheduledDeliveryTargetResolver>,
        Arc<dyn ScheduledDeliveryPolicyProvider>,
    )>,
}

impl QqAiBotPluginBundle {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        conversations: Arc<dyn ConversationRepository>,
        deliveries: Arc<dyn DeliveryRepository>,
        interactions: Arc<dyn InteractionRepository>,
        default_policy: ConversationPolicy,
        agent: Box<dyn AgentBridgeClient>,
        media: Arc<dyn MediaService>,
        delivery_gateway: Arc<dyn QqDeliveryGateway>,
        delivery_policy: Arc<dyn DeliveryPolicyResolver>,
        interaction_matcher: Arc<dyn InteractionConditionMatcher>,
        permission_authorizer: Arc<dyn BotPermissionAuthorizer>,
    ) -> Self {
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
            permission_authorizer,
            handlers: Vec::new(),
            streaming: QqStreamingStrategy::FinalOnly,
            command_prefixes: vec!["/".into()],
            scheduled_delivery: None,
        }
    }

    pub fn with_handlers(mut self, handlers: Vec<BotHandlerDescriptor>) -> Self {
        self.handlers = handlers;
        self
    }

    pub fn with_streaming(mut self, streaming: QqStreamingStrategy) -> Self {
        self.streaming = streaming;
        self
    }

    pub fn with_command_prefixes(mut self, prefixes: Vec<String>) -> Self {
        self.command_prefixes = prefixes;
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

    pub fn install(self, builder: ServiceRuntimeBuilder) -> ServiceRuntimeBuilder {
        let conversations = ConversationService::new(self.conversations, self.default_policy);
        let agent = BotAgentBridge::new(conversations, self.agent, self.streaming);
        let delivery = ActiveDeliveryService::new(
            self.deliveries,
            self.delivery_gateway,
            self.delivery_policy,
        );
        let scheduled_delivery = self.scheduled_delivery.map(|(targets, policies)| {
            ScheduledAgentDeliveryBridge::new(delivery.clone(), targets, policies)
        });
        let interaction = InteractionService::new(self.interactions, self.interaction_matcher);
        let rate_limits = BotRateLimitService::system();
        let media = self.media;
        let permission_authorizer = self.permission_authorizer;
        let mut handlers = self.handlers;
        let agent_commands = bot_agent_command_descriptors();
        let command_prefixes = self.command_prefixes;
        handlers.push(BotHandlerDescriptor {
            handler_id: "mutsuki.bot.interaction.waiter".into(),
            binding_id: format!("binding:{BOT_INTERACTION_SESSION_PROTOCOL_ID}"),
            generation: 1,
            handler_protocol_id: BOT_INTERACTION_SESSION_PROTOCOL_ID.into(),
            runner_hint: Some(BOT_INTERACTION_RUNNER_ID.into()),
            event_kinds: Vec::new(),
            conversation_kinds: Vec::new(),
            filter: None,
            permissions: Vec::new(),
            priority: i32::MAX,
            propagation: BotPropagationPolicy::Continue,
            rate_limit: None,
            timeout_ms: None,
            side_effects: vec!["database".into()],
            max_concurrency: None,
            before_hook_protocol_ids: Vec::new(),
            after_hook_protocol_ids: Vec::new(),
            error_hook_protocol_ids: Vec::new(),
        });
        handlers.push(BotHandlerDescriptor {
            handler_id: "mutsuki.bot.agent.command-parser".into(),
            binding_id: format!("binding:{BOT_COMMAND_PARSE_PROTOCOL_ID}"),
            generation: 1,
            handler_protocol_id: BOT_COMMAND_PARSE_PROTOCOL_ID.into(),
            runner_hint: Some(BOT_COMMAND_RUNNER_ID.into()),
            event_kinds: vec![mutsuki_bot_protocol::BotEventKind::MessageCreated],
            conversation_kinds: Vec::new(),
            filter: None,
            permissions: Vec::new(),
            priority: 100,
            propagation: BotPropagationPolicy::Continue,
            rate_limit: None,
            timeout_ms: None,
            side_effects: Vec::new(),
            max_concurrency: None,
            before_hook_protocol_ids: Vec::new(),
            after_hook_protocol_ids: Vec::new(),
            error_hook_protocol_ids: Vec::new(),
        });
        handlers.push(BotHandlerDescriptor {
            handler_id: "mutsuki.bot.agent.message".into(),
            binding_id: format!("binding:{BOT_AGENT_BRIDGE_PROTOCOL_ID}"),
            generation: 1,
            handler_protocol_id: BOT_AGENT_BRIDGE_PROTOCOL_ID.into(),
            runner_hint: Some(BOT_AGENT_BRIDGE_RUNNER_ID.into()),
            event_kinds: vec![mutsuki_bot_protocol::BotEventKind::MessageCreated],
            conversation_kinds: Vec::new(),
            filter: None,
            permissions: Vec::new(),
            priority: 0,
            propagation: BotPropagationPolicy::StopOnSuccess,
            rate_limit: None,
            timeout_ms: None,
            side_effects: vec!["agent".into(), "qq_delivery".into()],
            max_concurrency: Some(1),
            before_hook_protocol_ids: Vec::new(),
            after_hook_protocol_ids: Vec::new(),
            error_hook_protocol_ids: Vec::new(),
        });

        let mut builder = builder;
        for handler in &handlers {
            if handler.timeout_ms.is_none() && handler.max_concurrency.is_none() {
                continue;
            }
            let runner_id = handler
                .runner_hint
                .clone()
                .unwrap_or_else(|| handler.handler_id.clone());
            builder = builder.configure_runner_limits(
                runner_id,
                handler.max_concurrency.map(|limit| limit as usize),
                handler.timeout_ms,
            );
        }

        let builder = builder
            .register_builtin_plugin(handler_pipeline_manifest())
            .register_builtin_plugin(bot_handler_guard_manifest())
            .register_builtin_plugin(bot_command_manifest(1))
            .register_builtin_plugin(bot_agent_bridge_manifest())
            .register_builtin_plugin(bot_media_bridge_manifest())
            .register_builtin_plugin(bot_delivery_manifest())
            .register_builtin_plugin(bot_interaction_manifest())
            .register_runtime_client_runner(move |client| {
                handler_pipeline_runner(client, handlers.clone())
            })
            .register_builtin_runner(move || {
                Box::new(BotCommandRunner::with_commands(
                    1,
                    command_prefixes.clone(),
                    agent_commands.clone(),
                ))
            })
            .register_runtime_client_runner(move |client| {
                agent_bridge_runner(client, agent.clone())
            })
            .register_runtime_client_runner(move |client| {
                media_bridge_runner(client, media.clone())
            })
            .register_builtin_runner(move || delivery_runner(delivery.clone()))
            .register_builtin_runner(move || interaction_runner(interaction.clone()))
            .register_builtin_runner(move || permission_runner(permission_authorizer.clone()))
            .register_builtin_runner(move || rate_limit_runner(rate_limits.clone()));
        if let Some(scheduled_delivery) = scheduled_delivery {
            builder
                .register_builtin_plugin(bot_scheduled_delivery_manifest())
                .register_builtin_runner(move || {
                    scheduled_delivery_runner(scheduled_delivery.clone())
                })
        } else {
            builder
        }
    }
}
