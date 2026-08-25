//! Management RPC / Event bridge.
//!
//! Frontends never receive raw auth tokens, Host IPC endpoints, or plugin process handles.
// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::cast_possible_truncation,
    clippy::map_unwrap_or,
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::needless_pass_by_value,
    clippy::return_self_not_must_use
)]

mod metrics;
mod session;

pub use metrics::{BridgeMetrics, BridgeMetricsSnapshot};
pub use session::{AuthGrant, AuthPolicy, BridgeSession, SessionManager};

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
                let mut grant = self.inner.auth.authenticate(auth_token.as_deref())?;
                if self.safe_mode() {
                    grant
                        .capabilities
                        .retain(|cap| cap.starts_with("recovery.") || cap == "*");
                    if grant.capabilities.is_empty() {
                        grant.capabilities.push("recovery.read".into());
                        grant.capabilities.push("recovery.write".into());
                    }
                }
                let session = self.inner.sessions.create_authenticated(
                    grant.principal_id,
                    grant.capabilities,
                    self.safe_mode(),
                )?;
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

    pub async fn handle_message_async(
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
        if let WireMessage::Rpc(request) = message {
            let session = self.require_session(session_id)?;
            let response = self.dispatch_rpc_async(&session, request).await;
            return Ok(HandleOutcome::Reply(WireMessage::RpcResult(response)));
        }
        self.handle_message(session_id, message)
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

    /// Returns the wake signal for events queued to one authenticated session.
    ///
    /// The signal is only a readiness hint. Callers must drain the bounded
    /// session queue after every wake and may also drain after inbound traffic
    /// to cover publish/receive races.
    pub fn session_event_notifier(&self, session_id: Uuid) -> Option<Arc<tokio::sync::Notify>> {
        self.inner.sessions.event_notifier(session_id)
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
            let rpc = self.inner.extensions.read().resolve_rpc(
                &request.namespace,
                &request.method,
                &session.capabilities,
            );
            match rpc.and_then(|rpc| {
                rpc.call_with_context(
                    &request.method,
                    request.params.clone(),
                    mutsuki_web_extension::RpcCallContext::authenticated(
                        &session.principal_id,
                        &session.capabilities,
                    ),
                )
            }) {
                Ok(result) => RpcResponse {
                    id: request.id,
                    result: Some(result),
                    error: None,
                },
                Err(err) => rpc_error(request.id, err.rpc_code(), err.to_string()),
            }
        };
        self.inner.metrics.dec_rpc_inflight();
        self.inner
            .metrics
            .observe_rpc_latency(started.elapsed().as_millis() as u64);
        response
    }

    async fn dispatch_rpc_async(
        &self,
        session: &BridgeSession,
        request: RpcRequest,
    ) -> RpcResponse {
        if request.namespace == "host" || request.namespace == "recovery" {
            return self.dispatch_rpc(session, request);
        }
        self.inner.metrics.inc_rpc_inflight();
        let started = std::time::Instant::now();
        let rpc = self.inner.extensions.read().resolve_rpc(
            &request.namespace,
            &request.method,
            &session.capabilities,
        );
        let response = match rpc {
            Ok(rpc) => rpc
                .call_async_with_context(
                    &request.method,
                    request.params.clone(),
                    mutsuki_web_extension::RpcCallContext::authenticated(
                        &session.principal_id,
                        &session.capabilities,
                    ),
                )
                .await
                .map(|result| RpcResponse {
                    id: request.id,
                    result: Some(result),
                    error: None,
                })
                .unwrap_or_else(|err| rpc_error(request.id, err.rpc_code(), err.to_string())),
            Err(err) => rpc_error(request.id, err.rpc_code(), err.to_string()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_web_extension::ExtensionRegistry;
    use mutsuki_web_protocol::{DEFAULT_BUDGETS, EventSubscription};
    use std::time::Duration;

    /// A policy with no accepted tokens must authenticate nobody. Treating the empty list as
    /// "accept anything" would hand that policy's capabilities to any caller who sends a token.
    #[test]
    fn empty_accepted_token_list_authenticates_nobody() {
        let policy = AuthPolicy::Local {
            accepted_tokens: vec![],
            default_capabilities: vec!["runtime.write".into()],
            allow_unauthenticated: false,
        };
        assert!(policy.authenticate(Some("anything")).is_err());
        assert!(policy.authenticate(Some("")).is_err());
        assert!(policy.authenticate(None).is_err());

        let remote = AuthPolicy::remote(vec![], true);
        assert!(remote.authenticate(Some("anything")).is_err());
    }

    /// `open_local` is the only policy that may answer an anonymous caller, and only with the
    /// read-only set. A token it never issued must not upgrade that caller.
    #[test]
    fn open_local_grants_read_only_anonymously_and_rejects_unknown_tokens() {
        let policy = AuthPolicy::open_local();
        let grant = policy
            .authenticate(None)
            .expect("anonymous read-only grant");
        assert_eq!(grant.capabilities, vec!["host.read", "recovery.read"]);
        assert!(policy.authenticate(Some("guessed-token")).is_err());
    }

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

    #[tokio::test]
    async fn event_publish_wakes_only_matching_session_and_preserves_order() {
        let bridge = WebBridge::new(
            DEFAULT_BUDGETS,
            ExtensionRegistry::new(DEFAULT_BUDGETS),
            AuthPolicy::allow_local(vec!["host.read".into()]),
            false,
        );
        let open_session = || match bridge
            .handle_message(
                None,
                WireMessage::Hello {
                    protocol_version: WEB_PROTOCOL_VERSION.into(),
                    capabilities: vec![],
                    auth_token: Some("local-dev".into()),
                },
            )
            .unwrap()
        {
            HandleOutcome::Reply(WireMessage::HelloAck { session, .. }) => session.session_id,
            _ => panic!("expected hello ack"),
        };
        let subscribed = open_session();
        let idle = open_session();
        let subscription_id = Uuid::new_v4();
        bridge
            .handle_message(
                Some(subscribed),
                WireMessage::Subscribe(EventSubscription {
                    subscription_id,
                    topic: "config.revision_changed".into(),
                    required_capability: None,
                }),
            )
            .unwrap();

        let subscribed_ready = bridge.session_event_notifier(subscribed).unwrap();
        let idle_ready = bridge.session_event_notifier(idle).unwrap();
        bridge
            .publish_event(
                "config.revision_changed",
                serde_json::json!({ "revision": 1 }),
            )
            .unwrap();
        bridge
            .publish_event(
                "config.revision_changed",
                serde_json::json!({ "revision": 2 }),
            )
            .unwrap();

        tokio::time::timeout(Duration::from_millis(100), subscribed_ready.notified())
            .await
            .expect("matching session must wake");
        assert!(
            tokio::time::timeout(Duration::from_millis(20), idle_ready.notified())
                .await
                .is_err(),
            "unmatched session must stay idle"
        );
        let events = bridge.take_events(subscribed);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].sequence, 1);
        assert_eq!(events[1].sequence, 2);

        bridge
            .handle_message(
                Some(subscribed),
                WireMessage::Unsubscribe { subscription_id },
            )
            .unwrap();
        bridge
            .publish_event(
                "config.revision_changed",
                serde_json::json!({ "revision": 3 }),
            )
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(20), subscribed_ready.notified())
                .await
                .is_err(),
            "unsubscribed session must stay idle"
        );
        assert!(bridge.take_events(subscribed).is_empty());
    }
}
