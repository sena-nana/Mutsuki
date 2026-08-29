//! Authenticated Web Console bridge for owner-managed Agent connections.
// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::cast_possible_truncation,
    clippy::default_constructed_unit_structs,
    clippy::map_unwrap_or,
    clippy::missing_errors_doc,
    clippy::needless_pass_by_value,
    clippy::too_many_lines
)]
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mutsuki_agent_contracts::{
    AgentConnectionConfig, AgentConnectionManagementApi, AgentEvent, AgentEventEnvelope,
    AgentEventPage, AgentManagementError, AgentSession, AgentSessionManagementApi,
    InteractionResolution, PermissionDecision, SessionVersion,
};
use mutsuki_web_extension_api::{
    ExtensionError, RpcRegistry, WebExtension, WebExtensionDescriptor, content_hash,
    load_bundled_manifest,
};
use mutsuki_web_protocol::{
    AssetEntry, EXTENSION_MANIFEST_VERSION, ExtensionManifest, WEB_PROTOCOL_VERSION,
    WebFrontendAssets,
};
use serde_json::{Value, json};

pub const PLUGIN_ID: &str = "bot-agent";
pub const PLUGIN_VERSION: &str = "0.1.0";
pub const CAPABILITY_CONNECTION_READ: &str = "agent.connection.read";
pub const CAPABILITY_CONNECTION_WRITE: &str = "agent.connection.write";
pub const CAPABILITY_SESSION_READ: &str = "agent.session.read";
pub const CAPABILITY_SESSION_WRITE: &str = "agent.session.write";
const CONSOLE_MESSAGE_TAIL: usize = 20;
const CONSOLE_PAGE_BUDGET_BYTES: usize = 96 * 1024;
const CONSOLE_EVENT_GET_BUDGET_BYTES: usize = 200 * 1024;
const CONSOLE_FIELD_MAX_CHARS: usize = 512;
pub type AgentConnectionManagementResolver =
    Arc<dyn Fn() -> Result<Arc<dyn AgentConnectionManagementApi>, String> + Send + Sync>;
pub type LocalAgentManagementResolver =
    Arc<dyn Fn() -> Result<Arc<dyn AgentSessionManagementApi>, String> + Send + Sync>;

pub struct BotAgentWebExtension {
    connections: Option<AgentConnectionManagementResolver>,
    sessions: Option<LocalAgentManagementResolver>,
    assets_root: Option<PathBuf>,
}

impl BotAgentWebExtension {
    #[must_use]
    pub fn new(connections: Option<Arc<dyn AgentConnectionManagementApi>>) -> Self {
        Self {
            connections: connections.map(|manager| {
                Arc::new(move || Ok(manager.clone())) as AgentConnectionManagementResolver
            }),
            sessions: None,
            assets_root: None,
        }
    }

    #[must_use]
    pub fn with_connection_resolver(
        mut self,
        connections: Option<AgentConnectionManagementResolver>,
    ) -> Self {
        self.connections = connections;
        self
    }

    #[must_use]
    pub fn with_sessions(mut self, sessions: Option<LocalAgentManagementResolver>) -> Self {
        self.sessions = sessions;
        self
    }

    #[must_use]
    pub fn with_frontend_assets(mut self, root: impl Into<PathBuf>) -> Self {
        self.assets_root = Some(root.into());
        self
    }
}

impl WebExtension for BotAgentWebExtension {
    fn descriptor(&self) -> WebExtensionDescriptor {
        manifest(
            self.frontend_assets()
                .map(|assets| assets.manifest.assets)
                .unwrap_or_default(),
        )
    }

    fn frontend_assets(&self) -> Option<WebFrontendAssets> {
        let root = self.assets_root.as_ref()?;
        Some(WebFrontendAssets {
            manifest: load_bundled_manifest(root, manifest).ok()?,
            root_dir: root.clone(),
        })
    }

    fn register_rpc(&self, registry: &mut RpcRegistry) -> Result<(), ExtensionError> {
        if let Some(manager) = &self.connections {
            let manager = manager.clone();
            registry.register_contextual("connections.snapshot", move |context, _params| {
                context.require(CAPABILITY_CONNECTION_READ)?;
                serde_json::to_value(resolve_connections(&manager)?.snapshot())
                    .map_err(encode_error)
            });

            let manager = self.connections.as_ref().expect("checked").clone();
            registry.register_async_contextual("connections.test", move |context, params| {
                let manager = manager.clone();
                async move {
                    context.require(CAPABILITY_CONNECTION_WRITE)?;
                    let config = decode::<AgentConnectionConfig>(&params, "config")?;
                    let manager = resolve_connections(&manager)?;
                    let status =
                        tokio::task::spawn_blocking(move || manager.test_connection(config))
                            .await
                            .map_err(join_error)?
                            .map_err(agent_error)?;
                    serde_json::to_value(status).map_err(encode_error)
                }
            });

            let manager = self.connections.as_ref().expect("checked").clone();
            registry.register_async_contextual("connections.upsert", move |context, params| {
                let manager = manager.clone();
                async move {
                    context.require(CAPABILITY_CONNECTION_WRITE)?;
                    let expected_revision = required_u64(&params, "expected_revision")?;
                    let config = decode::<AgentConnectionConfig>(&params, "config")?;
                    let manager = resolve_connections(&manager)?;
                    let status = tokio::task::spawn_blocking(move || {
                        manager.upsert(expected_revision, config)
                    })
                    .await
                    .map_err(join_error)?
                    .map_err(agent_error)?;
                    serde_json::to_value(status).map_err(encode_error)
                }
            });

            let manager = self.connections.as_ref().expect("checked").clone();
            registry.register_async_contextual("connections.reconnect", move |context, params| {
                let manager = manager.clone();
                async move {
                    context.require(CAPABILITY_CONNECTION_WRITE)?;
                    let expected_revision = required_u64(&params, "expected_revision")?;
                    let connection_id = required_str(&params, "connection_id")?;
                    let manager = resolve_connections(&manager)?;
                    let status = tokio::task::spawn_blocking(move || {
                        manager.reconnect(expected_revision, &connection_id)
                    })
                    .await
                    .map_err(join_error)?
                    .map_err(agent_error)?;
                    serde_json::to_value(status).map_err(encode_error)
                }
            });
        }

        if let Some(sessions) = &self.sessions {
            let provider_sessions = sessions.clone();
            registry.register_async_contextual("provider.test", move |context, _params| {
                let provider_sessions = provider_sessions.clone();
                async move {
                    context.require(CAPABILITY_CONNECTION_WRITE)?;
                    let sessions = resolve_sessions(&provider_sessions)?;
                    tokio::task::spawn_blocking(move || sessions.test_provider())
                        .await
                        .map_err(join_error)?
                        .map_err(wire_error)?;
                    Ok(json!({"ok": true}))
                }
            });

            let sessions = sessions.clone();
            registry.register_contextual("sessions.list", move |context, params| {
                context.require(CAPABILITY_SESSION_READ)?;
                let after = params.get("after_session_id").and_then(Value::as_str);
                let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(20) as usize;
                serde_json::to_value(
                    resolve_sessions(&sessions)?
                        .list_sessions(after, limit)
                        .map_err(wire_error)?,
                )
                .map_err(encode_error)
            });

            let sessions = self.sessions.as_ref().expect("checked").clone();
            registry.register_contextual("sessions.get", move |context, params| {
                context.require(CAPABILITY_SESSION_READ)?;
                let mut session = resolve_sessions(&sessions)?
                    .session(&required_str(&params, "session_id")?)
                    .map_err(wire_error)?;
                slim_session_for_console(&mut session);
                serde_json::to_value(session).map_err(encode_error)
            });

            let sessions = self.sessions.as_ref().expect("checked").clone();
            registry.register_contextual("sessions.state", move |context, params| {
                context.require(CAPABILITY_SESSION_READ)?;
                serde_json::to_value(
                    resolve_sessions(&sessions)?
                        .session_state(&required_str(&params, "session_id")?)
                        .map_err(wire_error)?,
                )
                .map_err(encode_error)
            });

            let sessions = self.sessions.as_ref().expect("checked").clone();
            registry.register_contextual("sessions.events", move |context, params| {
                context.require(CAPABILITY_SESSION_READ)?;
                let after = params
                    .get("after_sequence")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(100) as u32;
                let events = resolve_sessions(&sessions)?
                    .events_after(&required_str(&params, "session_id")?, after, limit)
                    .map_err(wire_error)?;
                serde_json::to_value(pack_event_page(events)).map_err(encode_error)
            });

            let sessions = self.sessions.as_ref().expect("checked").clone();
            registry.register_contextual("sessions.event_get", move |context, params| {
                context.require(CAPABILITY_SESSION_READ)?;
                let session_id = required_str(&params, "session_id")?;
                let sequence = required_u64(&params, "sequence")?;
                let after = sequence.saturating_sub(1);
                let events = resolve_sessions(&sessions)?
                    .events_after(&session_id, after, 1)
                    .map_err(wire_error)?;
                let event = events
                    .into_iter()
                    .find(|event| event.sequence == sequence)
                    .ok_or_else(|| {
                        ExtensionError::Registration(
                            json!({
                                "code": "agent.session.event_not_found",
                                "message": format!("event {sequence} was not found"),
                                "retryable": false
                            })
                            .to_string(),
                        )
                    })?;
                encode_event_get(event)
            });

            let sessions = self.sessions.as_ref().expect("checked").clone();
            registry.register_contextual("sessions.approve", move |context, params| {
                context.require(CAPABILITY_SESSION_WRITE)?;
                serde_json::to_value(
                    resolve_sessions(&sessions)?
                        .approve(decode::<PermissionDecision>(&params, "decision")?)
                        .map_err(wire_error)?,
                )
                .map_err(encode_error)
            });

            let sessions = self.sessions.as_ref().expect("checked").clone();
            registry.register_contextual("sessions.reject", move |context, params| {
                context.require(CAPABILITY_SESSION_WRITE)?;
                serde_json::to_value(
                    resolve_sessions(&sessions)?
                        .reject(decode::<PermissionDecision>(&params, "decision")?)
                        .map_err(wire_error)?,
                )
                .map_err(encode_error)
            });

            let sessions = self.sessions.as_ref().expect("checked").clone();
            registry.register_contextual("sessions.interact", move |context, params| {
                context.require(CAPABILITY_SESSION_WRITE)?;
                serde_json::to_value(
                    resolve_sessions(&sessions)?
                        .resolve_interaction(decode::<InteractionResolution>(
                            &params,
                            "resolution",
                        )?)
                        .map_err(wire_error)?,
                )
                .map_err(encode_error)
            });

            let sessions = self.sessions.as_ref().expect("checked").clone();
            registry.register_contextual("sessions.cancel_turn", move |context, params| {
                context.require(CAPABILITY_SESSION_WRITE)?;
                serde_json::to_value(
                    resolve_sessions(&sessions)?
                        .cancel_turn(
                            &required_str(&params, "session_id")?,
                            &required_str(&params, "turn_id")?,
                            SessionVersion(required_u64(&params, "expected_version")?),
                        )
                        .map_err(wire_error)?,
                )
                .map_err(encode_error)
            });

            let sessions = self.sessions.as_ref().expect("checked").clone();
            registry.register_contextual("sessions.close", move |context, params| {
                context.require(CAPABILITY_SESSION_WRITE)?;
                resolve_sessions(&sessions)?
                    .close_session(
                        &required_str(&params, "session_id")?,
                        SessionVersion(required_u64(&params, "expected_version")?),
                    )
                    .map_err(wire_error)?;
                Ok(json!({"closed": true}))
            });
        }

        Ok(())
    }

    fn register_events(
        &self,
        _registry: &mut mutsuki_web_extension_api::EventRegistry,
    ) -> Result<(), ExtensionError> {
        Ok(())
    }
}

fn slim_session_for_console(session: &mut AgentSession) {
    session.events.clear();
    if session.messages.len() > CONSOLE_MESSAGE_TAIL {
        let keep_from = session.messages.len() - CONSOLE_MESSAGE_TAIL;
        session.messages = session.messages.split_off(keep_from);
    }
    for message in &mut session.messages {
        truncate_string(&mut message.content, CONSOLE_FIELD_MAX_CHARS);
    }
}

fn pack_event_page(events: Vec<AgentEventEnvelope>) -> AgentEventPage {
    let fetched = events.len();
    let mut selected = Vec::new();
    let mut used = 64;
    for event in events.into_iter().rev().map(slim_envelope) {
        let size = encoded_len(&event);
        if !selected.is_empty() && used + size > CONSOLE_PAGE_BUDGET_BYTES {
            break;
        }
        if selected.is_empty() && size > CONSOLE_EVENT_GET_BUDGET_BYTES {
            continue;
        }
        used += size;
        selected.push(event);
    }
    selected.reverse();
    AgentEventPage {
        next_sequence: selected.last().map(|event| event.sequence).unwrap_or(0),
        lost: 0,
        truncated: selected.len() < fetched,
        events: selected,
    }
}

fn slim_envelope(mut envelope: AgentEventEnvelope) -> AgentEventEnvelope {
    slim_event(&mut envelope.event);
    if envelope.meta.summary.len() > CONSOLE_FIELD_MAX_CHARS {
        truncate_string(&mut envelope.meta.summary, CONSOLE_FIELD_MAX_CHARS);
    }
    envelope
}

fn slim_event(event: &mut AgentEvent) {
    match event {
        AgentEvent::UserMessage { content, .. }
        | AgentEvent::ModelDelta { text: content, .. }
        | AgentEvent::ReasoningDelta { text: content, .. }
        | AgentEvent::CommandOutput { chunk: content, .. }
        | AgentEvent::ToolResult {
            summary: content, ..
        }
        | AgentEvent::ToolCallCompleted {
            summary: content, ..
        }
        | AgentEvent::CommandExited {
            summary: content, ..
        }
        | AgentEvent::FinalResponse {
            summary: content, ..
        } => truncate_string(content, CONSOLE_FIELD_MAX_CHARS),
        AgentEvent::ToolCall { input, .. } | AgentEvent::ToolCallStarted { input, .. } => {
            slim_json(input);
        }
        AgentEvent::Failed { error, .. } => {
            truncate_string(&mut error.message, CONSOLE_FIELD_MAX_CHARS);
        }
        _ => {}
    }
}

fn slim_json(value: &mut Value) {
    match value {
        Value::String(text) => truncate_string(text, CONSOLE_FIELD_MAX_CHARS),
        Value::Array(items) if encoded_len(items) > CONSOLE_FIELD_MAX_CHARS * 4 => {
            *value = json!({ "_truncated": true, "len": items.len() });
        }
        Value::Object(map) if encoded_len(map) > CONSOLE_FIELD_MAX_CHARS * 8 => {
            let keys: Vec<String> = map.keys().cloned().collect();
            *value = json!({ "_truncated": true, "keys": keys });
        }
        Value::Array(items) => items.iter_mut().for_each(slim_json),
        Value::Object(map) => map.values_mut().for_each(slim_json),
        _ => {}
    }
}

fn truncate_string(text: &mut String, max_chars: usize) {
    if text.chars().count() <= max_chars {
        return;
    }
    *text = text.chars().take(max_chars.saturating_sub(1)).collect();
    text.push('…');
}

fn encoded_len(value: &impl serde::Serialize) -> usize {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .unwrap_or(0)
}

fn encode_event_get(event: AgentEventEnvelope) -> Result<Value, ExtensionError> {
    let value = serde_json::to_value(&event).map_err(encode_error)?;
    if encoded_len(&value) > CONSOLE_EVENT_GET_BUDGET_BYTES {
        return Err(ExtensionError::Registration(
            json!({
                "code": "agent.session.event_too_large",
                "message": format!(
                    "event {} exceeds console payload budget",
                    event.sequence
                ),
                "retryable": false
            })
            .to_string(),
        ));
    }
    Ok(value)
}

fn decode<T: serde::de::DeserializeOwned>(params: &Value, key: &str) -> Result<T, ExtensionError> {
    serde_json::from_value(
        params
            .get(key)
            .cloned()
            .ok_or_else(|| ExtensionError::Registration(format!("missing {key}")))?,
    )
    .map_err(|error| ExtensionError::Registration(error.to_string()))
}

fn required_str(params: &Value, key: &str) -> Result<String, ExtensionError> {
    let value = params
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ExtensionError::Registration(format!("missing {key}")))?;
    Ok(value.to_owned())
}

fn required_u64(params: &Value, key: &str) -> Result<u64, ExtensionError> {
    params
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| ExtensionError::Registration(format!("missing {key}")))
}

fn agent_error(error: AgentManagementError) -> ExtensionError {
    ExtensionError::Registration(json!({"code": error.code, "message": error.message}).to_string())
}

fn wire_error(error: mutsuki_agent_contracts::AgentWireError) -> ExtensionError {
    ExtensionError::Registration(
        json!({"code": error.code, "message": error.message, "retryable": error.retryable})
            .to_string(),
    )
}

fn resolve_sessions(
    resolver: &LocalAgentManagementResolver,
) -> Result<Arc<dyn AgentSessionManagementApi>, ExtensionError> {
    resolver().map_err(|message| {
        ExtensionError::Registration(
            json!({"code": "agent.owner_unavailable", "message": message}).to_string(),
        )
    })
}

fn resolve_connections(
    resolver: &AgentConnectionManagementResolver,
) -> Result<Arc<dyn AgentConnectionManagementApi>, ExtensionError> {
    resolver().map_err(|message| {
        ExtensionError::Registration(
            json!({"code": "agent.connection_owner_unavailable", "message": message}).to_string(),
        )
    })
}

fn encode_error(error: serde_json::Error) -> ExtensionError {
    ExtensionError::Registration(format!("response encoding failed: {error}"))
}

fn join_error(error: tokio::task::JoinError) -> ExtensionError {
    ExtensionError::Registration(format!("management task failed: {error}"))
}

fn manifest(assets: Vec<AssetEntry>) -> ExtensionManifest {
    ExtensionManifest {
        manifest_version: EXTENSION_MANIFEST_VERSION,
        id: PLUGIN_ID.into(),
        version: PLUGIN_VERSION.into(),
        entry: "index.js".into(),
        capabilities: vec![
            CAPABILITY_CONNECTION_READ.into(),
            CAPABILITY_CONNECTION_WRITE.into(),
            CAPABILITY_SESSION_READ.into(),
            CAPABILITY_SESSION_WRITE.into(),
        ],
        permissions: vec!["pages".into(), "navigation".into()],
        assets,
        protocol_version: WEB_PROTOCOL_VERSION.into(),
    }
}

/// Writes the shared trajectory projector/view used by Agent and Tasks pages.
pub fn materialize_trajectory_assets(out_dir: &Path) -> Result<(), std::io::Error> {
    std::fs::create_dir_all(out_dir)?;
    std::fs::write(
        out_dir.join("trajectory-model.js"),
        include_str!("../assets/trajectory-model.js"),
    )?;
    std::fs::write(
        out_dir.join("trajectory-view.js"),
        include_str!("../assets/trajectory-view.js"),
    )?;
    Ok(())
}

/// Writes the embedded management frontend and its content-addressed manifest.
pub fn materialize_frontend_assets(out_dir: &Path) -> Result<PathBuf, std::io::Error> {
    std::fs::create_dir_all(out_dir)?;
    materialize_trajectory_assets(out_dir)?;
    let js = include_str!("../assets/index.js");
    let model = include_str!("../assets/trajectory-model.js");
    let view = include_str!("../assets/trajectory-view.js");
    std::fs::write(out_dir.join("index.js"), js)?;
    let assets = [
        ("index.js", js.as_bytes()),
        ("trajectory-model.js", model.as_bytes()),
        ("trajectory-view.js", view.as_bytes()),
    ]
    .into_iter()
    .map(|(path, bytes)| AssetEntry {
        path: path.into(),
        content_hash: content_hash(bytes),
        bytes: bytes.len() as u64,
    })
    .collect();
    let encoded = serde_json::to_vec_pretty(&manifest(assets)).map_err(std::io::Error::other)?;
    std::fs::write(out_dir.join("manifest.json"), encoded)?;
    Ok(out_dir.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_agent_contracts::AgentEventMeta;
    use mutsuki_web_extension_api::RpcCallContext;

    #[test]
    fn trajectory_model_projects_agent_and_runtime_events() {
        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets/trajectory-model.test.mjs");
        let output = std::process::Command::new("node")
            .arg(&script)
            .output()
            .expect("node must be available to verify trajectory projection");
        assert!(
            output.status.success(),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn assets_are_materialized() {
        let root = tempfile::tempdir().unwrap();
        materialize_frontend_assets(root.path()).unwrap();
        assert!(root.path().join("index.js").is_file());
        assert!(root.path().join("trajectory-model.js").is_file());
        assert!(root.path().join("trajectory-view.js").is_file());
        assert!(root.path().join("manifest.json").is_file());
        let index_js = include_str!("../assets/index.js");
        assert!(index_js.contains("sessions.event_get"));
        assert!(!index_js.contains("测试模型"));
        assert!(!index_js.contains("provider.test"));
        assert!(include_str!("../assets/trajectory-model.js").contains("EVENT_WINDOW_LIMIT"));
    }

    #[test]
    fn rpc_authorization_uses_authenticated_context_not_request_fields() {
        let extension = BotAgentWebExtension::new(None)
            .with_connection_resolver(Some(Arc::new(|| Err("not running".into()))));
        let mut rpc = RpcRegistry::new(PLUGIN_ID);
        extension.register_rpc(&mut rpc).unwrap();

        assert!(matches!(
            rpc.call_with_context(
                "connections.snapshot",
                json!({ "capabilities": ["*"] }),
                RpcCallContext::default(),
            ),
            Err(ExtensionError::CapabilityDenied(capability))
                if capability == CAPABILITY_CONNECTION_READ
        ));
        assert!(matches!(
            rpc.call_with_context(
                "connections.snapshot",
                json!({}),
                RpcCallContext::new(&[CAPABILITY_CONNECTION_READ.into()]),
            ),
            Err(ExtensionError::Registration(message))
                if message.contains("agent.connection_owner_unavailable")
        ));
    }

    #[test]
    fn console_get_drops_events_and_keeps_message_tail() {
        let mut session = dummy_session();
        session.events.push(dummy_envelope(1, "alpha"));
        session.messages = (0..25)
            .map(|index| mutsuki_agent_contracts::AgentMessage::user(format!("m{index}")))
            .collect();
        slim_session_for_console(&mut session);
        assert!(session.events.is_empty());
        assert_eq!(session.messages.len(), 20);
        assert_eq!(session.messages[0].content, "m5");
        assert_eq!(session.messages[19].content, "m24");
    }

    #[test]
    fn console_event_page_prefers_newest_suffix_under_budget() {
        let events = (1..=80)
            .map(|sequence| dummy_envelope(sequence, &"x".repeat(2_000)))
            .collect();
        let page = pack_event_page(events);
        assert!(!page.events.is_empty());
        assert!(encoded_len(&page) <= CONSOLE_EVENT_GET_BUDGET_BYTES);
        assert_eq!(
            page.events.last().map(|event| event.sequence),
            Some(80),
            "page must keep the newest fetched events"
        );
        if page.truncated {
            assert!(page.events.len() < 80);
        }
        match &page.events[0].event {
            AgentEvent::ModelDelta { text, .. } => {
                assert!(text.chars().count() <= CONSOLE_FIELD_MAX_CHARS);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn console_event_get_rejects_oversized_payload() {
        let event = dummy_envelope(3, &"z".repeat(CONSOLE_EVENT_GET_BUDGET_BYTES));
        let error = encode_event_get(event).unwrap_err();
        let ExtensionError::Registration(message) = error else {
            panic!("expected registration error");
        };
        assert!(message.contains("agent.session.event_too_large"));
    }

    #[test]
    fn session_event_rpcs_require_read_capability() {
        let extension = BotAgentWebExtension::new(None).with_sessions(Some(Arc::new(|| {
            Ok(Arc::new(FakeSessions::default()) as Arc<dyn AgentSessionManagementApi>)
        })));
        let mut rpc = RpcRegistry::new(PLUGIN_ID);
        extension.register_rpc(&mut rpc).unwrap();
        assert!(matches!(
            rpc.call_with_context("sessions.event_get", json!({"session_id":"s1","sequence":1}), RpcCallContext::default()),
            Err(ExtensionError::CapabilityDenied(capability)) if capability == CAPABILITY_SESSION_READ
        ));
        let value = rpc
            .call_with_context(
                "sessions.event_get",
                json!({"session_id":"s1","sequence":1}),
                RpcCallContext::new(&[CAPABILITY_SESSION_READ.into()]),
            )
            .unwrap();
        assert_eq!(value["sequence"], 1);
        let session = rpc
            .call_with_context(
                "sessions.get",
                json!({"session_id":"s1"}),
                RpcCallContext::new(&[CAPABILITY_SESSION_READ.into()]),
            )
            .unwrap();
        assert!(session["events"].as_array().unwrap().is_empty());
        let page = rpc
            .call_with_context(
                "sessions.events",
                json!({"session_id":"s1","after_sequence":0,"limit":10}),
                RpcCallContext::new(&[CAPABILITY_SESSION_READ.into()]),
            )
            .unwrap();
        assert!(page.get("events").is_some());
        assert!(page.get("truncated").is_some());
    }

    fn dummy_envelope(sequence: u64, text: &str) -> AgentEventEnvelope {
        AgentEventEnvelope {
            session_id: "s1".into(),
            sequence,
            meta: AgentEventMeta::default(),
            event: AgentEvent::ModelDelta {
                turn_id: "t1".into(),
                text: text.into(),
            },
        }
    }

    fn dummy_session() -> AgentSession {
        serde_json::from_value(json!({
            "session_id": "s1",
            "profile_id": "p",
            "messages": [],
            "turn_count": 0,
            "events": [],
            "next_event_sequence": 1,
            "resource": {
                "ref_id": "ref-1",
                "resource_id": {"kind_id":"k","slot_id":"s","generation":0,"version":0},
                "semantic": "frozen_value",
                "provider_id": "p",
                "resource_kind": "k",
                "schema": "s",
                "version": 0,
                "generation": 0,
                "access": {"type":"inline"},
                "size_hint": null,
                "content_hash": null,
                "lifetime": "persistent",
                "lease": null,
                "seal_state": "writable"
            },
            "cell": {
                "cell_id": "c1",
                "resource_kind": "k",
                "owner_plugin_id": "p",
                "schema": "s",
                "generation": 0,
                "health": "ok",
                "reload_policy": "none"
            }
        }))
        .expect("dummy session")
    }

    #[derive(Default)]
    struct FakeSessions;

    impl AgentSessionManagementApi for FakeSessions {
        fn test_provider(&self) -> Result<(), mutsuki_agent_contracts::AgentWireError> {
            Ok(())
        }

        fn list_sessions(
            &self,
            _after_session_id: Option<&str>,
            _limit: usize,
        ) -> Result<
            mutsuki_agent_contracts::AgentSessionPage,
            mutsuki_agent_contracts::AgentWireError,
        > {
            Err(unavailable())
        }

        fn session(
            &self,
            session_id: &str,
        ) -> Result<AgentSession, mutsuki_agent_contracts::AgentWireError> {
            let mut session = dummy_session();
            session.session_id = session_id.into();
            session.events.push(dummy_envelope(1, "kept-out-of-get"));
            session
                .messages
                .push(mutsuki_agent_contracts::AgentMessage::user("hello"));
            Ok(session)
        }

        fn session_state(
            &self,
            _session_id: &str,
        ) -> Result<
            mutsuki_agent_contracts::AgentSessionState,
            mutsuki_agent_contracts::AgentWireError,
        > {
            Err(unavailable())
        }

        fn events_after(
            &self,
            _session_id: &str,
            after_sequence: u64,
            limit: u32,
        ) -> Result<Vec<AgentEventEnvelope>, mutsuki_agent_contracts::AgentWireError> {
            Ok((1..=3)
                .filter(|sequence| *sequence > after_sequence)
                .take(limit as usize)
                .map(|sequence| dummy_envelope(sequence, "delta"))
                .collect())
        }

        fn approve(
            &self,
            _decision: PermissionDecision,
        ) -> Result<SessionVersion, mutsuki_agent_contracts::AgentWireError> {
            Err(unavailable())
        }

        fn reject(
            &self,
            _decision: PermissionDecision,
        ) -> Result<SessionVersion, mutsuki_agent_contracts::AgentWireError> {
            Err(unavailable())
        }

        fn resolve_interaction(
            &self,
            _resolution: InteractionResolution,
        ) -> Result<SessionVersion, mutsuki_agent_contracts::AgentWireError> {
            Err(unavailable())
        }

        fn cancel_turn(
            &self,
            _session_id: &str,
            _turn_id: &str,
            _expected_version: SessionVersion,
        ) -> Result<SessionVersion, mutsuki_agent_contracts::AgentWireError> {
            Err(unavailable())
        }

        fn close_session(
            &self,
            _session_id: &str,
            _expected_version: SessionVersion,
        ) -> Result<(), mutsuki_agent_contracts::AgentWireError> {
            Err(unavailable())
        }
    }

    fn unavailable() -> mutsuki_agent_contracts::AgentWireError {
        mutsuki_agent_contracts::AgentWireError {
            code: "agent.test.unavailable".into(),
            message: "unused".into(),
            retryable: false,
        }
    }
}
