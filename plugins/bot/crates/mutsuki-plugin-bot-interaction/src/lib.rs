// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::default_trait_access,
    clippy::missing_panics_doc,
    clippy::too_many_lines
)]

use mutsuki_bot_conversation::qq_conversation_from_event;
use mutsuki_bot_interaction::{InteractionError, InteractionService};
use mutsuki_bot_protocol::{
    BOT_FLOW_BOT_EVENT_TYPE, BOT_INTERACTION_CREATE_PROTOCOL_ID, BOT_INTERACTION_MATCH_PROTOCOL_ID,
    BOT_INTERACTION_SESSION_PROTOCOL_ID, BotEvent, BotEventKind, BotFlowEventEnvelope,
    BotFlowPayload, BotFlowTypeRef, BotInteractionCommand, BotInteractionSession, BotNodeBinding,
    BotNodeCatalogFragment, BotNodeDescriptor, BotNodeInvocation, BotNodeOutput,
    BotNodePortDescriptor, BotNodePortDirection, BotNodeResult, BotNodeRole, InteractionScope,
    InteractionStatus, InteractionWaitSpec,
};
use mutsuki_runtime_contracts::{
    CompletionBatch, ExecutionClass, PluginManifest, RunnerDescriptor, RunnerResult, Task,
    WorkBatch,
};
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeFailure, RuntimeResult};
use mutsuki_runtime_sdk::{
    PluginBuilder, ProtocolDescriptorBuilder, RunnerDescriptorBuilder, RuntimeClientRef,
    TaskAwaitRunnerAdapter, map_work_batch_entries,
};

pub const BOT_INTERACTION_PLUGIN_ID: &str = "mutsuki.plugin.bot.interaction";
pub const BOT_INTERACTION_RUNNER_ID: &str = "mutsuki.bot.interaction.runner";
pub const BOT_INTERACTION_MATCH_RUNNER_ID: &str = "mutsuki.bot.interaction.match";
pub const BOT_INTERACTION_CREATE_RUNNER_ID: &str = "mutsuki.bot.interaction.create";
pub const BOT_INTERACTION_MATCH_NODE_TYPE: &str = "mutsuki.bot.interaction.match";
pub const BOT_INTERACTION_CREATE_NODE_TYPE: &str = "mutsuki.bot.interaction.create";
pub const DEFAULT_INTERACTION_WAITER_TIMEOUT_MS: u64 = 60_000;
const DEFAULT_INTERACTION_STATE_REF_ID: &str = "await-next";

#[must_use]
pub fn bot_interaction_manifest() -> PluginManifest {
    PluginBuilder::new(BOT_INTERACTION_PLUGIN_ID)
        .runner_descriptor(interaction_descriptor())
        .runner_descriptor(match_descriptor())
        .runner_descriptor(create_descriptor())
        .protocol_handler(
            ProtocolDescriptorBuilder::new(BOT_INTERACTION_SESSION_PROTOCOL_ID)
                .input_schema(serde_json::json!({
                    "type": "object",
                    "required": ["action"]
                }))
                .output_schema(serde_json::json!({
                    "type": "object",
                    "required": ["session_id", "status", "version"]
                }))
                .error_schema(serde_json::json!({
                    "type": "object",
                    "required": ["code", "source", "route"]
                }))
                .build(),
            BOT_INTERACTION_RUNNER_ID,
            "bot-interaction",
        )
        .protocol_handler(
            ProtocolDescriptorBuilder::new(BOT_INTERACTION_MATCH_PROTOCOL_ID)
                .input_schema(serde_json::json!({"type": "object"}))
                .output_schema(serde_json::json!({
                    "type": "object",
                    "required": ["outputs", "metadata"]
                }))
                .error_schema(serde_json::json!({
                    "type": "object",
                    "required": ["code", "source", "route"]
                }))
                .build(),
            BOT_INTERACTION_MATCH_RUNNER_ID,
            "bot-interaction-match",
        )
        .protocol_handler(
            ProtocolDescriptorBuilder::new(BOT_INTERACTION_CREATE_PROTOCOL_ID)
                .input_schema(serde_json::json!({"type": "object"}))
                .output_schema(serde_json::json!({
                    "type": "object",
                    "required": ["outputs", "metadata"]
                }))
                .error_schema(serde_json::json!({
                    "type": "object",
                    "required": ["code", "source", "route"]
                }))
                .build(),
            BOT_INTERACTION_CREATE_RUNNER_ID,
            "bot-interaction-create",
        )
        .extension(
            interaction_node_catalog()
                .into_plugin_extension()
                .expect("interaction node catalog serializes"),
        )
        .build()
        .manifest
}

fn interaction_node_catalog() -> BotNodeCatalogFragment {
    BotNodeCatalogFragment {
        nodes: vec![match_node_descriptor(), create_node_descriptor()],
    }
}

fn match_node_descriptor() -> BotNodeDescriptor {
    BotNodeDescriptor {
        node_type_id: BOT_INTERACTION_MATCH_NODE_TYPE.into(),
        version: 1,
        title: "交互会话匹配".into(),
        category: "交互".into(),
        role: BotNodeRole::Match,
        binding: Some(BotNodeBinding {
            binding_id: format!("binding:{BOT_INTERACTION_MATCH_PROTOCOL_ID}"),
            protocol_id: BOT_INTERACTION_MATCH_PROTOCOL_ID.into(),
            runner_hint: Some(BOT_INTERACTION_MATCH_RUNNER_ID.into()),
        }),
        ports: vec![
            BotNodePortDescriptor {
                port_id: "event".into(),
                title: "事件".into(),
                direction: BotNodePortDirection::Input,
                event_type: BotFlowTypeRef::new(BOT_FLOW_BOT_EVENT_TYPE, 1),
                required: true,
            },
            BotNodePortDescriptor {
                port_id: "matched".into(),
                title: "已匹配".into(),
                direction: BotNodePortDirection::Output,
                event_type: BotFlowTypeRef::new(BOT_FLOW_BOT_EVENT_TYPE, 1),
                required: false,
            },
            BotNodePortDescriptor {
                port_id: "unmatched".into(),
                title: "未匹配".into(),
                direction: BotNodePortDirection::Output,
                event_type: BotFlowTypeRef::new(BOT_FLOW_BOT_EVENT_TYPE, 1),
                required: false,
            },
            BotNodePortDescriptor {
                port_id: "retry".into(),
                title: "重试提示".into(),
                direction: BotNodePortDirection::Output,
                event_type: BotFlowTypeRef::new("mutsuki.bot.message.send", 1),
                required: false,
            },
        ],
        config_schema: serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "state_ref_id": {"type": "string", "title": "状态引用"}
            }
        }),
    }
}

fn create_node_descriptor() -> BotNodeDescriptor {
    BotNodeDescriptor {
        node_type_id: BOT_INTERACTION_CREATE_NODE_TYPE.into(),
        version: 1,
        title: "创建交互等待".into(),
        category: "交互".into(),
        role: BotNodeRole::Processor,
        binding: Some(BotNodeBinding {
            binding_id: format!("binding:{BOT_INTERACTION_CREATE_PROTOCOL_ID}"),
            protocol_id: BOT_INTERACTION_CREATE_PROTOCOL_ID.into(),
            runner_hint: Some(BOT_INTERACTION_CREATE_RUNNER_ID.into()),
        }),
        ports: vec![
            BotNodePortDescriptor {
                port_id: "event".into(),
                title: "事件".into(),
                direction: BotNodePortDirection::Input,
                event_type: BotFlowTypeRef::new(BOT_FLOW_BOT_EVENT_TYPE, 1),
                required: true,
            },
            BotNodePortDescriptor {
                port_id: "output".into(),
                title: "事件".into(),
                direction: BotNodePortDirection::Output,
                event_type: BotFlowTypeRef::new(BOT_FLOW_BOT_EVENT_TYPE, 1),
                required: false,
            },
        ],
        config_schema: serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "timeout_ms": {
                    "type": "integer",
                    "minimum": 1,
                    "default": 60_000,
                    "title": "等待超时毫秒"
                },
                "state_ref_id": {
                    "type": "string",
                    "title": "状态引用",
                    "default": "await-next"
                }
            }
        }),
    }
}

#[must_use]
pub fn interaction_runner(
    client: RuntimeClientRef,
    service: InteractionService,
) -> Box<dyn Runner> {
    let factory = Box::new(move |_ctx, task: Task| {
        let service = service.clone();
        Box::pin(async move { interaction_result(&service, &task) })
            as std::pin::Pin<
                Box<dyn std::future::Future<Output = RuntimeResult<RunnerResult>> + Send>,
            >
    });
    Box::new(
        TaskAwaitRunnerAdapter::new(interaction_descriptor(), client, factory)
            .with_self_call_policy(false),
    )
}

pub struct InteractionMatchRunner {
    service: InteractionService,
    descriptor: RunnerDescriptor,
}

impl InteractionMatchRunner {
    #[must_use]
    pub fn new(service: InteractionService) -> Self {
        Self {
            service,
            descriptor: match_descriptor(),
        }
    }
}

impl Runner for InteractionMatchRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        map_work_batch_entries(&batch, |task| {
            let invocation = task
                .payload
                .decode_shared::<BotNodeInvocation>()
                .map_err(|error| runtime_error(task, error))?;
            interaction_node_result(&self.service, task, invocation.as_ref())
        })
    }
}

pub struct InteractionCreateRunner {
    service: InteractionService,
    descriptor: RunnerDescriptor,
}

impl InteractionCreateRunner {
    #[must_use]
    pub fn new(service: InteractionService) -> Self {
        Self {
            service,
            descriptor: create_descriptor(),
        }
    }
}

impl Runner for InteractionCreateRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        map_work_batch_entries(&batch, |task| {
            let invocation = task
                .payload
                .decode_shared::<BotNodeInvocation>()
                .map_err(|error| runtime_error(task, error))?;
            create_node_result(&self.service, task, invocation.as_ref())
        })
    }
}

fn interaction_descriptor() -> RunnerDescriptor {
    RunnerDescriptorBuilder::new(BOT_INTERACTION_RUNNER_ID, BOT_INTERACTION_PLUGIN_ID)
        .accepted_protocol(BOT_INTERACTION_SESSION_PROTOCOL_ID)
        .execution_class(ExecutionClass::Orchestration)
        .build()
}

fn match_descriptor() -> RunnerDescriptor {
    RunnerDescriptorBuilder::new(BOT_INTERACTION_MATCH_RUNNER_ID, BOT_INTERACTION_PLUGIN_ID)
        .accepted_protocol(BOT_INTERACTION_MATCH_PROTOCOL_ID)
        .execution_class(ExecutionClass::Orchestration)
        .build()
}

fn create_descriptor() -> RunnerDescriptor {
    RunnerDescriptorBuilder::new(BOT_INTERACTION_CREATE_RUNNER_ID, BOT_INTERACTION_PLUGIN_ID)
        .accepted_protocol(BOT_INTERACTION_CREATE_PROTOCOL_ID)
        .execution_class(ExecutionClass::Orchestration)
        .build()
}

fn interaction_result(service: &InteractionService, task: &Task) -> RuntimeResult<RunnerResult> {
    let command: BotInteractionCommand = serde_json::from_value(task.payload.to_value())
        .map_err(|error| RuntimeFailure::new(runtime_error(task, error)))?;
    let output = match command {
        BotInteractionCommand::Create { session } => service
            .create(session)
            .map(|()| serde_json::json!({"created": true})),
        BotInteractionCommand::MatchEvent { event, now_unix_ms } => {
            service.match_event(&event, now_unix_ms).and_then(|value| {
                serde_json::to_value(value)
                    .map_err(|error| InteractionError::Repository(error.to_string()))
            })
        }
        BotInteractionCommand::Cancel { session } => service
            .cancel(session)
            .map(|()| serde_json::json!({"cancelled": true})),
        BotInteractionCommand::Transition {
            session,
            next_state_ref_id,
            next_wait,
            retries_remaining,
        } => service
            .transition(session, next_state_ref_id, next_wait, retries_remaining)
            .and_then(|value| {
                serde_json::to_value(value)
                    .map_err(|error| InteractionError::Repository(error.to_string()))
            }),
        BotInteractionCommand::Recover { now_unix_ms } => {
            service.recover(now_unix_ms).and_then(|value| {
                serde_json::to_value(value)
                    .map_err(|error| InteractionError::Repository(error.to_string()))
            })
        }
        BotInteractionCommand::RecoverGeneration {
            now_unix_ms,
            active_generation,
        } => service
            .recover_generation(now_unix_ms, active_generation)
            .and_then(|value| {
                serde_json::to_value(value)
                    .map_err(|error| InteractionError::Repository(error.to_string()))
            }),
    }
    .map_err(|error| RuntimeFailure::new(runtime_error(task, error)))?;
    let mut result = RunnerResult::completed(task.task_id.clone());
    result.output = Some(output);
    Ok(result)
}

fn interaction_node_result(
    service: &InteractionService,
    task: &Task,
    invocation: &BotNodeInvocation,
) -> Result<RunnerResult, mutsuki_runtime_contracts::RuntimeError> {
    let event: BotEvent = serde_json::from_value(invocation.input.payload.value.clone())
        .map_err(|error| runtime_error(task, error))?;
    let expected_state = invocation
        .config
        .get("state_ref_id")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty());
    let matched = service
        .match_event(&event, event.time_ms.max(0).cast_unsigned())
        .map_err(|error| runtime_error(task, error))?;
    let matched =
        matched.filter(|item| expected_state.is_none_or(|expected| expected == item.state_ref_id));
    let mut outputs = Vec::new();
    match matched {
        Some(matched) if matched.accepted => {
            outputs.push(BotNodeOutput {
                port_id: "matched".into(),
                event: invocation.input.clone(),
            });
        }
        Some(matched) => {
            if let Some(prompt) = matched.retry_prompt {
                outputs.push(BotNodeOutput {
                    port_id: "retry".into(),
                    event: flow_output(
                        &invocation.input,
                        "mutsuki.bot.message.send",
                        serde_json::to_value(prompt).map_err(|error| runtime_error(task, error))?,
                    ),
                });
            }
            outputs.push(BotNodeOutput {
                port_id: "unmatched".into(),
                event: invocation.input.clone(),
            });
        }
        None => outputs.push(BotNodeOutput {
            port_id: "unmatched".into(),
            event: invocation.input.clone(),
        }),
    }
    let mut result = RunnerResult::completed(task.task_id.clone());
    result.output = Some(
        serde_json::to_value(BotNodeResult {
            outputs,
            metadata: Default::default(),
        })
        .map_err(|error| runtime_error(task, error))?,
    );
    Ok(result)
}

fn create_node_result(
    service: &InteractionService,
    task: &Task,
    invocation: &BotNodeInvocation,
) -> Result<RunnerResult, mutsuki_runtime_contracts::RuntimeError> {
    let event: BotEvent = serde_json::from_value(invocation.input.payload.value.clone())
        .map_err(|error| runtime_error(task, error))?;
    let session = session_from_event(&event, &invocation.config)
        .map_err(|error| runtime_error(task, error))?;
    match service.create(session) {
        Ok(()) | Err(InteractionError::WaiterConflict) => {}
        Err(error) => return Err(runtime_error(task, error)),
    }
    let mut result = RunnerResult::completed(task.task_id.clone());
    result.output = Some(
        serde_json::to_value(BotNodeResult {
            outputs: vec![BotNodeOutput {
                port_id: "output".into(),
                event: invocation.input.clone(),
            }],
            metadata: Default::default(),
        })
        .map_err(|error| runtime_error(task, error))?,
    );
    Ok(result)
}

fn session_from_event(
    event: &BotEvent,
    config: &serde_json::Value,
) -> Result<BotInteractionSession, InteractionError> {
    let conversation =
        qq_conversation_from_event(event).map_err(|_| InteractionError::UnsupportedTarget)?;
    let timeout_ms = config
        .get("timeout_ms")
        .and_then(serde_json::Value::as_u64)
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_INTERACTION_WAITER_TIMEOUT_MS);
    let state_ref_id = config
        .get("state_ref_id")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_INTERACTION_STATE_REF_ID)
        .to_owned();
    let now_unix_ms = event.time_ms.max(0).cast_unsigned();
    Ok(BotInteractionSession {
        session_id: format!("wait:{}", event.event_id),
        conversation,
        scope: InteractionScope::ActorInConversation,
        actor_id: event
            .actor
            .as_ref()
            .map(|actor| actor.user_id.clone())
            .filter(|value| !value.is_empty()),
        state_ref_id,
        wait: InteractionWaitSpec {
            event_kinds: vec![BotEventKind::MessageCreated],
            command: None,
            predicate_service_id: None,
            timeout_at_unix_ms: now_unix_ms.saturating_add(timeout_ms),
            retry_prompt: None,
        },
        status: InteractionStatus::Waiting,
        generation: 1,
        version: 1,
        exclusive: true,
        retries_remaining: 1,
    })
}

fn flow_output(
    source: &BotFlowEventEnvelope,
    event_type: &str,
    value: serde_json::Value,
) -> BotFlowEventEnvelope {
    BotFlowEventEnvelope {
        event_id: source.event_id.clone(),
        protocol_id: source.protocol_id.clone(),
        payload: BotFlowPayload {
            event_type: BotFlowTypeRef::new(event_type, 1),
            value,
        },
        context: source.context.clone(),
        trace_id: source.trace_id.clone(),
        correlation_id: source.correlation_id.clone(),
    }
}

fn runtime_error(
    task: &Task,
    error: impl std::fmt::Display,
) -> mutsuki_runtime_contracts::RuntimeError {
    mutsuki_runtime_contracts::RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
        BOT_INTERACTION_PLUGIN_ID,
        format!("{}.{}", task.task_id, error),
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use mutsuki_bot_interaction::{InteractionRepository, InteractionService};
    use mutsuki_bot_protocol::{
        BotAccountRef, BotConversationKind, BotEventKind, BotFlowContext, BotPlatform, BotTarget,
        BotUser, InteractionWaitSpec, QQ_CONVERSATION_REF_VERSION, QqConversationRef,
    };
    use mutsuki_runtime_contracts::Task;

    use super::*;

    #[derive(Default)]
    struct Repository {
        sessions: Mutex<BTreeMap<String, BotInteractionSession>>,
    }

    struct MatchAll;

    impl mutsuki_bot_interaction::InteractionConditionMatcher for MatchAll {
        fn command_matches(
            &self,
            _command: &str,
            _event: &BotEvent,
        ) -> Result<bool, InteractionError> {
            Ok(true)
        }

        fn predicate_matches(
            &self,
            _service_id: &str,
            _event: &BotEvent,
        ) -> Result<bool, InteractionError> {
            Ok(true)
        }
    }

    fn service(repository: Arc<Repository>) -> InteractionService {
        InteractionService::new(repository, Arc::new(MatchAll))
    }

    impl InteractionRepository for Repository {
        fn create(&self, session: BotInteractionSession) -> Result<(), InteractionError> {
            self.sessions
                .lock()
                .unwrap()
                .insert(session.session_id.clone(), session);
            Ok(())
        }

        fn active_for_origin(
            &self,
            origin_key: &str,
        ) -> Result<Vec<BotInteractionSession>, InteractionError> {
            Ok(self
                .sessions
                .lock()
                .unwrap()
                .values()
                .filter(|session| {
                    session.status == InteractionStatus::Waiting
                        && session.conversation.origin_key() == origin_key
                })
                .cloned()
                .collect())
        }

        fn compare_and_set(
            &self,
            expected_version: u64,
            session: BotInteractionSession,
        ) -> Result<(), InteractionError> {
            let mut sessions = self.sessions.lock().unwrap();
            if sessions
                .get(&session.session_id)
                .map(|current| current.version)
                != Some(expected_version)
            {
                return Err(InteractionError::GenerationConflict);
            }
            sessions.insert(session.session_id.clone(), session);
            Ok(())
        }

        fn recover_waiting(&self) -> Result<Vec<BotInteractionSession>, InteractionError> {
            Ok(self
                .sessions
                .lock()
                .unwrap()
                .values()
                .filter(|session| session.status == InteractionStatus::Waiting)
                .cloned()
                .collect())
        }
    }

    fn session(id: &str, actor: &str, timeout: u64) -> BotInteractionSession {
        BotInteractionSession {
            session_id: id.into(),
            conversation: conversation(),
            scope: InteractionScope::ActorInConversation,
            actor_id: Some(actor.into()),
            state_ref_id: format!("state-{id}"),
            wait: InteractionWaitSpec {
                event_kinds: vec![BotEventKind::MessageCreated],
                command: None,
                predicate_service_id: None,
                timeout_at_unix_ms: timeout,
                retry_prompt: None,
            },
            status: InteractionStatus::Waiting,
            generation: 1,
            version: 1,
            exclusive: true,
            retries_remaining: 1,
        }
    }

    fn conversation() -> QqConversationRef {
        QqConversationRef {
            version: QQ_CONVERSATION_REF_VERSION,
            account_id: "main".into(),
            kind: BotConversationKind::Group,
            user_id: None,
            group_id: Some("group".into()),
            guild_id: None,
            channel_id: None,
            thread_id: None,
        }
    }

    fn event(actor: &str) -> BotEvent {
        BotEvent {
            event_id: format!("event-{actor}"),
            platform: BotPlatform::QqBot,
            bot: BotAccountRef {
                account_id: "main".into(),
                platform: BotPlatform::QqBot,
            },
            kind: BotEventKind::MessageCreated,
            time_ms: 1,
            target: BotTarget::Group {
                group_id: "group".into(),
            },
            actor: Some(BotUser {
                user_id: actor.into(),
                display_name: None,
                avatar_url: None,
            }),
            message: None,
            raw: None,
            ext: BTreeMap::new(),
        }
    }

    #[test]
    fn create_processor_opens_waiter_and_next_message_matches() {
        let repository = Arc::new(Repository::default());
        let svc = service(repository.clone());
        let empty = event("actor");
        let invocation = BotNodeInvocation {
            flow_id: "qq.ai.orchestrated".into(),
            graph_revision: 1,
            execution_id: "exec".into(),
            node_id: "interaction-create".into(),
            input_port_id: "event".into(),
            config: serde_json::json!({"timeout_ms": 60_000}),
            input: BotFlowEventEnvelope {
                event_id: empty.event_id.clone(),
                protocol_id: BOT_INTERACTION_CREATE_PROTOCOL_ID.into(),
                payload: BotFlowPayload {
                    event_type: BotFlowTypeRef::new(BOT_FLOW_BOT_EVENT_TYPE, 1),
                    value: serde_json::to_value(&empty).unwrap(),
                },
                context: BotFlowContext {
                    bot: None,
                    target: None,
                    actor: None,
                    ext: Default::default(),
                },
                trace_id: None,
                correlation_id: None,
            },
        };
        let task = Task::new(
            "create-waiter",
            BOT_INTERACTION_CREATE_PROTOCOL_ID,
            serde_json::json!({}),
        );
        super::create_node_result(&svc, &task, &invocation).unwrap();
        let waiting = repository
            .active_for_origin(&conversation().origin_key())
            .unwrap();
        assert_eq!(waiting.len(), 1);
        assert_eq!(waiting[0].status, InteractionStatus::Waiting);
        assert_eq!(
            waiting[0].wait.timeout_at_unix_ms,
            1 + DEFAULT_INTERACTION_WAITER_TIMEOUT_MS
        );

        let follow_up = event("actor");
        let matched = svc.match_event(&follow_up, 2).unwrap().unwrap();
        assert!(matched.accepted);
        assert_eq!(matched.status, InteractionStatus::Completed);
    }

    #[test]
    fn create_processor_ignores_exclusive_conflict() {
        let repository = Arc::new(Repository::default());
        let svc = service(repository.clone());
        svc.create(session("wait", "actor", 1_000)).unwrap();
        let invocation = BotNodeInvocation {
            flow_id: "qq.ai.orchestrated".into(),
            graph_revision: 1,
            execution_id: "exec".into(),
            node_id: "interaction-create".into(),
            input_port_id: "event".into(),
            config: serde_json::json!({}),
            input: BotFlowEventEnvelope {
                event_id: "event-actor".into(),
                protocol_id: BOT_INTERACTION_CREATE_PROTOCOL_ID.into(),
                payload: BotFlowPayload {
                    event_type: BotFlowTypeRef::new(BOT_FLOW_BOT_EVENT_TYPE, 1),
                    value: serde_json::to_value(event("actor")).unwrap(),
                },
                context: BotFlowContext {
                    bot: None,
                    target: None,
                    actor: None,
                    ext: Default::default(),
                },
                trace_id: None,
                correlation_id: None,
            },
        };
        let task = Task::new(
            "create-conflict",
            BOT_INTERACTION_CREATE_PROTOCOL_ID,
            serde_json::json!({}),
        );
        super::create_node_result(&svc, &task, &invocation).unwrap();
        assert_eq!(
            repository
                .active_for_origin(&conversation().origin_key())
                .unwrap()
                .len(),
            1
        );
    }
}
