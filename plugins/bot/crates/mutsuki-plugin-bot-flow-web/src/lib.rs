//! Authenticated WebExtension for the Bot-owned flow catalog and published graph lifecycle.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use mutsuki_bot_flow::{BotFlowError, BotFlowRegistry};
use mutsuki_bot_protocol::{BotFlowDocument, BotFlowDraftSaveRequest, BotFlowPublishRequest};
use mutsuki_web_extension::{
    ExtensionError, RpcRegistry, WebExtension, WebExtensionDescriptor, content_hash,
};
use mutsuki_web_protocol::{
    AssetEntry, EXTENSION_MANIFEST_VERSION, ExtensionManifest, WEB_PROTOCOL_VERSION,
    WebFrontendAssets,
};
use serde_json::{Value, json};

pub const PLUGIN_ID: &str = "bot-flow";
pub const PLUGIN_VERSION: &str = "0.1.0";
pub const CAPABILITY_FLOW_READ: &str = "bot.flow.read";
pub const CAPABILITY_FLOW_WRITE: &str = "bot.flow.write";
pub const CAPABILITY_FLOW_PUBLISH: &str = "bot.flow.publish";

pub struct BotFlowWebExtension {
    registry: Arc<BotFlowRegistry>,
    assets_root: Option<PathBuf>,
}

impl BotFlowWebExtension {
    #[must_use]
    pub fn new(registry: Arc<BotFlowRegistry>) -> Self {
        Self {
            registry,
            assets_root: None,
        }
    }

    #[must_use]
    pub fn with_frontend_assets(mut self, root: impl Into<PathBuf>) -> Self {
        self.assets_root = Some(root.into());
        self
    }
}

impl WebExtension for BotFlowWebExtension {
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
        let service = self.registry.clone();
        registry.register("catalog.read", move |params| {
            require_capability(&params, CAPABILITY_FLOW_READ)?;
            serde_json::to_value(service.catalog()).map_err(encode_error)
        });

        let service = self.registry.clone();
        registry.register("snapshot.read", move |params| {
            require_capability(&params, CAPABILITY_FLOW_READ)?;
            serde_json::to_value(service.snapshot().map_err(flow_error)?).map_err(encode_error)
        });

        let service = self.registry.clone();
        registry.register("draft.save", move |params| {
            require_capability(&params, CAPABILITY_FLOW_WRITE)?;
            let request = decode::<BotFlowDraftSaveRequest>(&params, "request")?;
            serde_json::to_value(service.save_draft(request, unix_ms()).map_err(flow_error)?)
                .map_err(encode_error)
        });

        let service = self.registry.clone();
        registry.register("draft.validate", move |params| {
            require_capability(&params, CAPABILITY_FLOW_WRITE)?;
            let flows = match params.get("flows") {
                Some(value) => serde_json::from_value::<Vec<BotFlowDocument>>(value.clone())
                    .map_err(|error| ExtensionError::Registration(error.to_string()))?,
                None => service
                    .snapshot()
                    .map_err(flow_error)?
                    .draft
                    .map_or_else(Vec::new, |draft| draft.flows),
            };
            serde_json::to_value(service.validate(&flows)).map_err(encode_error)
        });

        let service = self.registry.clone();
        registry.register("draft.discard", move |params| {
            require_capability(&params, CAPABILITY_FLOW_WRITE)?;
            let expected_revision = required_u64(&params, "expected_revision")?;
            service
                .discard_draft(expected_revision)
                .map_err(flow_error)?;
            Ok(json!({"discarded_revision": expected_revision}))
        });

        let service = self.registry.clone();
        registry.register("publish", move |params| {
            require_capability(&params, CAPABILITY_FLOW_PUBLISH)?;
            let request = decode::<BotFlowPublishRequest>(&params, "request")?;
            serde_json::to_value(
                service
                    .publish(request, unix_ms())
                    .map_err(flow_error)?
                    .as_ref(),
            )
            .map_err(encode_error)
        });
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

fn required_u64(params: &Value, key: &str) -> Result<u64, ExtensionError> {
    params
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| ExtensionError::Registration(format!("missing {key}")))
}

fn require_capability(params: &Value, required: &str) -> Result<(), ExtensionError> {
    params
        .get("capabilities")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .any(|capability| capability == "*" || capability == required)
        .then_some(())
        .ok_or_else(|| ExtensionError::CapabilityDenied(required.into()))
}

fn flow_error(error: BotFlowError) -> ExtensionError {
    let code = match error {
        BotFlowError::RevisionConflict { .. } => "bot.flow.revision_conflict",
        BotFlowError::Invalid(_) | BotFlowError::InvalidCatalog(_) => "bot.flow.invalid",
        BotFlowError::Repository(_) => "bot.flow.storage_failed",
    };
    ExtensionError::Registration(json!({"code": code, "message": error.to_string()}).to_string())
}

fn encode_error(error: serde_json::Error) -> ExtensionError {
    ExtensionError::Registration(format!("response encoding failed: {error}"))
}

fn unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(i64::MAX)
}

fn manifest(assets: Vec<AssetEntry>) -> ExtensionManifest {
    ExtensionManifest {
        manifest_version: EXTENSION_MANIFEST_VERSION,
        id: PLUGIN_ID.into(),
        version: PLUGIN_VERSION.into(),
        entry: "index.js".into(),
        capabilities: vec![
            CAPABILITY_FLOW_READ.into(),
            CAPABILITY_FLOW_WRITE.into(),
            CAPABILITY_FLOW_PUBLISH.into(),
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
    use mutsuki_bot_flow::{BotFlowRegistry, BotNodeCatalog, InMemoryBotFlowRepository};

    fn rpc() -> RpcRegistry {
        let registry = Arc::new(
            BotFlowRegistry::open(
                Arc::new(InMemoryBotFlowRepository::default()),
                BotNodeCatalog::default(),
            )
            .unwrap(),
        );
        let extension = BotFlowWebExtension::new(registry);
        let mut rpc = RpcRegistry::new(PLUGIN_ID);
        extension.register_rpc(&mut rpc).unwrap();
        rpc
    }

    #[test]
    fn rpc_surface_enforces_capabilities_and_revision_cas() {
        let rpc = rpc();
        assert_eq!(
            rpc.methods().len(),
            6,
            "only the controlled Flow RPC surface is registered"
        );
        assert!(rpc.call("catalog.read", json!({})).is_err());
        assert!(
            rpc.call(
                "draft.save",
                json!({
                    "capabilities": [CAPABILITY_FLOW_READ],
                    "request": {"expected_draft_revision": null, "base_published_revision": 0, "flows": []}
                }),
            )
            .is_err()
        );
        let draft = rpc
            .call(
                "draft.save",
                json!({
                    "capabilities": [CAPABILITY_FLOW_WRITE],
                    "request": {"expected_draft_revision": null, "base_published_revision": 0, "flows": []}
                }),
            )
            .unwrap();
        assert_eq!(draft["revision"], 1);
        let validation = rpc
            .call(
                "draft.validate",
                json!({"capabilities": [CAPABILITY_FLOW_WRITE]}),
            )
            .unwrap();
        assert_eq!(validation["valid"], true);
        let published = rpc
            .call(
                "publish",
                json!({
                    "capabilities": [CAPABILITY_FLOW_PUBLISH],
                    "request": {"expected_draft_revision": 1, "expected_published_revision": 0}
                }),
            )
            .unwrap();
        assert_eq!(published["revision"], 1);
        assert!(
            rpc.call(
                "publish",
                json!({
                    "capabilities": [CAPABILITY_FLOW_PUBLISH],
                    "request": {"expected_draft_revision": 1, "expected_published_revision": 0}
                }),
            )
            .unwrap_err()
            .to_string()
            .contains("revision_conflict")
        );
    }

    #[test]
    fn draft_discard_is_revision_fenced() {
        let rpc = rpc();
        rpc.call(
            "draft.save",
            json!({
                "capabilities": [CAPABILITY_FLOW_WRITE],
                "request": {"expected_draft_revision": null, "base_published_revision": 0, "flows": []}
            }),
        )
        .unwrap();
        assert!(
            rpc.call(
                "draft.discard",
                json!({"capabilities": [CAPABILITY_FLOW_WRITE], "expected_revision": 2}),
            )
            .is_err()
        );
        assert_eq!(
            rpc.call(
                "draft.discard",
                json!({"capabilities": [CAPABILITY_FLOW_WRITE], "expected_revision": 1}),
            )
            .unwrap()["discarded_revision"],
            1
        );
    }
}
