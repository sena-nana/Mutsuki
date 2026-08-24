//! Independently selected Bot Flow node editor WebExtension.
//!
//! The editor adapts Bot catalog/validation to `ConfigService`; it owns no
//! server-side draft or storage implementation.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mutsuki_bot_flow::{BOT_FLOW_CONFIG_PROVIDER_ID, BotFlowConfigProvider, BotFlowRegistry};
use mutsuki_bot_protocol::BotFlowDocument;
use mutsuki_config_service::{
    ConfigApplyRequest, ConfigContext, ConfigRevision, ConfigService, capability,
};
use mutsuki_web_extension::{
    ExtensionError, RpcRegistry, WebExtension, WebExtensionDescriptor, content_hash,
};
use mutsuki_web_protocol::{
    AssetEntry, EXTENSION_MANIFEST_VERSION, ExtensionManifest, WEB_PROTOCOL_VERSION,
    WebFrontendAssets,
};
use serde::Deserialize;
use serde_json::{Value, json};

pub const PLUGIN_ID: &str = "bot-flow-editor";
pub const PLUGIN_VERSION: &str = "0.2.0";
pub const CAPABILITY_FLOW_READ: &str = "bot.flow.read";
pub const CAPABILITY_FLOW_WRITE: &str = "bot.flow.write";

pub struct BotFlowEditorWebExtension {
    config: Arc<ConfigService>,
    registry: Arc<BotFlowRegistry>,
    assets_root: Option<PathBuf>,
}

impl BotFlowEditorWebExtension {
    #[must_use]
    pub fn new(config: Arc<ConfigService>, registry: Arc<BotFlowRegistry>) -> Self {
        Self {
            config,
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

impl WebExtension for BotFlowEditorWebExtension {
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

    fn register_rpc(&self, rpc: &mut RpcRegistry) -> Result<(), ExtensionError> {
        let registry = self.registry.clone();
        rpc.register_contextual("catalog.read", move |context, _params| {
            context.require(CAPABILITY_FLOW_READ)?;
            serde_json::to_value(registry.catalog()).map_err(encode_error)
        });

        let config = self.config.clone();
        rpc.register_async_contextual("snapshot.read", move |context, _params| {
            let config = config.clone();
            async move {
                context.require(CAPABILITY_FLOW_READ)?;
                config
                    .read(
                        BOT_FLOW_CONFIG_PROVIDER_ID,
                        ConfigContext::global(),
                        &[capability::VALUE_READ.into()],
                    )
                    .await
                    .and_then(|snapshot| {
                        BotFlowConfigProvider::decode(&snapshot.value).map(|flow| {
                            json!({
                                "revision": snapshot.revision,
                                "flow": flow,
                            })
                        })
                    })
                    .map_err(config_error)
            }
        });

        let registry = self.registry.clone();
        rpc.register_contextual("validate", move |context, params| {
            context.require(CAPABILITY_FLOW_WRITE)?;
            serde_json::to_value(registry.validate(&decode_flow(&params)?)).map_err(encode_error)
        });

        let config = self.config.clone();
        rpc.register_async_contextual("apply", move |context, params| {
            let config = config.clone();
            async move {
                context.require(CAPABILITY_FLOW_WRITE)?;
                let request: ApplyRequest = serde_json::from_value(params).map_err(decode_error)?;
                config
                    .apply(
                        BOT_FLOW_CONFIG_PROVIDER_ID,
                        ConfigApplyRequest {
                            candidate: BotFlowConfigProvider::encode(&request.flow)
                                .map_err(config_error)?,
                            expected_revision: ConfigRevision(request.expected_revision),
                            dry_run: false,
                        },
                        ConfigContext::global(),
                        &[capability::VALUE_WRITE.into(), capability::APPLY.into()],
                    )
                    .await
                    .and_then(|result| serde_json::to_value(result).map_err(json_config_error))
                    .map_err(config_error)
            }
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplyRequest {
    expected_revision: u64,
    flow: BotFlowDocument,
}

fn decode_flow(params: &Value) -> Result<BotFlowDocument, ExtensionError> {
    serde_json::from_value(
        params
            .get("flow")
            .cloned()
            .ok_or_else(|| ExtensionError::Registration("missing flow".into()))?,
    )
    .map_err(decode_error)
}

fn manifest(assets: Vec<AssetEntry>) -> ExtensionManifest {
    ExtensionManifest {
        manifest_version: EXTENSION_MANIFEST_VERSION,
        id: PLUGIN_ID.into(),
        version: PLUGIN_VERSION.into(),
        entry: "index.js".into(),
        capabilities: vec![CAPABILITY_FLOW_READ.into(), CAPABILITY_FLOW_WRITE.into()],
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
    let editor = include_str!("../assets/lilia-node-editor.js");
    let menu = include_str!("../assets/lilia-context-menu.js");
    std::fs::write(out_dir.join("index.js"), js)?;
    std::fs::write(out_dir.join("lilia-node-editor.js"), editor)?;
    std::fs::write(out_dir.join("lilia-context-menu.js"), menu)?;
    let assets = [
        ("index.js", js.as_bytes()),
        ("lilia-node-editor.js", editor.as_bytes()),
        ("lilia-context-menu.js", menu.as_bytes()),
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

fn config_error(error: mutsuki_config_service::ConfigError) -> ExtensionError {
    ExtensionError::Registration(error.to_string())
}

fn json_config_error(error: serde_json::Error) -> mutsuki_config_service::ConfigError {
    mutsuki_config_service::ConfigError::ApplyRejected {
        reason: error.to_string(),
    }
}

fn encode_error(error: serde_json::Error) -> ExtensionError {
    ExtensionError::Registration(error.to_string())
}

fn decode_error(error: serde_json::Error) -> ExtensionError {
    ExtensionError::Registration(format!("invalid request: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_bot_flow::{BotFlowConfigProvider, BotNodeCatalog};
    use mutsuki_config_service::{ConfigProviderRegistry, InMemoryConfigRepository};
    use mutsuki_web_extension::RpcCallContext;

    fn editor() -> (BotFlowEditorWebExtension, Arc<BotFlowRegistry>) {
        let flow = Arc::new(BotFlowRegistry::new(BotNodeCatalog::default()));
        let providers = Arc::new(ConfigProviderRegistry::default());
        providers
            .register(Arc::new(BotFlowConfigProvider::new(flow.clone())))
            .unwrap();
        let service = Arc::new(
            ConfigService::new(providers, Arc::new(InMemoryConfigRepository::default())).unwrap(),
        );
        (BotFlowEditorWebExtension::new(service, flow.clone()), flow)
    }

    #[test]
    fn rpc_surface_is_explicit_and_uses_authenticated_context() {
        let (editor, _) = editor();
        let mut rpc = RpcRegistry::new(PLUGIN_ID);
        editor.register_rpc(&mut rpc).unwrap();
        let mut methods = rpc.methods();
        methods.sort();
        assert_eq!(
            methods,
            [
                "bot-flow-editor.apply",
                "bot-flow-editor.catalog.read",
                "bot-flow-editor.snapshot.read",
                "bot-flow-editor.validate",
            ]
        );
        assert!(matches!(
            rpc.call_with_context(
                "catalog.read",
                json!({ "capabilities": ["*"] }),
                RpcCallContext::default(),
            ),
            Err(ExtensionError::CapabilityDenied(capability))
                if capability == CAPABILITY_FLOW_READ
        ));
        rpc.call_with_context(
            "catalog.read",
            json!({}),
            RpcCallContext::new(&[CAPABILITY_FLOW_READ.into()]),
        )
        .unwrap();
    }

    #[test]
    fn apply_is_one_revision_cas_and_conflict_keeps_active_snapshot() {
        let (editor, flow) = editor();
        let mut rpc = RpcRegistry::new(PLUGIN_ID);
        editor.register_rpc(&mut rpc).unwrap();
        let write = RpcCallContext::new(&[CAPABILITY_FLOW_WRITE.into()]);
        futures_executor::block_on(rpc.call_async_with_context(
            "apply",
            json!({ "expected_revision": 0, "flow": BotFlowDocument::default() }),
            write.clone(),
        ))
        .unwrap();
        assert_eq!(flow.active().revision, 1);

        let error = futures_executor::block_on(rpc.call_async_with_context(
            "apply",
            json!({ "expected_revision": 0, "flow": BotFlowDocument::default() }),
            write,
        ))
        .unwrap_err();
        assert!(error.to_string().contains("revision conflict"));
        assert_eq!(flow.active().revision, 1);
    }

    #[test]
    fn assets_include_lilia_node_editor() {
        let root = tempfile::tempdir().unwrap();
        materialize_frontend_assets(root.path()).unwrap();
        assert!(root.path().join("index.js").is_file());
        assert!(root.path().join("lilia-node-editor.js").is_file());
        assert!(root.path().join("lilia-context-menu.js").is_file());
        assert!(root.path().join("manifest.json").is_file());
        assert!(include_str!("../assets/index.js").contains("lilia-node-editor"));
        assert!(include_str!("../assets/index.js").contains("lilia-context-menu"));
        assert!(include_str!("../assets/lilia-context-menu.js").contains("openContextMenuAt"));
        assert!(include_str!("../assets/lilia-context-menu.js").contains("ctx-menu__submenu"));
        assert!(include_str!("../assets/lilia-node-editor.js").contains("mountLiliaNodeEditor"));
        assert!(include_str!("../assets/lilia-node-editor.js").contains("left:0;top:0"));
        assert!(!include_str!("../assets/index.js").contains("节点 ID"));
        assert!(!include_str!("../assets/index.js").contains("输入事件协议"));
        assert!(!include_str!("../assets/index.js").contains("新建流程"));
        assert!(!include_str!("../assets/index.js").contains("DRAFT_KEY"));
        assert!(!include_str!("../assets/index.js").contains("已发布"));
        assert!(include_str!("../assets/index.js").contains("保存"));
        assert!(include_str!("../assets/index.js").contains("已上线"));
        assert!(include_str!("../assets/lilia-node-editor.js").contains("contextmenu"));
        assert!(include_str!("../assets/index.js").contains("ctx-menu"));
        assert!(include_str!("../assets/index.js").contains("在空白处右键添加节点"));
        assert!(!include_str!("../assets/index.js").contains("flow-catalog"));
        assert!(!include_str!("../assets/index.js").contains("从左侧拖入"));
    }

    #[test]
    fn node_editor_geometry_and_port_types() {
        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets/lilia-node-editor.test.mjs");
        let output = std::process::Command::new("node")
            .arg(&script)
            .output()
            .expect("node must be available to verify the Lilia node editor");
        assert!(
            output.status.success(),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
