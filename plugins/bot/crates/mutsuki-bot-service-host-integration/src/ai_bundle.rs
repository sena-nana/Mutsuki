use std::sync::Arc;
use std::time::Duration;

use mutsuki_agent_contracts::MediaService;
use mutsuki_bot_conversation::{ConversationRepository, ConversationService};
use mutsuki_bot_delivery::{
    ActiveDeliveryService, DeliveryPolicyResolver, QqDeliveryGateway, ReplyDeliveryRepository,
    ScheduledAgentDeliveryBridge, ScheduledDeliveryPolicyProvider, ScheduledDeliveryTargetResolver,
    bot_delivery_manifest, bot_reply_delivery_manifest, bot_scheduled_delivery_manifest,
    delivery_runner, reply_delivery_runner, scheduled_delivery_runner,
};
use mutsuki_bot_interaction::{
    BOT_INTERACTION_RUNNER_ID, InteractionConditionMatcher, InteractionRepository,
    InteractionService, bot_interaction_manifest, interaction_runner,
};
use mutsuki_bot_protocol::{
    BOT_AGENT_BRIDGE_PROTOCOL_ID, BOT_COMMAND_PARSE_PROTOCOL_ID,
    BOT_INTERACTION_SESSION_PROTOCOL_ID, BotHandlerDescriptor, BotPropagationPolicy,
    ConversationPolicy, DeliveryPolicy, QqStreamingStrategy,
};
use mutsuki_plugin_bot_agent::{
    AgentBridgeClient, BOT_AGENT_BRIDGE_RUNNER_ID, BOT_AGENT_CONFIG_SERVICE_ID, BotAgentBridge,
    BotAgentConfig, BotAgentConfigError, BotAgentConfigHandle,
    agent_bridge_runner_with_delivery_policy, bot_agent_bridge_manifest,
    bot_agent_command_descriptors,
};
use mutsuki_plugin_bot_command::{BOT_COMMAND_RUNNER_ID, BotCommandRunner, bot_command_manifest};
use mutsuki_plugin_bot_event_router::{
    BotPermissionAuthorizer, BotRateLimitService, bot_handler_guard_manifest,
    handler_pipeline_manifest, handler_pipeline_runner, permission_runner, rate_limit_runner,
};
use mutsuki_plugin_bot_media::{bot_media_bridge_manifest, media_bridge_runner};
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
    delivery_gateway: Arc<dyn QqDeliveryGateway>,
    delivery_policy: Arc<dyn DeliveryPolicyResolver>,
    interaction_matcher: Arc<dyn InteractionConditionMatcher>,
    permission_authorizer: Arc<dyn BotPermissionAuthorizer>,
    handlers: Vec<BotHandlerDescriptor>,
    agent_config: BotAgentConfigHandle,
    command_prefixes: Vec<String>,
    scheduled_delivery: Option<(
        Arc<dyn ScheduledDeliveryTargetResolver>,
        Arc<dyn ScheduledDeliveryPolicyProvider>,
    )>,
    reply_delivery_policy: DeliveryPolicy,
    reply_delivery_recovery_interval: Duration,
    qq_management: Option<Arc<mutsuki_plugin_bot_qq_web::LocalQqManagementProvider>>,
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
        delivery_gateway: Arc<dyn QqDeliveryGateway>,
        delivery_policy: Arc<dyn DeliveryPolicyResolver>,
        interaction_matcher: Arc<dyn InteractionConditionMatcher>,
        permission_authorizer: Arc<dyn BotPermissionAuthorizer>,
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
            permission_authorizer,
            handlers: Vec::new(),
            agent_config: BotAgentConfigHandle::new(agent_config)
                .expect("explicitly injected Agent config is valid"),
            command_prefixes: vec!["/".into()],
            scheduled_delivery: None,
            reply_delivery_policy: DeliveryPolicy {
                max_attempts: 3,
                initial_backoff_ms: 1_000,
                max_backoff_ms: 60_000,
                not_before_unix_ms: None,
                expires_at_unix_ms: None,
            },
            reply_delivery_recovery_interval: Duration::from_millis(250),
            qq_management: None,
        }
    }

    pub fn with_handlers(mut self, handlers: Vec<BotHandlerDescriptor>) -> Self {
        self.handlers = handlers;
        self
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

    /// Seeds handler/command projections into the QQ console management owner when present.
    #[must_use]
    pub fn with_qq_management(
        mut self,
        local: Arc<mutsuki_plugin_bot_qq_web::LocalQqManagementProvider>,
    ) -> Self {
        self.qq_management = Some(local);
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
            max_concurrency: None,
            before_hook_protocol_ids: Vec::new(),
            after_hook_protocol_ids: Vec::new(),
            error_hook_protocol_ids: Vec::new(),
        });

        if let Some(local) = &self.qq_management {
            local.replace_handlers(
                handlers
                    .iter()
                    .cloned()
                    .map(|descriptor| mutsuki_plugin_bot_qq_web::handler_view(descriptor, true))
                    .collect(),
            );
            local.replace_commands(agent_commands.clone());
        }

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

        let agent_manifest = bot_agent_bridge_manifest();
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
            .register_builtin_plugin(handler_pipeline_manifest())
            .register_builtin_plugin(bot_handler_guard_manifest())
            .register_builtin_plugin(bot_command_manifest(1))
            .register_builtin_loaded_plugin_factory(agent_manifest, move || {
                Ok::<LoadedPlugin, String>(LoadedPlugin {
                    manifest: loaded_agent_manifest.clone(),
                    runners: Vec::new(),
                    async_handlers: Vec::new(),
                    host_services: vec![RuntimeBootstrapperService {
                        service_id: BOT_AGENT_CONFIG_SERVICE_ID.into(),
                        capability: Some("bot.agent.config".into()),
                        service: config_service.clone(),
                    }],
                    resource_providers: Vec::new(),
                    async_resource_providers: Vec::new(),
                })
            })
            .register_builtin_plugin(bot_media_bridge_manifest())
            .register_builtin_plugin(bot_delivery_manifest())
            .register_builtin_plugin(bot_reply_delivery_manifest())
            .register_event_source(Box::new(BotReplyDeliveryRecoveryEventSource::new(
                reply_delivery_recovery_interval,
            )))
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
            .register_builtin_runner(move || permission_runner(permission_authorizer.clone()))
            .register_builtin_runner(move || rate_limit_runner(rate_limits.clone()));
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
