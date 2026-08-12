//! Authenticated Web Console bridge for owner-managed Agent connections.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mutsuki_agent_service_host_integration::{
    AgentConnectionConfig, AgentConnectionId, AgentConnectionManager,
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

pub struct BotAgentWebExtension {
    connections: Option<Arc<AgentConnectionManager>>,
    assets_root: Option<PathBuf>,
}

impl BotAgentWebExtension {
    #[must_use]
    pub fn new(connections: Option<Arc<AgentConnectionManager>>) -> Self {
        Self {
            connections,
            assets_root: None,
        }
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
            registry.register("connections.snapshot", move |params| {
                require_capability(&params, CAPABILITY_CONNECTION_READ)?;
                serde_json::to_value(manager.snapshot()).map_err(encode_error)
            });

            let manager = self.connections.as_ref().expect("checked").clone();
            registry.register("connections.test", move |params| {
                require_capability(&params, CAPABILITY_CONNECTION_WRITE)?;
                let config = decode::<AgentConnectionConfig>(&params, "config")?;
                serde_json::to_value(manager.test_connection(config).map_err(agent_error)?)
                    .map_err(encode_error)
            });

            let manager = self.connections.as_ref().expect("checked").clone();
            registry.register("connections.upsert", move |params| {
                require_capability(&params, CAPABILITY_CONNECTION_WRITE)?;
                let expected_revision = required_u64(&params, "expected_revision")?;
                let config = decode::<AgentConnectionConfig>(&params, "config")?;
                serde_json::to_value(
                    manager
                        .upsert(expected_revision, config)
                        .map_err(agent_error)?,
                )
                .map_err(encode_error)
            });

            let manager = self.connections.as_ref().expect("checked").clone();
            registry.register("connections.reconnect", move |params| {
                require_capability(&params, CAPABILITY_CONNECTION_WRITE)?;
                let expected_revision = required_u64(&params, "expected_revision")?;
                let connection_id = AgentConnectionId::new(required_str(&params, "connection_id")?)
                    .map_err(|error| ExtensionError::Registration(error.to_string()))?;
                serde_json::to_value(
                    manager
                        .reconnect(expected_revision, &connection_id)
                        .map_err(agent_error)?,
                )
                .map_err(encode_error)
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

fn has_capability(params: &Value, required: &str) -> bool {
    params
        .get("capabilities")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .any(|capability| capability == "*" || capability == required)
}

fn require_capability(params: &Value, required: &str) -> Result<(), ExtensionError> {
    has_capability(params, required)
        .then_some(())
        .ok_or_else(|| ExtensionError::CapabilityDenied(required.into()))
}

fn agent_error(
    error: mutsuki_agent_service_host_integration::AgentConnectionError,
) -> ExtensionError {
    ExtensionError::Registration(
        json!({"code": error.code(), "message": error.to_string()}).to_string(),
    )
}

fn encode_error(error: serde_json::Error) -> ExtensionError {
    ExtensionError::Registration(format!("response encoding failed: {error}"))
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

    #[test]
    fn assets_are_materialized() {
        let root = tempfile::tempdir().unwrap();
        materialize_frontend_assets(root.path()).unwrap();
        assert!(root.path().join("index.js").is_file());
        assert!(root.path().join("manifest.json").is_file());
    }
}
