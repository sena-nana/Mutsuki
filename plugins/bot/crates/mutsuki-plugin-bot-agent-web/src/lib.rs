//! Authenticated Web Console bridge for owner-managed Agent connections.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mutsuki_agent_contracts::{InteractionResolution, PermissionDecision, SessionVersion};
use mutsuki_agent_service_host_integration::{
    AgentConnectionConfig, AgentConnectionId, AgentConnectionManager, LocalAgentManagementService,
};
use mutsuki_web_extension::{
    ExtensionError, RpcRegistry, WebExtension, WebExtensionDescriptor, content_hash,
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
pub type AgentConnectionManagementResolver =
    Arc<dyn Fn() -> Result<Arc<AgentConnectionManager>, String> + Send + Sync>;
pub type LocalAgentManagementResolver =
    Arc<dyn Fn() -> Result<Arc<LocalAgentManagementService>, String> + Send + Sync>;

pub struct BotAgentWebExtension {
    connections: Option<AgentConnectionManagementResolver>,
    sessions: Option<LocalAgentManagementResolver>,
    assets_root: Option<PathBuf>,
}

impl BotAgentWebExtension {
    #[must_use]
    pub fn new(connections: Option<Arc<AgentConnectionManager>>) -> Self {
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
            manifest: load_manifest(root).ok()?,
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
                    let connection_id =
                        AgentConnectionId::new(required_str(&params, "connection_id")?)
                            .map_err(|error| ExtensionError::Registration(error.to_string()))?;
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
                serde_json::to_value(
                    resolve_sessions(&sessions)?
                        .session(&required_str(&params, "session_id")?)
                        .map_err(wire_error)?,
                )
                .map_err(encode_error)
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
                serde_json::to_value(
                    resolve_sessions(&sessions)?
                        .events_after(
                            &required_str(&params, "session_id")?,
                            params
                                .get("after_sequence")
                                .and_then(Value::as_u64)
                                .unwrap_or(0),
                            params.get("limit").and_then(Value::as_u64).unwrap_or(100) as u32,
                        )
                        .map_err(wire_error)?,
                )
                .map_err(encode_error)
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
        _registry: &mut mutsuki_web_extension::EventRegistry,
    ) -> Result<(), ExtensionError> {
        Ok(())
    }
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

fn agent_error(
    error: mutsuki_agent_service_host_integration::AgentConnectionError,
) -> ExtensionError {
    ExtensionError::Registration(
        json!({"code": error.code(), "message": error.to_string()}).to_string(),
    )
}

fn wire_error(error: mutsuki_agent_contracts::AgentWireError) -> ExtensionError {
    ExtensionError::Registration(
        json!({"code": error.code, "message": error.message, "retryable": error.retryable})
            .to_string(),
    )
}

fn resolve_sessions(
    resolver: &LocalAgentManagementResolver,
) -> Result<Arc<LocalAgentManagementService>, ExtensionError> {
    resolver().map_err(|message| {
        ExtensionError::Registration(
            json!({"code": "agent.owner_unavailable", "message": message}).to_string(),
        )
    })
}

fn resolve_connections(
    resolver: &AgentConnectionManagementResolver,
) -> Result<Arc<AgentConnectionManager>, ExtensionError> {
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

fn load_manifest(root: &Path) -> Result<ExtensionManifest, ExtensionError> {
    let path = root.join("manifest.json");
    if path.exists() {
        return serde_json::from_slice(
            &std::fs::read(path).map_err(|error| ExtensionError::Manifest(error.to_string()))?,
        )
        .map_err(|error| ExtensionError::Manifest(error.to_string()));
    }
    let bytes = std::fs::read(root.join("index.js"))
        .map_err(|error| ExtensionError::Manifest(error.to_string()))?;
    Ok(manifest(vec![AssetEntry {
        path: "index.js".into(),
        content_hash: content_hash(&bytes),
        bytes: bytes.len() as u64,
    }]))
}

/// Writes the embedded management frontend and its content-addressed manifest.
pub fn materialize_frontend_assets(out_dir: &Path) -> Result<PathBuf, std::io::Error> {
    std::fs::create_dir_all(out_dir)?;
    let js = include_str!("../assets/index.js");
    std::fs::write(out_dir.join("index.js"), js)?;
    let encoded = serde_json::to_vec_pretty(&manifest(vec![AssetEntry {
        path: "index.js".into(),
        content_hash: content_hash(js.as_bytes()),
        bytes: js.len() as u64,
    }]))
    .map_err(std::io::Error::other)?;
    std::fs::write(out_dir.join("manifest.json"), encoded)?;
    Ok(out_dir.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_web_extension::RpcCallContext;

    #[test]
    fn assets_are_materialized() {
        let root = tempfile::tempdir().unwrap();
        materialize_frontend_assets(root.path()).unwrap();
        assert!(root.path().join("index.js").is_file());
        assert!(root.path().join("manifest.json").is_file());
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
}
