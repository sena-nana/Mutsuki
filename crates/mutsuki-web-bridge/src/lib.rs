//! Management RPC / Event bridge.
//!
//! Frontends never receive raw auth tokens, Host IPC endpoints, or plugin process handles.

mod metrics;
mod session;

pub use metrics::{BridgeMetrics, BridgeMetricsSnapshot};
pub use session::{AuthPolicy, BridgeSession, SessionManager};

use std::sync::Arc;

use mutsuki_web_extension::ExtensionRegistry;
use mutsuki_web_protocol::{
    EventEnvelope, ProtocolError, ProtocolResult, ResourceBudgets, RpcErrorBody, RpcRequest,
    RpcResponse, SessionInfo, WEB_PROTOCOL_VERSION, WEB_PROTOCOL_VERSION_MAJOR, WireMessage,
};
use parking_lot::RwLock;
use uuid::Uuid;

/// Shared bridge state used by HTTP/WebSocket handlers.
#[derive(Clone)]
pub struct WebBridge {
    inner: Arc<WebBridgeInner>,
}

struct WebBridgeInner {
    sessions: SessionManager,
    extensions: RwLock<ExtensionRegistry>,
    budgets: ResourceBudgets,
    metrics: BridgeMetrics,
    safe_mode: RwLock<bool>,
    auth: AuthPolicy,
}

impl WebBridge {
    pub fn new(
        budgets: ResourceBudgets,
        extensions: ExtensionRegistry,
        auth: AuthPolicy,
        safe_mode: bool,
    ) -> Self {
        Self {
            inner: Arc::new(WebBridgeInner {
                sessions: SessionManager::new(budgets),
                extensions: RwLock::new(extensions),
                budgets,
                metrics: BridgeMetrics::default(),
                safe_mode: RwLock::new(safe_mode),
                auth,
            }),
        }
    }

    pub fn metrics(&self) -> BridgeMetricsSnapshot {
        self.inner.metrics.export()
    }

    pub fn set_extension_counts(&self, loaded: u64, failed: u64) {
        self.inner.metrics.set_extension_counts(loaded, failed);
    }

    pub fn set_connections(&self, value: u64) {
        self.inner.metrics.set_connections(value);
    }

    pub fn set_static_cache_bytes(&self, value: u64) {
        self.inner.metrics.set_static_cache_bytes(value);
    }

    pub fn set_ws_queue_depth(&self, value: u64) {
        self.inner.metrics.set_ws_queue_depth(value);
    }

    pub fn budgets(&self) -> ResourceBudgets {
        self.inner.budgets
    }

    pub fn set_safe_mode(&self, enabled: bool) {
        *self.inner.safe_mode.write() = enabled;
    }

    pub fn safe_mode(&self) -> bool {
        *self.inner.safe_mode.read()
    }

    pub fn extensions(&self) -> parking_lot::RwLockReadGuard<'_, ExtensionRegistry> {
        self.inner.extensions.read()
    }

    pub fn extensions_mut(&self) -> parking_lot::RwLockWriteGuard<'_, ExtensionRegistry> {
        self.inner.extensions.write()
    }

    pub fn active_sessions(&self) -> u64 {
        self.inner.sessions.active_count() as u64
    }

    pub fn handle_message(
        &self,
        session_id: Option<Uuid>,
        message: WireMessage,
    ) -> ProtocolResult<HandleOutcome> {
        let size = message.payload_size();
        if size > self.inner.budgets.max_payload_bytes {
            return Err(ProtocolError::PayloadTooLarge {
                limit: self.inner.budgets.max_payload_bytes,
                actual: size,
            });
        }

        match message {
            WireMessage::Hello {
                protocol_version,
                capabilities: _client_capabilities,
                auth_token,
            } => {
                ensure_protocol_compatible(&protocol_version)?;
                // Capabilities are server-granted only. Client Hello must not escalate.
                let mut session_caps = self.inner.auth.authenticate(auth_token.as_deref())?;
                if self.safe_mode() {
                    session_caps.retain(|cap| cap.starts_with("recovery.") || cap == "*");
                    if session_caps.is_empty() {
                        session_caps.push("recovery.read".into());
                        session_caps.push("recovery.write".into());
                    }
                }
                let session = self.inner.sessions.create(session_caps, self.safe_mode())?;
                self.inner.metrics.inc_sessions();
                Ok(HandleOutcome::Reply(WireMessage::HelloAck {
                    protocol_version: WEB_PROTOCOL_VERSION.to_string(),
                    session: SessionInfo {
                        session_id: session.session_id,
                        capabilities: session.capabilities.clone(),
                        safe_mode: session.safe_mode,
                    },
                }))
            }
            WireMessage::Rpc(request) => {
                let session = self.require_session(session_id)?;
                let response = self.dispatch_rpc(&session, request);
                Ok(HandleOutcome::Reply(WireMessage::RpcResult(response)))
            }
            WireMessage::Subscribe(sub) => {
                let session = self.require_session(session_id)?;
                if let Some(cap) = &sub.required_capability {
                    session.require_capability(cap)?;
                }
                self.inner
                    .sessions
                    .subscribe(session.session_id, sub.clone())?;
                self.inner.metrics.inc_subscriptions();
                Ok(HandleOutcome::Subscribed(sub.subscription_id))
            }
            WireMessage::Unsubscribe { subscription_id } => {
                let session = self.require_session(session_id)?;
                self.inner
                    .sessions
                    .unsubscribe(session.session_id, subscription_id);
                Ok(HandleOutcome::Unsubscribed(subscription_id))
            }
            WireMessage::Ping => Ok(HandleOutcome::Reply(WireMessage::Pong)),
            WireMessage::Pong
            | WireMessage::HelloAck { .. }
            | WireMessage::RpcResult(_)
            | WireMessage::Event(_)
            | WireMessage::Error { .. } => Err(ProtocolError::InvalidMessage(
                "message not accepted from client".into(),
            )),
        }
    }

    pub fn publish_event(
        &self,
        topic: &str,
        payload: mutsuki_web_protocol::JsonValue,
    ) -> ProtocolResult<u64> {
        let delivered = self
            .inner
            .sessions
            .fanout(topic, payload, &self.inner.metrics)?;
        Ok(delivered)
    }

    pub fn take_events(&self, session_id: Uuid) -> Vec<EventEnvelope> {
        self.inner.sessions.drain_events(session_id)
    }

    pub fn close_session(&self, session_id: Uuid) {
        self.inner.sessions.close(session_id);
        self.inner.metrics.dec_sessions();
    }

    fn require_session(&self, session_id: Option<Uuid>) -> ProtocolResult<BridgeSession> {
        let session_id = session_id.ok_or(ProtocolError::Unauthenticated)?;
        self.inner
            .sessions
            .get(session_id)
            .ok_or(ProtocolError::Unauthenticated)
    }

    fn dispatch_rpc(&self, session: &BridgeSession, request: RpcRequest) -> RpcResponse {
        self.inner.metrics.inc_rpc_inflight();
        let started = std::time::Instant::now();
        let response = if request.namespace == "host" {
            self.dispatch_host_rpc(session, &request)
        } else if request.namespace == "recovery" {
            self.dispatch_recovery_rpc(session, &request)
        } else {
            match self.inner.extensions.read().call_rpc(
                &request.namespace,
                &request.method,
                inject_session_capabilities(request.params.clone(), &session.capabilities),
                &session.capabilities,
            ) {
                Ok(result) => RpcResponse {
                    id: request.id,
                    result: Some(result),
                    error: None,
                },
                Err(err) => {
                    let code = if err.to_string().contains("capability denied") {
                        "capability_denied"
                    } else {
                        "rpc_failed"
                    };
                    rpc_error(request.id, code, err.to_string())
                }
            }
        };
        self.inner.metrics.dec_rpc_inflight();
        self.inner
            .metrics
            .observe_rpc_latency(started.elapsed().as_millis() as u64);
        response
    }

    fn dispatch_host_rpc(&self, session: &BridgeSession, request: &RpcRequest) -> RpcResponse {
        match request.method.as_str() {
            "status" => {
                if let Err(err) = session.require_capability("host.read") {
                    return rpc_error(request.id, "capability_denied", err.to_string());
                }
                let report = self.inner.extensions.read().report();
                RpcResponse {
                    id: request.id,
                    result: Some(serde_json::json!({
                        "protocol_version": WEB_PROTOCOL_VERSION,
                        "safe_mode": self.safe_mode(),
                        "extensions_loaded": report.loaded.len(),
                        "extensions_failed": report.failed.len(),
                        "active_sessions": self.active_sessions(),
                    })),
                    error: None,
                }
            }
            "metrics" => {
                if let Err(err) = session.require_capability("host.read") {
                    return rpc_error(request.id, "capability_denied", err.to_string());
                }
                let metrics = self.metrics();
                RpcResponse {
                    id: request.id,
                    result: Some(serde_json::to_value(metrics).unwrap_or_default()),
                    error: None,
                }
            }
            other => rpc_error(
                request.id,
                "method_not_found",
                format!("unknown host method: {other}"),
            ),
        }
    }

    fn dispatch_recovery_rpc(&self, session: &BridgeSession, request: &RpcRequest) -> RpcResponse {
        match request.method.as_str() {
            "list_failures" => {
                if let Err(err) = session.require_capability("recovery.read") {
                    return rpc_error(request.id, "capability_denied", err.to_string());
                }
                let failures = self.inner.extensions.read().failures();
                RpcResponse {
                    id: request.id,
                    result: Some(serde_json::to_value(failures).unwrap_or_default()),
                    error: None,
                }
            }
            "disable_extension" => {
                if let Err(err) = session.require_capability("recovery.write") {
                    return rpc_error(request.id, "capability_denied", err.to_string());
                }
                let extension_id = request
                    .params
                    .get("extension_id")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default();
                let disabled = self.inner.extensions.write().disable(extension_id);
                RpcResponse {
                    id: request.id,
                    result: Some(serde_json::json!({ "disabled": disabled })),
                    error: None,
                }
            }
            "enter_safe_mode" => {
                if let Err(err) = session.require_capability("recovery.write") {
                    return rpc_error(request.id, "capability_denied", err.to_string());
                }
                self.set_safe_mode(true);
                RpcResponse {
                    id: request.id,
                    result: Some(serde_json::json!({ "safe_mode": true })),
                    error: None,
                }
            }
            "clear_extension_cache" => {
                if let Err(err) = session.require_capability("recovery.write") {
                    return rpc_error(request.id, "capability_denied", err.to_string());
                }
                let extension_id = request
                    .params
                    .get("extension_id")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default();
                let unloaded = self.inner.extensions.write().unload(extension_id);
                RpcResponse {
                    id: request.id,
                    result: Some(serde_json::json!({ "cleared": unloaded })),
                    error: None,
                }
            }
            other => rpc_error(
                request.id,
                "method_not_found",
                format!("unknown recovery method: {other}"),
            ),
        }
    }
}

#[derive(Debug)]
pub enum HandleOutcome {
    Reply(WireMessage),
    Subscribed(Uuid),
    Unsubscribed(Uuid),
}

fn ensure_protocol_compatible(client: &str) -> ProtocolResult<()> {
    let major = client
        .split('.')
        .next()
        .and_then(|part| part.parse::<u32>().ok())
        .ok_or_else(|| ProtocolError::VersionMismatch {
            client: client.to_string(),
            host: WEB_PROTOCOL_VERSION.to_string(),
        })?;
    if major != WEB_PROTOCOL_VERSION_MAJOR {
        return Err(ProtocolError::VersionMismatch {
            client: client.to_string(),
            host: WEB_PROTOCOL_VERSION.to_string(),
        });
    }
    Ok(())
}

fn rpc_error(id: Uuid, code: &str, message: String) -> RpcResponse {
    // Never echo tokens or IPC endpoints in error bodies.
    let sanitized = message
        .replace("token=", "token=<redacted>")
        .replace("ipc://", "ipc://<redacted>");
    RpcResponse {
        id,
        result: None,
        error: Some(RpcErrorBody {
            code: code.into(),
            message: sanitized,
        }),
    }
}

/// Derive effective RPC capabilities: session ∩ client request (never escalate).
fn inject_session_capabilities(
    params: mutsuki_web_protocol::JsonValue,
    session_capabilities: &[String],
) -> mutsuki_web_protocol::JsonValue {
    let client_caps = match &params {
        serde_json::Value::Object(map) => map
            .get("capabilities")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let effective = effective_capabilities(session_capabilities, &client_caps);
    let caps = serde_json::Value::Array(
        effective
            .into_iter()
            .map(serde_json::Value::String)
            .collect(),
    );
    match params {
        serde_json::Value::Object(mut map) => {
            map.insert("capabilities".into(), caps);
            serde_json::Value::Object(map)
        }
        serde_json::Value::Null => serde_json::json!({ "capabilities": caps }),
        other => serde_json::json!({
            "capabilities": caps,
            "value": other,
        }),
    }
}

fn effective_capabilities(session: &[String], client: &[String]) -> Vec<String> {
    let session_unrestricted = session.iter().any(|cap| cap == "*");
    if client.is_empty() {
        return session.to_vec();
    }
    if client.iter().any(|cap| cap == "*") {
        return session.to_vec();
    }
    if session_unrestricted {
        return client.to_vec();
    }
    client
        .iter()
        .filter(|cap| session.iter().any(|owned| owned == *cap))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_web_extension::ExtensionRegistry;
    use mutsuki_web_protocol::DEFAULT_BUDGETS;

    #[test]
    fn capability_is_enforced_server_side() {
        let bridge = WebBridge::new(
            DEFAULT_BUDGETS,
            ExtensionRegistry::new(DEFAULT_BUDGETS),
            AuthPolicy::allow_local(vec!["host.read".into()]),
            false,
        );
        let hello = bridge
            .handle_message(
                None,
                WireMessage::Hello {
                    protocol_version: WEB_PROTOCOL_VERSION.into(),
                    capabilities: vec![],
                    auth_token: Some("local-dev".into()),
                },
            )
            .unwrap();
        let session_id = match hello {
            HandleOutcome::Reply(WireMessage::HelloAck { session, .. }) => session.session_id,
            _ => panic!("expected hello ack"),
        };
        let response = bridge
            .handle_message(
                Some(session_id),
                WireMessage::Rpc(RpcRequest {
                    id: Uuid::new_v4(),
                    namespace: "recovery".into(),
                    method: "enter_safe_mode".into(),
                    params: serde_json::json!({}),
                }),
            )
            .unwrap();
        match response {
            HandleOutcome::Reply(WireMessage::RpcResult(result)) => {
                assert!(result.error.is_some());
                assert_eq!(result.error.unwrap().code, "capability_denied");
            }
            _ => panic!("expected rpc result"),
        }
    }

    #[test]
    fn hello_ignores_client_capability_escalation() {
        let bridge = WebBridge::new(
            DEFAULT_BUDGETS,
            ExtensionRegistry::new(DEFAULT_BUDGETS),
            AuthPolicy::allow_local(vec!["host.read".into()]),
            false,
        );
        let hello = bridge
            .handle_message(
                None,
                WireMessage::Hello {
                    protocol_version: WEB_PROTOCOL_VERSION.into(),
                    capabilities: vec!["*".into(), "runtime.write".into()],
                    auth_token: Some("local-dev".into()),
                },
            )
            .unwrap();
        match hello {
            HandleOutcome::Reply(WireMessage::HelloAck { session, .. }) => {
                assert_eq!(session.capabilities, vec!["host.read".to_string()]);
                assert!(!session.capabilities.iter().any(|cap| cap == "*"));
            }
            _ => panic!("expected hello ack"),
        }
    }

    #[test]
    fn rpc_capabilities_are_intersected_with_session() {
        assert_eq!(
            effective_capabilities(
                &["runtime.read".into(), "runtime.write".into()],
                &["runtime.read".into()]
            ),
            vec!["runtime.read".to_string()]
        );
        assert_eq!(
            effective_capabilities(&["runtime.read".into()], &["runtime.write".into()]),
            Vec::<String>::new()
        );
        assert_eq!(
            effective_capabilities(&["runtime.read".into()], &["*".into()]),
            vec!["runtime.read".to_string()]
        );
    }

    #[test]
    fn rejects_oversized_payload() {
        let mut budgets = DEFAULT_BUDGETS;
        budgets.max_payload_bytes = 32;
        let bridge = WebBridge::new(
            budgets,
            ExtensionRegistry::new(budgets),
            AuthPolicy::open_local(),
            false,
        );
        let huge = "x".repeat(64);
        let err = bridge
            .handle_message(
                None,
                WireMessage::Hello {
                    protocol_version: WEB_PROTOCOL_VERSION.into(),
                    capabilities: vec![huge],
                    auth_token: None,
                },
            )
            .unwrap_err();
        assert!(matches!(err, ProtocolError::PayloadTooLarge { .. }));
    }
}
