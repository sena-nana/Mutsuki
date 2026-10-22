use std::collections::{BTreeMap, BTreeSet};

use async_trait::async_trait;
use mutsuki_agent_contracts::{
    AgentEventEnvelope, AgentEventMeta, ArtifactRef, ResourceCellRef, ResourceRef,
};
use mutsuki_bot_conversation::ConversationRepository;
use mutsuki_bot_protocol::{
    AgentSessionScope, BotAccountRef, BotEventKind, BotPlatform, BotTarget, BotUser,
    ConversationPolicy,
};
use mutsuki_runtime_contracts::{
    ResourceAccess, ResourceId, ResourceLifetime, ResourceSealState, ResourceSemantic,
};

use super::*;

fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
    futures::executor::block_on(future)
}

#[derive(Default)]
struct Repository {
    bindings: Mutex<BTreeMap<String, AgentSessionBinding>>,
    events: Mutex<BTreeMap<(String, String), bool>>,
}

#[async_trait]
impl ConversationRepository for Repository {
    async fn session_binding(
        &self,
        key: &str,
    ) -> Result<Option<AgentSessionBinding>, ConversationError> {
        Ok(self.bindings.lock().unwrap().get(key).cloned())
    }

    async fn compare_and_set_session_binding(
        &self,
        key: &str,
        expected: Option<u64>,
        binding: AgentSessionBinding,
    ) -> Result<(), ConversationError> {
        let mut bindings = self.bindings.lock().unwrap();
        if bindings.get(key).map(|value| value.generation) != expected {
            return Err(ConversationError::GenerationConflict);
        }
        bindings.insert(key.into(), binding);
        Ok(())
    }

    async fn begin_agent_event(
        &self,
        binding_key: &str,
        event_id: &str,
        _turn_id: &str,
    ) -> Result<AgentEventClaim, ConversationError> {
        let mut events = self.events.lock().unwrap();
        let key = (binding_key.into(), event_id.into());
        Ok(match events.get(&key) {
            Some(true) => AgentEventClaim::Completed,
            Some(false) => AgentEventClaim::ResumePending,
            None => {
                events.insert(key, false);
                AgentEventClaim::New
            }
        })
    }

    async fn complete_agent_event(
        &self,
        binding_key: &str,
        event_id: &str,
    ) -> Result<(), ConversationError> {
        self.events
            .lock()
            .unwrap()
            .insert((binding_key.into(), event_id.into()), true);
        Ok(())
    }
}

#[derive(Default)]
struct FakeAgentClient {
    sessions: BTreeSet<String>,
    versions: BTreeMap<String, u64>,
    next_events: BTreeMap<String, AgentEventPage>,
    submitted: Vec<(String, String)>,
    started_profiles: Arc<Mutex<Vec<String>>>,
}

impl AgentBridgeClient for FakeAgentClient {
    fn get_session(&mut self, session_id: &str) -> Result<AgentSession, AgentWireError> {
        if !self.sessions.contains(session_id) {
            return Err(wire_error("agent.session.not_found"));
        }
        Ok(session(session_id))
    }

    fn start_session(
        &mut self,
        request: AgentSessionCreateRequest,
    ) -> Result<AgentSession, AgentWireError> {
        let id = request.session_id.unwrap();
        self.started_profiles
            .lock()
            .unwrap()
            .push(request.profile_id.clone());
        self.sessions.insert(id.clone());
        self.versions.insert(id.clone(), 0);
        Ok(session(&id))
    }

    fn submit_turn(
        &mut self,
        session_id: &str,
        expected_version: SessionVersion,
        turn_id: &str,
        _messages: Vec<AgentMessage>,
        idempotency_key: &str,
    ) -> Result<SessionVersion, AgentWireError> {
        assert_eq!(self.versions[session_id], expected_version.0);
        let next = expected_version.0 + 1;
        self.versions.insert(session_id.into(), next);
        self.submitted
            .push((session_id.into(), idempotency_key.into()));
        self.next_events.insert(
            session_id.into(),
            AgentEventPage {
                events: vec![AgentEventEnvelope {
                    session_id: session_id.into(),
                    sequence: next,
                    meta: AgentEventMeta::default(),
                    event: AgentEvent::FinalResponse {
                        turn_id: turn_id.into(),
                        summary: format!("reply-{next}"),
                        result: None,
                    },
                }],
                next_sequence: next,
                lost: 0,
                truncated: false,
            },
        );
        Ok(SessionVersion(next))
    }

    fn cancel_turn(
        &mut self,
        session_id: &str,
        _turn_id: &str,
        expected_version: SessionVersion,
    ) -> Result<SessionVersion, AgentWireError> {
        let next = expected_version.0 + 1;
        self.versions.insert(session_id.into(), next);
        Ok(SessionVersion(next))
    }

    fn fork_session(
        &mut self,
        source_session_id: &str,
        target_session_id: &str,
        expected_version: SessionVersion,
    ) -> Result<SessionVersion, AgentWireError> {
        assert_eq!(self.versions[source_session_id], expected_version.0);
        self.sessions.insert(target_session_id.into());
        self.versions.insert(target_session_id.into(), 1);
        Ok(SessionVersion(1))
    }

    fn events(
        &mut self,
        session_id: &str,
        _after_sequence: u64,
    ) -> Result<AgentEventPage, AgentWireError> {
        Ok(self.next_events.remove(session_id).unwrap())
    }
}

#[derive(Default)]
struct ReplayableAgentState {
    sessions: BTreeSet<String>,
    versions: BTreeMap<String, u64>,
    idempotency: BTreeMap<String, SessionVersion>,
    pages: BTreeMap<String, AgentEventPage>,
    submit_count: usize,
    event_calls: usize,
    fail_first_events: bool,
}

#[derive(Clone)]
struct ReplayableAgentClient {
    state: Arc<Mutex<ReplayableAgentState>>,
}

impl AgentBridgeClient for ReplayableAgentClient {
    fn get_session(&mut self, session_id: &str) -> Result<AgentSession, AgentWireError> {
        let state = self.state.lock().unwrap();
        state
            .sessions
            .contains(session_id)
            .then(|| session(session_id))
            .ok_or_else(|| wire_error("agent.session.not_found"))
    }

    fn start_session(
        &mut self,
        request: AgentSessionCreateRequest,
    ) -> Result<AgentSession, AgentWireError> {
        let session_id = request
            .session_id
            .ok_or_else(|| wire_error("agent.session.id_missing"))?;
        let mut state = self.state.lock().unwrap();
        state.sessions.insert(session_id.clone());
        state.versions.insert(session_id.clone(), 0);
        Ok(session(&session_id))
    }

    fn submit_turn(
        &mut self,
        session_id: &str,
        expected_version: SessionVersion,
        turn_id: &str,
        _messages: Vec<AgentMessage>,
        idempotency_key: &str,
    ) -> Result<SessionVersion, AgentWireError> {
        let mut state = self.state.lock().unwrap();
        if let Some(version) = state.idempotency.get(idempotency_key) {
            return Ok(*version);
        }
        if state.versions.get(session_id).copied() != Some(expected_version.0) {
            return Err(wire_error("agent.session.version_conflict"));
        }
        let next = SessionVersion(expected_version.0 + 1);
        state.versions.insert(session_id.into(), next.0);
        state.idempotency.insert(idempotency_key.into(), next);
        state.submit_count += 1;
        state.pages.insert(
            session_id.into(),
            AgentEventPage {
                events: vec![AgentEventEnvelope {
                    session_id: session_id.into(),
                    sequence: next.0,
                    meta: AgentEventMeta::default(),
                    event: AgentEvent::FinalResponse {
                        turn_id: turn_id.into(),
                        summary: "recovered reply".into(),
                        result: None,
                    },
                }],
                next_sequence: next.0,
                lost: 0,
                truncated: false,
            },
        );
        Ok(next)
    }

    fn cancel_turn(
        &mut self,
        session_id: &str,
        _turn_id: &str,
        expected_version: SessionVersion,
    ) -> Result<SessionVersion, AgentWireError> {
        let mut state = self.state.lock().unwrap();
        if state.versions.get(session_id).copied() != Some(expected_version.0) {
            return Err(wire_error("agent.session.version_conflict"));
        }
        let next = expected_version.0 + 1;
        state.versions.insert(session_id.into(), next);
        Ok(SessionVersion(next))
    }

    fn fork_session(
        &mut self,
        _source_session_id: &str,
        _target_session_id: &str,
        _expected_version: SessionVersion,
    ) -> Result<SessionVersion, AgentWireError> {
        Err(wire_error("agent.session.fork_not_used"))
    }

    fn events(
        &mut self,
        session_id: &str,
        after_sequence: u64,
    ) -> Result<AgentEventPage, AgentWireError> {
        let mut state = self.state.lock().unwrap();
        state.event_calls += 1;
        if state.fail_first_events && state.event_calls == 1 {
            return Err(wire_error("agent.transport.disconnected"));
        }
        let mut page = state
            .pages
            .get(session_id)
            .cloned()
            .unwrap_or(AgentEventPage {
                events: Vec::new(),
                next_sequence: after_sequence,
                lost: 0,
                truncated: false,
            });
        page.events.retain(|event| event.sequence > after_sequence);
        page.next_sequence = page
            .events
            .last()
            .map_or(after_sequence, |event| event.sequence);
        Ok(page)
    }
}

#[test]
fn private_and_group_two_turns_reuse_their_stable_sessions() {
    for target in [
        BotTarget::User {
            user_id: "user".into(),
        },
        BotTarget::Group {
            group_id: "group".into(),
        },
    ] {
        let repository = Arc::new(Repository::default());
        let bridge = BotAgentBridge::new(
            ConversationService::new(repository, policy()),
            Box::new(FakeAgentClient::default()),
            &BotStreamingStrategy::FinalOnly,
        );
        let first = block_on(bridge.submit_event(&event("one", target.clone()))).unwrap();
        let second = block_on(bridge.submit_event(&event("two", target))).unwrap();
        assert_eq!(first.binding.session_id, second.binding.session_id);
        assert_eq!(second.binding.session_version, 2);
        assert_eq!(second.binding.last_event_sequence, 2);
        assert_eq!(second.outgoing[0].plain_text(), "reply-2");
    }
}

#[test]
fn configured_default_profile_is_used_when_policy_does_not_bind_one() {
    let repository = Arc::new(Repository::default());
    let mut policy = policy();
    policy.agent_runtime_profile_id = None;
    let config = BotAgentConfigHandle::new(BotAgentConfig {
        enabled: true,
        connection_id: "injected".into(),
        default_profile_id: "configured-profile".into(),
        ..BotAgentConfig::default()
    })
    .unwrap();
    let started_profiles = Arc::new(Mutex::new(Vec::new()));
    let bridge = BotAgentBridge::new_with_config(
        ConversationService::new(repository, policy),
        Box::new(FakeAgentClient {
            started_profiles: started_profiles.clone(),
            ..FakeAgentClient::default()
        }),
        config,
    );

    let result = block_on(bridge.submit_event(&event(
        "configured-profile",
        BotTarget::User {
            user_id: "actor".into(),
        },
    )))
    .unwrap();
    assert_eq!(
        result.resolved.policy.agent_runtime_profile_id.as_deref(),
        Some("configured-profile")
    );
    assert_eq!(
        started_profiles.lock().unwrap().as_slice(),
        ["configured-profile"]
    );
}

#[test]
fn event_ext_profile_overlays_resolved_policy_ahead_of_config_default() {
    let repository = Arc::new(Repository::default());
    let mut policy = policy();
    policy.agent_runtime_profile_id = None;
    let config = BotAgentConfigHandle::new(BotAgentConfig {
        enabled: true,
        connection_id: "injected".into(),
        default_profile_id: "configured-profile".into(),
        ..BotAgentConfig::default()
    })
    .unwrap();
    let started_profiles = Arc::new(Mutex::new(Vec::new()));
    let bridge = BotAgentBridge::new_with_config(
        ConversationService::new(repository, policy),
        Box::new(FakeAgentClient {
            started_profiles: started_profiles.clone(),
            ..FakeAgentClient::default()
        }),
        config,
    );
    let mut source = event(
        "ext-profile",
        BotTarget::User {
            user_id: "actor".into(),
        },
    );
    source.ext.insert(
        BOT_EXT_AGENT_PROFILE_ID.into(),
        serde_json::Value::String("persona-profile".into()),
    );

    let result = block_on(bridge.submit_event(&source)).unwrap();
    assert_eq!(
        result.resolved.policy.agent_runtime_profile_id.as_deref(),
        Some("persona-profile")
    );
    assert_eq!(
        started_profiles.lock().unwrap().as_slice(),
        ["persona-profile"]
    );
}

#[test]
fn bind_profile_fills_missing_ext_and_does_not_overwrite_persona() {
    let source = event(
        "bind-profile",
        BotTarget::User {
            user_id: "actor".into(),
        },
    );
    let filled = bind_profile_event(&source, serde_json::json!({"profile_id": "config-profile"}));
    assert_eq!(
        filled.ext[BOT_EXT_AGENT_PROFILE_ID],
        serde_json::Value::String("config-profile".into())
    );

    let mut with_persona = source.clone();
    with_persona.ext.insert(
        BOT_EXT_AGENT_PROFILE_ID.into(),
        serde_json::Value::String("persona-profile".into()),
    );
    let preserved = bind_profile_event(
        &with_persona,
        serde_json::json!({"profile_id": "config-profile"}),
    );
    assert_eq!(
        preserved.ext[BOT_EXT_AGENT_PROFILE_ID],
        serde_json::Value::String("persona-profile".into())
    );

    let overridden = bind_profile_event(
        &with_persona,
        serde_json::json!({"profile_id": "config-profile", "override": true}),
    );
    assert_eq!(
        overridden.ext[BOT_EXT_AGENT_PROFILE_ID],
        serde_json::Value::String("config-profile".into())
    );

    let empty_config = bind_profile_event(&source, serde_json::json!({}));
    assert!(!empty_config.ext.contains_key(BOT_EXT_AGENT_PROFILE_ID));
}

#[test]
fn submit_reply_envelope_carries_source_message_and_user_without_reserving() {
    let mut source = event(
        "reply-source",
        BotTarget::User {
            user_id: "actor".into(),
        },
    );
    source.message.as_mut().unwrap().message_id = Some("mid-1".into());
    let context = overlay_reply_source_context(
        BotFlowContext {
            bot: None,
            target: None,
            actor: None,
            ext: BTreeMap::default(),
        },
        &source,
    );
    assert_eq!(
        context.ext[BOT_EXT_REPLY_SOURCE_MESSAGE_ID],
        serde_json::Value::String("mid-1".into())
    );
    assert_eq!(
        context.ext[BOT_EXT_REPLY_SOURCE_USER_ID],
        serde_json::Value::String("actor".into())
    );
}

#[test]
fn globally_disabled_bridge_rejects_before_creating_a_session() {
    let repository = Arc::new(Repository::default());
    let config = BotAgentConfigHandle::new(BotAgentConfig {
        enabled: false,
        ..BotAgentConfig::default()
    })
    .unwrap();
    let bridge = BotAgentBridge::new_with_config(
        ConversationService::new(repository.clone(), policy()),
        Box::new(FakeAgentClient::default()),
        config,
    );

    assert!(matches!(
        block_on(bridge.submit_event(&event(
            "disabled",
            BotTarget::Group {
                group_id: "group".into(),
            },
        ))),
        Err(BotAgentError::AgentDisabled)
    ));
    assert!(repository.bindings.lock().unwrap().is_empty());
}

#[test]
fn configured_concurrency_gate_releases_after_invocation_drop() {
    let gate = Arc::new(BotAgentConcurrencyGate::default());
    let permit = gate.try_acquire(1).expect("first invocation is admitted");
    assert!(gate.try_acquire(1).is_none());
    drop(permit);
    assert!(gate.try_acquire(1).is_some());
}

#[test]
fn final_response_keeps_non_resource_artifact_summary() {
    let source = event(
        "artifact",
        BotTarget::User {
            user_id: "actor".into(),
        },
    );
    let page = AgentEventPage {
        events: vec![
            AgentEventEnvelope {
                session_id: "session".into(),
                sequence: 1,
                meta: AgentEventMeta::default(),
                event: AgentEvent::FinalResponse {
                    turn_id: "turn".into(),
                    summary: "answer".into(),
                    result: None,
                },
            },
            AgentEventEnvelope {
                session_id: "session".into(),
                sequence: 2,
                meta: AgentEventMeta::default(),
                event: AgentEvent::ArtifactProduced {
                    turn_id: "turn".into(),
                    artifact: ArtifactRef {
                        artifact_id: "citation".into(),
                        media_type: "text/plain".into(),
                        summary: "citation summary".into(),
                        content_ref: None,
                        kind: Some("citation".into()),
                        size_bytes: None,
                        content_hash: None,
                        provenance: None,
                        open_hint: None,
                        action_hint: None,
                    },
                },
            },
        ],
        next_sequence: 2,
        lost: 0,
        truncated: false,
    };

    let outgoing = outgoing_messages_with_limit(
        &source,
        &page,
        &BotStreamingStrategy::FinalOnly,
        BOT_AGENT_DEFAULT_MAX_MESSAGE_BYTES,
    );

    assert_eq!(outgoing[0].plain_text(), "answer\n\ncitation summary");
}

#[test]
fn final_only_reply_is_segmented_at_the_configured_transport_boundary() {
    let source = event(
        "long-reply",
        BotTarget::User {
            user_id: "actor".into(),
        },
    );
    let page = AgentEventPage {
        events: vec![AgentEventEnvelope {
            session_id: "session".into(),
            sequence: 1,
            meta: AgentEventMeta::default(),
            event: AgentEvent::FinalResponse {
                turn_id: "turn".into(),
                summary: "abcdefghij".into(),
                result: None,
            },
        }],
        next_sequence: 1,
        lost: 0,
        truncated: false,
    };

    let outgoing =
        outgoing_messages_with_limit(&source, &page, &BotStreamingStrategy::FinalOnly, 4);
    assert_eq!(
        outgoing
            .iter()
            .map(BotMessage::plain_text)
            .collect::<Vec<_>>(),
        vec!["abcd", "efgh", "ij"]
    );
}

#[test]
fn segmented_streaming_uses_model_deltas_while_final_only_uses_final_response() {
    let source = event(
        "streaming",
        BotTarget::User {
            user_id: "actor".into(),
        },
    );
    let page = AgentEventPage {
        events: vec![
            AgentEventEnvelope {
                session_id: "session".into(),
                sequence: 1,
                meta: AgentEventMeta::default(),
                event: AgentEvent::ModelDelta {
                    turn_id: "turn".into(),
                    text: "partial ".into(),
                },
            },
            AgentEventEnvelope {
                session_id: "session".into(),
                sequence: 2,
                meta: AgentEventMeta::default(),
                event: AgentEvent::ModelDelta {
                    turn_id: "turn".into(),
                    text: "reply".into(),
                },
            },
            AgentEventEnvelope {
                session_id: "session".into(),
                sequence: 3,
                meta: AgentEventMeta::default(),
                event: AgentEvent::FinalResponse {
                    turn_id: "turn".into(),
                    summary: "final reply".into(),
                    result: None,
                },
            },
        ],
        next_sequence: 3,
        lost: 0,
        truncated: false,
    };

    let final_only = outgoing_messages_with_limit(
        &source,
        &page,
        &BotStreamingStrategy::FinalOnly,
        BOT_AGENT_DEFAULT_MAX_MESSAGE_BYTES,
    );
    let segmented = outgoing_messages_with_limit(
        &source,
        &page,
        &BotStreamingStrategy::SegmentMessages,
        BOT_AGENT_DEFAULT_MAX_MESSAGE_BYTES,
    );
    assert_eq!(final_only[0].plain_text(), "final reply");
    assert_eq!(segmented[0].plain_text(), "partial reply");
}

#[test]
fn completed_event_is_suppressed_before_agent_submit_after_bridge_reload() {
    let repository = Arc::new(Repository::default());
    let event = event(
        "duplicate",
        BotTarget::Group {
            group_id: "group".into(),
        },
    );
    let first_bridge = BotAgentBridge::new(
        ConversationService::new(repository.clone(), policy()),
        Box::new(FakeAgentClient::default()),
        &BotStreamingStrategy::FinalOnly,
    );
    let first = block_on(first_bridge.submit_event(&event)).unwrap();
    assert_eq!(first.binding.session_version, 1);
    let reloaded_bridge = BotAgentBridge::new(
        ConversationService::new(repository, policy()),
        Box::new(FakeAgentClient::default()),
        &BotStreamingStrategy::FinalOnly,
    );
    let duplicate = block_on(reloaded_bridge.submit_event(&event)).unwrap();
    assert_eq!(duplicate.binding.session_version, 1);
    assert!(duplicate.outgoing.is_empty());
}

#[test]
fn pending_event_resumes_after_bridge_reload_without_duplicate_submit() {
    let repository = Arc::new(Repository::default());
    let state = Arc::new(Mutex::new(ReplayableAgentState {
        fail_first_events: true,
        ..ReplayableAgentState::default()
    }));
    let event = event(
        "pending-reconnect",
        BotTarget::User {
            user_id: "actor".into(),
        },
    );
    let first_bridge = BotAgentBridge::new(
        ConversationService::new(repository.clone(), policy()),
        Box::new(ReplayableAgentClient {
            state: state.clone(),
        }),
        &BotStreamingStrategy::FinalOnly,
    );
    assert!(matches!(
        block_on(first_bridge.submit_event(&event)),
        Err(BotAgentError::AgentClient { ref code, .. })
            if code == "agent.transport.disconnected"
    ));

    let reloaded_bridge = BotAgentBridge::new(
        ConversationService::new(repository, policy()),
        Box::new(ReplayableAgentClient {
            state: state.clone(),
        }),
        &BotStreamingStrategy::FinalOnly,
    );
    let recovered = block_on(reloaded_bridge.submit_event(&event)).unwrap();
    assert_eq!(recovered.outgoing[0].plain_text(), "recovered reply");
    let state = state.lock().unwrap();
    assert_eq!(state.submit_count, 1);
    assert_eq!(state.event_calls, 2);
}

#[test]
fn cancel_fork_reset_and_regenerate_have_fenced_session_transitions() {
    let repository = Arc::new(Repository::default());
    let bridge = BotAgentBridge::new(
        ConversationService::new(repository, policy()),
        Box::new(FakeAgentClient::default()),
        &BotStreamingStrategy::FinalOnly,
    );
    let event = event(
        "actions",
        BotTarget::Group {
            group_id: "group".into(),
        },
    );
    let submitted = block_on(bridge.submit_event(&event)).unwrap();
    let cancelled = block_on(bridge.cancel_event(&event, &submitted.turn_id)).unwrap();
    assert_eq!(cancelled.session_version, 2);
    let forked = block_on(bridge.fork(&event)).unwrap();
    assert_ne!(forked.session_id, cancelled.session_id);
    assert_eq!(forked.session_version, 1);
    assert!(forked.generation > cancelled.generation);
    let reset = block_on(bridge.reset(&event)).unwrap();
    assert_ne!(reset.session_id, submitted.binding.session_id);
    assert_eq!(reset.session_version, 0);
    assert!(reset.generation > forked.generation);
    let regenerated = block_on(bridge.regenerate(&event)).unwrap();
    assert_eq!(regenerated.binding.session_id, reset.session_id);
    assert_eq!(regenerated.binding.session_version, 1);
    assert!(regenerated.turn_id.contains(":regenerate:"));
}

#[test]
fn validated_image_and_trace_metadata_enter_the_public_agent_message() {
    let mut event = event(
        "media",
        BotTarget::User {
            user_id: "user".into(),
        },
    );
    event
        .message
        .as_mut()
        .unwrap()
        .segments
        .push(MessageSegment::Image {
            resource: media_resource("image", "image/png"),
        });
    let message = event_message(
        &event,
        Some(&BotAgentTraceContext {
            trace_id: Some("trace".into()),
            correlation_id: Some("correlation".into()),
        }),
    )
    .unwrap();
    assert!(matches!(message.parts[0], AgentContentPart::Image { .. }));
    assert_eq!(message.metadata.as_ref().unwrap()["event_id"], "media");
    assert_eq!(message.metadata.as_ref().unwrap()["trace_id"], "trace");

    let MessageSegment::Image { resource } =
        event.message.as_mut().unwrap().segments.last_mut().unwrap()
    else {
        unreachable!()
    };
    resource.content_hash = None;
    assert!(matches!(
        event_message(&event, None),
        Err(BotAgentError::MediaResourceUnvalidated)
    ));
}

#[test]
fn conversation_injections_stay_in_metadata_not_user_text() {
    let mut event = event(
        "icl",
        BotTarget::Group {
            group_id: "group".into(),
        },
    );
    event.ext.insert(
        BOT_EXT_CONVERSATION_ICL.into(),
        serde_json::to_value([ConversationIclEntry {
            actor_id: "alice".into(),
            display_name: Some("Alice".into()),
            text: "hello group".into(),
            time_ms: 1,
        }])
        .unwrap(),
    );
    event.ext.insert(
        BOT_EXT_PERSONA_PROMPT.into(),
        serde_json::Value::String("you are a helper".into()),
    );
    let message = event_message(&event, None).unwrap();
    assert_eq!(
        message.content,
        event.message.as_ref().unwrap().plain_text()
    );
    assert!(message.parts.iter().all(|part| !matches!(
        part,
        AgentContentPart::Text { text } if text.contains("hello group") || text.contains("you are a helper")
    )));
    let injections: Vec<AgentMessageContextInjection> = serde_json::from_value(
        message.metadata.as_ref().unwrap()[AGENT_MESSAGE_CONTEXT_INJECTIONS_META].clone(),
    )
    .unwrap();
    assert!(
        injections
            .iter()
            .any(|injection| injection.source_kind == CONTEXT_SOURCE_ICL
                && injection.text.contains("hello group"))
    );
    assert!(
        injections
            .iter()
            .any(|injection| injection.source_kind == CONTEXT_SOURCE_PERSONA
                && injection.text == "you are a helper")
    );
}

fn bind_profile_event(source: &BotEvent, config: serde_json::Value) -> BotEvent {
    let task = Task::new(
        "bind-profile",
        BOT_AGENT_BIND_PROFILE_PROTOCOL_ID,
        serde_json::json!({}),
    );
    let invocation = BotNodeInvocation {
        flow_id: "flow".into(),
        graph_revision: 1,
        execution_id: "ex".into(),
        node_id: "bind".into(),
        input_port_id: "input".into(),
        wiring: BotNodeWiring::default(),
        config,
        input: BotFlowEventEnvelope {
            event_id: source.event_id.clone(),
            protocol_id: BOT_FLOW_BOT_EVENT_TYPE.into(),
            payload: BotFlowPayload {
                event_type: BotFlowTypeRef::new(BOT_FLOW_BOT_EVENT_TYPE, 1),
                value: serde_json::to_value(source).unwrap(),
            },
            context: BotFlowContext {
                bot: None,
                target: None,
                actor: None,
                ext: BTreeMap::default(),
            },
            trace_id: None,
            correlation_id: None,
        },
    };
    let result = bind_profile_node(&task, invocation).unwrap();
    let node: BotNodeResult = serde_json::from_value(result.output.unwrap()).unwrap();
    serde_json::from_value(node.outputs[0].event.payload.value.clone()).unwrap()
}

fn event(id: &str, target: BotTarget) -> BotEvent {
    BotEvent {
        event_id: id.into(),
        platform: BotPlatform::QqBot,
        bot: BotAccountRef {
            account_id: "main".into(),
            platform: BotPlatform::QqBot,
        },
        kind: BotEventKind::MessageCreated,
        time_ms: 1,
        target: target.clone(),
        actor: Some(BotUser {
            user_id: "actor".into(),
            display_name: None,
            avatar_url: None,
        }),
        message: Some(BotMessage::text(target, "hello")),
        raw: None,
        ext: BTreeMap::new(),
    }
}

fn policy() -> ConversationPolicy {
    ConversationPolicy {
        revision: 1,
        session_scope: AgentSessionScope::SharedConversation,
        business_profile_binding_id: None,
        agent_runtime_profile_id: Some("profile".into()),
        stt_enabled: false,
        tts_enabled: false,
        speech_reply_policy: BotSpeechReplyPolicy::default(),
        stt_selector_id: None,
        tts_selector_id: None,
        active_delivery_enabled: false,
    }
}

fn session(id: &str) -> AgentSession {
    AgentSession::new(
        id,
        "profile",
        resource(id),
        ResourceCellRef {
            cell_id: format!("cell-{id}").into(),
            resource_kind: "agent.session".into(),
            owner_plugin_id: "test".into(),
            schema: "agent.session.v1".into(),
            generation: 1,
            health: "ready".into(),
            reload_policy: "retain".into(),
        },
    )
}

fn resource(id: &str) -> ResourceRef {
    ResourceRef {
        ref_id: format!("ref-{id}").into(),
        resource_id: ResourceId {
            kind_id: "agent.session".into(),
            slot_id: id.into(),
            generation: 1,
            version: 1,
        },
        semantic: ResourceSemantic::VersionedSnapshot,
        provider_id: "test".into(),
        resource_kind: "agent.session".into(),
        schema: "agent.session.v1".into(),
        version: 1,
        generation: 1,
        access: ResourceAccess::Inline,
        size_hint: None,
        content_hash: None,
        lifetime: ResourceLifetime::Persistent,
        lease: None,
        seal_state: ResourceSealState::Sealed,
    }
}

fn media_resource(id: &str, schema: &str) -> ResourceRef {
    let mut resource = resource(id);
    resource.resource_kind = "blob".into();
    resource.schema = schema.into();
    resource.size_hint = Some(4);
    resource.content_hash = Some(format!("sha256:{id}"));
    resource
}

fn wire_error(code: &str) -> AgentWireError {
    AgentWireError {
        code: code.into(),
        message: code.into(),
        retryable: false,
    }
}
