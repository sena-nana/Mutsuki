use std::sync::Arc;

use async_trait::async_trait;
use mutsuki_bot_conversation::qq_conversation_from_event;
use mutsuki_bot_protocol::{
    BOT_FLOW_MESSAGE_EVENT_TYPE, BOT_INTERACTION_SESSION_PROTOCOL_ID, BotEvent,
    BotFlowEventEnvelope, BotFlowPayload, BotFlowTypeRef, BotInteractionCommand,
    BotInteractionSession, BotNodeBinding, BotNodeCatalogFragment, BotNodeDescriptor,
    BotNodeInvocation, BotNodeOutput, BotNodePortDescriptor, BotNodePortDirection, BotNodeResult,
    BotNodeRole, InteractionMatch, InteractionScope, InteractionStatus, InteractionWaitSpec,
};
use mutsuki_runtime_contracts::{ExecutionClass, PluginManifest, RunnerResult, Task};
use mutsuki_runtime_core::{Runner, RuntimeFailure, RuntimeResult};
use mutsuki_runtime_sdk::{
    PluginBuilder, ProtocolDescriptorBuilder, RunnerDescriptorBuilder, RuntimeClientRef,
    TaskAwaitRunnerAdapter,
};
use thiserror::Error;

#[async_trait]
pub trait InteractionRepository: Send + Sync {
    async fn create(&self, session: BotInteractionSession) -> Result<(), InteractionError>;
    async fn active_for_origin(
        &self,
        origin_key: &str,
    ) -> Result<Vec<BotInteractionSession>, InteractionError>;
    async fn compare_and_set(
        &self,
        expected_version: u64,
        session: BotInteractionSession,
    ) -> Result<(), InteractionError>;
    async fn recover_waiting(&self) -> Result<Vec<BotInteractionSession>, InteractionError>;
}

pub trait InteractionConditionMatcher: Send + Sync {
    /// Tests whether an event satisfies a named interaction command.
    ///
    /// # Errors
    ///
    /// Returns a typed matcher error when the command cannot be evaluated.
    fn command_matches(&self, command: &str, event: &BotEvent) -> Result<bool, InteractionError>;

    /// Tests whether an event satisfies an owner-provided predicate service.
    ///
    /// # Errors
    ///
    /// Returns a typed matcher error when the predicate cannot be evaluated.
    fn predicate_matches(
        &self,
        service_id: &str,
        event: &BotEvent,
    ) -> Result<bool, InteractionError>;
}

#[derive(Clone)]
pub struct InteractionService {
    repository: Arc<dyn InteractionRepository>,
    matcher: Arc<dyn InteractionConditionMatcher>,
}

impl InteractionService {
    pub fn new(
        repository: Arc<dyn InteractionRepository>,
        matcher: Arc<dyn InteractionConditionMatcher>,
    ) -> Self {
        Self {
            repository,
            matcher,
        }
    }

    /// Creates a validated interaction waiter when its scope is available.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid waiter, an exclusive-scope conflict, or repository failure.
    pub async fn create(&self, session: BotInteractionSession) -> Result<(), InteractionError> {
        validate(&session)?;
        let origin = session.conversation.origin_key();
        let conflict = self
            .repository
            .active_for_origin(&origin)
            .await?
            .into_iter()
            .any(|current| conflicts(&current, &session));
        if conflict {
            return Err(InteractionError::WaiterConflict);
        }
        self.repository.create(session).await
    }

    /// Matches an event against the active waiters and persists the resulting transition.
    ///
    /// # Errors
    ///
    /// Returns an error for unsupported targets, matcher failure, or repository conflict.
    pub async fn match_event(
        &self,
        event: &BotEvent,
        now_unix_ms: u64,
    ) -> Result<Option<InteractionMatch>, InteractionError> {
        let conversation =
            qq_conversation_from_event(event).map_err(|_| InteractionError::UnsupportedTarget)?;
        let mut sessions = self
            .repository
            .active_for_origin(&conversation.origin_key())
            .await?;
        sessions.sort_by(|left, right| {
            right
                .exclusive
                .cmp(&left.exclusive)
                .then_with(|| left.session_id.cmp(&right.session_id))
        });
        for mut session in sessions {
            if session.wait.timeout_at_unix_ms <= now_unix_ms {
                session.status = InteractionStatus::TimedOut;
                session.version += 1;
                self.repository
                    .compare_and_set(session.version - 1, session)
                    .await?;
                continue;
            }
            if !actor_matches(&session, event) || !event_matches(&session, event) {
                continue;
            }
            let command_matches = match session.wait.command.as_deref() {
                Some(command) => self.matcher.command_matches(command, event)?,
                None => true,
            };
            let predicate_matches = match session.wait.predicate_service_id.as_deref() {
                Some(service_id) => self.matcher.predicate_matches(service_id, event)?,
                None => true,
            };
            let expected = session.version;
            session.version += 1;
            let accepted = command_matches && predicate_matches;
            if accepted {
                session.status = InteractionStatus::Completed;
            } else {
                session.retries_remaining = session.retries_remaining.saturating_sub(1);
                if session.retries_remaining == 0 {
                    session.status = InteractionStatus::Failed;
                }
            }
            let matched = InteractionMatch {
                session_id: session.session_id.clone(),
                event_id: event.event_id.clone(),
                next_version: session.version,
                accepted,
                status: session.status,
                state_ref_id: session.state_ref_id.clone(),
                retries_remaining: session.retries_remaining,
                retry_prompt: (!accepted && session.status == InteractionStatus::Waiting)
                    .then(|| session.wait.retry_prompt.clone())
                    .flatten(),
            };
            self.repository.compare_and_set(expected, session).await?;
            return Ok(Some(matched));
        }
        Ok(None)
    }

    /// Cancels a waiting interaction with an optimistic version check.
    ///
    /// # Errors
    ///
    /// Returns an error when the session is not waiting or persistence fails.
    pub async fn cancel(&self, mut session: BotInteractionSession) -> Result<(), InteractionError> {
        if session.status != InteractionStatus::Waiting {
            return Err(InteractionError::NotWaiting);
        }
        let expected = session.version;
        session.version += 1;
        session.status = InteractionStatus::Cancelled;
        self.repository.compare_and_set(expected, session).await
    }

    /// Publishes the next waiting step after a completed interaction.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid transition or repository conflict.
    pub async fn transition(
        &self,
        mut session: BotInteractionSession,
        next_state_ref_id: String,
        next_wait: InteractionWaitSpec,
        retries_remaining: u32,
    ) -> Result<BotInteractionSession, InteractionError> {
        if session.status != InteractionStatus::Completed
            || next_state_ref_id.trim().is_empty()
            || next_wait.timeout_at_unix_ms == 0
            || retries_remaining == 0
        {
            return Err(InteractionError::InvalidTransition);
        }
        let expected = session.version;
        session.version += 1;
        session.state_ref_id = next_state_ref_id;
        session.wait = next_wait;
        session.retries_remaining = retries_remaining;
        session.status = InteractionStatus::Waiting;
        self.repository
            .compare_and_set(expected, session.clone())
            .await?;
        Ok(session)
    }

    /// Recovers active waiters and persists timeouts.
    ///
    /// # Errors
    ///
    /// Returns an error when repository recovery or a timeout transition fails.
    pub async fn recover(
        &self,
        now_unix_ms: u64,
    ) -> Result<Vec<BotInteractionSession>, InteractionError> {
        let mut recovered = Vec::new();
        for mut session in self.repository.recover_waiting().await? {
            if session.wait.timeout_at_unix_ms <= now_unix_ms {
                let expected = session.version;
                session.version += 1;
                session.status = InteractionStatus::TimedOut;
                self.repository.compare_and_set(expected, session).await?;
            } else {
                recovered.push(session);
            }
        }
        Ok(recovered)
    }

    /// Recovers only waiters from the active runtime generation.
    ///
    /// # Errors
    ///
    /// Returns an error when cancelling stale waiters or persisting timeouts fails.
    pub async fn recover_generation(
        &self,
        now_unix_ms: u64,
        active_generation: u64,
    ) -> Result<Vec<BotInteractionSession>, InteractionError> {
        let mut recovered = Vec::new();
        for mut session in self.repository.recover_waiting().await? {
            let expected = session.version;
            if session.generation != active_generation {
                session.version += 1;
                session.status = InteractionStatus::Cancelled;
                self.repository.compare_and_set(expected, session).await?;
            } else if session.wait.timeout_at_unix_ms <= now_unix_ms {
                session.version += 1;
                session.status = InteractionStatus::TimedOut;
                self.repository.compare_and_set(expected, session).await?;
            } else {
                recovered.push(session);
            }
        }
        Ok(recovered)
    }
}

pub const BOT_INTERACTION_PLUGIN_ID: &str = "mutsuki.plugin.bot.interaction";
pub const BOT_INTERACTION_RUNNER_ID: &str = "mutsuki.bot.interaction.runner";

#[must_use]
pub fn bot_interaction_manifest() -> PluginManifest {
    PluginBuilder::new(BOT_INTERACTION_PLUGIN_ID)
        .runner_descriptor(interaction_descriptor())
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
        nodes: vec![BotNodeDescriptor {
            node_type_id: "mutsuki.bot.interaction.match".into(),
            version: 1,
            title: "交互会话匹配".into(),
            category: "交互".into(),
            role: BotNodeRole::Match,
            binding: Some(BotNodeBinding {
                binding_id: format!("binding:{BOT_INTERACTION_SESSION_PROTOCOL_ID}"),
                protocol_id: BOT_INTERACTION_SESSION_PROTOCOL_ID.into(),
                runner_hint: Some(BOT_INTERACTION_RUNNER_ID.into()),
            }),
            ports: vec![
                BotNodePortDescriptor {
                    port_id: "event".into(),
                    title: "消息".into(),
                    direction: BotNodePortDirection::Input,
                    event_type: BotFlowTypeRef::new(BOT_FLOW_MESSAGE_EVENT_TYPE, 1),
                    required: true,
                },
                BotNodePortDescriptor {
                    port_id: "matched".into(),
                    title: "已匹配".into(),
                    direction: BotNodePortDirection::Output,
                    event_type: BotFlowTypeRef::new("mutsuki.bot.interaction.match", 1),
                    required: false,
                },
                BotNodePortDescriptor {
                    port_id: "unmatched".into(),
                    title: "未匹配".into(),
                    direction: BotNodePortDirection::Output,
                    event_type: BotFlowTypeRef::new(BOT_FLOW_MESSAGE_EVENT_TYPE, 1),
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
            config_schema: serde_json::json!({"type": "object", "additionalProperties": false}),
        }],
    }
}

#[must_use]
pub fn interaction_runner(
    client: RuntimeClientRef,
    service: InteractionService,
) -> Box<dyn Runner> {
    let factory = Box::new(move |_ctx, task: Task| {
        let service = service.clone();
        Box::pin(async move { interaction_result(&service, &task).await })
            as std::pin::Pin<
                Box<dyn std::future::Future<Output = RuntimeResult<RunnerResult>> + Send>,
            >
    });
    Box::new(
        TaskAwaitRunnerAdapter::new(interaction_descriptor(), client, factory)
            .with_self_call_policy(false),
    )
}

fn interaction_descriptor() -> mutsuki_runtime_contracts::RunnerDescriptor {
    RunnerDescriptorBuilder::new(BOT_INTERACTION_RUNNER_ID, BOT_INTERACTION_PLUGIN_ID)
        .accepted_protocol(BOT_INTERACTION_SESSION_PROTOCOL_ID)
        .execution_class(ExecutionClass::Orchestration)
        .build()
}

async fn interaction_result(
    service: &InteractionService,
    task: &Task,
) -> RuntimeResult<RunnerResult> {
    let payload = task.payload.to_value();
    if let Ok(invocation) = serde_json::from_value::<BotNodeInvocation>(payload.clone()) {
        return interaction_node_result(service, task, invocation).await;
    }
    let command: BotInteractionCommand =
        serde_json::from_value(payload).map_err(|error| runtime_error(task, error))?;
    let output = match command {
        BotInteractionCommand::Create { session } => service
            .create(session)
            .await
            .map(|()| serde_json::json!({"created": true})),
        BotInteractionCommand::MatchEvent { event, now_unix_ms } => service
            .match_event(&event, now_unix_ms)
            .await
            .and_then(|value| {
                serde_json::to_value(value)
                    .map_err(|error| InteractionError::Repository(error.to_string()))
            }),
        BotInteractionCommand::Cancel { session } => service
            .cancel(session)
            .await
            .map(|()| serde_json::json!({"cancelled": true})),
        BotInteractionCommand::Transition {
            session,
            next_state_ref_id,
            next_wait,
            retries_remaining,
        } => service
            .transition(session, next_state_ref_id, next_wait, retries_remaining)
            .await
            .and_then(|value| {
                serde_json::to_value(value)
                    .map_err(|error| InteractionError::Repository(error.to_string()))
            }),
        BotInteractionCommand::Recover { now_unix_ms } => {
            service.recover(now_unix_ms).await.and_then(|value| {
                serde_json::to_value(value)
                    .map_err(|error| InteractionError::Repository(error.to_string()))
            })
        }
        BotInteractionCommand::RecoverGeneration {
            now_unix_ms,
            active_generation,
        } => service
            .recover_generation(now_unix_ms, active_generation)
            .await
            .and_then(|value| {
                serde_json::to_value(value)
                    .map_err(|error| InteractionError::Repository(error.to_string()))
            }),
    }
    .map_err(|error| runtime_error(task, error))?;
    let mut result = RunnerResult::completed(task.task_id.clone());
    result.output = Some(output);
    Ok(result)
}

async fn interaction_node_result(
    service: &InteractionService,
    task: &Task,
    invocation: BotNodeInvocation,
) -> RuntimeResult<RunnerResult> {
    let event: BotEvent = serde_json::from_value(invocation.input.payload.value.clone())
        .map_err(|error| runtime_error(task, error))?;
    let matched = service
        .match_event(&event, event.time_ms.max(0).cast_unsigned())
        .await
        .map_err(|error| runtime_error(task, error))?;
    let mut outputs = Vec::new();
    if let Some(matched) = matched {
        let retry_prompt = matched.retry_prompt.clone();
        outputs.push(BotNodeOutput {
            port_id: "matched".into(),
            event: flow_output(
                &invocation.input,
                "mutsuki.bot.interaction.match",
                serde_json::to_value(matched).map_err(|error| runtime_error(task, error))?,
            ),
        });
        if let Some(prompt) = retry_prompt {
            outputs.push(BotNodeOutput {
                port_id: "retry".into(),
                event: flow_output(
                    &invocation.input,
                    "mutsuki.bot.message.send",
                    serde_json::to_value(prompt).map_err(|error| runtime_error(task, error))?,
                ),
            });
        }
    } else {
        outputs.push(BotNodeOutput {
            port_id: "unmatched".into(),
            event: invocation.input,
        });
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

fn runtime_error(task: &Task, error: impl std::fmt::Display) -> RuntimeFailure {
    RuntimeFailure::new(mutsuki_runtime_contracts::RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
        BOT_INTERACTION_PLUGIN_ID,
        format!("{}.{}", task.task_id, error),
    ))
}

fn validate(session: &BotInteractionSession) -> Result<(), InteractionError> {
    if session.session_id.trim().is_empty()
        || session.state_ref_id.trim().is_empty()
        || session.status != InteractionStatus::Waiting
        || session.retries_remaining == 0
        || (session.scope == InteractionScope::ActorInConversation
            && session.actor_id.as_deref().is_none_or(str::is_empty))
    {
        return Err(InteractionError::InvalidSession);
    }
    Ok(())
}

fn conflicts(left: &BotInteractionSession, right: &BotInteractionSession) -> bool {
    left.status == InteractionStatus::Waiting
        && (left.exclusive || right.exclusive)
        && (left.scope == InteractionScope::Conversation
            || right.scope == InteractionScope::Conversation
            || left.actor_id == right.actor_id)
}

fn actor_matches(session: &BotInteractionSession, event: &BotEvent) -> bool {
    session.scope == InteractionScope::Conversation
        || event.actor.as_ref().map(|actor| actor.user_id.as_str()) == session.actor_id.as_deref()
}

fn event_matches(session: &BotInteractionSession, event: &BotEvent) -> bool {
    session.wait.event_kinds.is_empty() || session.wait.event_kinds.contains(&event.kind)
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum InteractionError {
    #[error("interaction session is invalid")]
    InvalidSession,
    #[error("an exclusive interaction waiter already owns this scope")]
    WaiterConflict,
    #[error("interaction session is not waiting")]
    NotWaiting,
    #[error("interaction session transition is invalid")]
    InvalidTransition,
    #[error("Bot event target is not a QQ conversation")]
    UnsupportedTarget,
    #[error("interaction repository generation conflict")]
    GenerationConflict,
    #[error("interaction repository failed: {0}")]
    Repository(String),
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use mutsuki_bot_protocol::{
        BotAccountRef, BotConversationKind, BotEventKind, BotPlatform, BotTarget, BotUser,
        InteractionWaitSpec, QQ_CONVERSATION_REF_VERSION, QqConversationRef,
    };

    use super::*;

    #[derive(Default)]
    struct Repository {
        sessions: Mutex<BTreeMap<String, BotInteractionSession>>,
    }

    struct MatchAll;

    struct MatchCode;

    impl InteractionConditionMatcher for MatchAll {
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

    impl InteractionConditionMatcher for MatchCode {
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
            event: &BotEvent,
        ) -> Result<bool, InteractionError> {
            Ok(event.event_id == "event-valid")
        }
    }

    fn service(repository: Arc<Repository>) -> InteractionService {
        InteractionService::new(repository, Arc::new(MatchAll))
    }

    #[async_trait]
    impl InteractionRepository for Repository {
        async fn create(&self, session: BotInteractionSession) -> Result<(), InteractionError> {
            self.sessions
                .lock()
                .unwrap()
                .insert(session.session_id.clone(), session);
            Ok(())
        }

        async fn active_for_origin(
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

        async fn compare_and_set(
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

        async fn recover_waiting(&self) -> Result<Vec<BotInteractionSession>, InteractionError> {
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

    #[test]
    fn actor_waiter_ignores_other_member_and_recovers_across_service_restart() {
        futures::executor::block_on(async {
            let repository = Arc::new(Repository::default());
            service(repository.clone())
                .create(session("wait", "actor", 1_000))
                .await
                .unwrap();
            assert!(
                service(repository.clone())
                    .match_event(&event("other"), 100)
                    .await
                    .unwrap()
                    .is_none()
            );
            let restarted = service(repository.clone());
            assert_eq!(restarted.recover(100).await.unwrap().len(), 1);
            let matched = restarted
                .match_event(&event("actor"), 101)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(matched.session_id, "wait");
            assert_eq!(
                repository.sessions.lock().unwrap()["wait"].status,
                InteractionStatus::Completed
            );
        });
    }

    #[test]
    fn exclusive_waiter_rejects_conflict_and_timeout_is_persisted() {
        futures::executor::block_on(async {
            let repository = Arc::new(Repository::default());
            let service = service(repository.clone());
            service.create(session("first", "actor", 50)).await.unwrap();
            assert_eq!(
                service.create(session("second", "actor", 50)).await,
                Err(InteractionError::WaiterConflict)
            );
            assert!(service.recover(50).await.unwrap().is_empty());
            assert_eq!(
                repository.sessions.lock().unwrap()["first"].status,
                InteractionStatus::TimedOut
            );
        });
    }

    #[test]
    fn rejected_attempt_consumes_retry_and_completed_step_can_transition() {
        futures::executor::block_on(async {
            let repository = Arc::new(Repository::default());
            let service = InteractionService::new(repository.clone(), Arc::new(MatchCode));
            let mut first = session("verification", "actor", 1_000);
            first.retries_remaining = 2;
            first.wait.predicate_service_id = Some("verify-code".into());
            service.create(first).await.unwrap();

            let rejected = service
                .match_event(&event("actor"), 100)
                .await
                .unwrap()
                .unwrap();
            assert!(!rejected.accepted);
            assert_eq!(rejected.status, InteractionStatus::Waiting);
            assert_eq!(rejected.retries_remaining, 1);

            let mut valid = event("actor");
            valid.event_id = "event-valid".into();
            let accepted = service.match_event(&valid, 101).await.unwrap().unwrap();
            assert!(accepted.accepted);
            assert_eq!(accepted.status, InteractionStatus::Completed);

            let completed = repository.sessions.lock().unwrap()["verification"].clone();
            let transitioned = service
                .transition(
                    completed,
                    "confirm-profile".into(),
                    InteractionWaitSpec {
                        event_kinds: vec![BotEventKind::MessageCreated],
                        command: None,
                        predicate_service_id: None,
                        timeout_at_unix_ms: 2_000,
                        retry_prompt: None,
                    },
                    1,
                )
                .await
                .unwrap();
            assert_eq!(transitioned.status, InteractionStatus::Waiting);
            assert_eq!(transitioned.state_ref_id, "confirm-profile");
            assert_eq!(transitioned.version, accepted.next_version + 1);
        });
    }

    #[test]
    fn reload_cancels_waiters_from_an_old_generation() {
        futures::executor::block_on(async {
            let repository = Arc::new(Repository::default());
            let service = service(repository.clone());
            service
                .create(session("old", "actor", 1_000))
                .await
                .unwrap();

            assert!(service.recover_generation(100, 2).await.unwrap().is_empty());
            assert_eq!(
                repository.sessions.lock().unwrap()["old"].status,
                InteractionStatus::Cancelled
            );
        });
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
}
