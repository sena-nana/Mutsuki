//! Database WebExtension: browse the live Bot state SQLite catalog.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mutsuki_bot_state_db::{BotStateDbError, BotStateDbRepository};
use mutsuki_web_extension::{
    ExtensionError, RpcRegistry, WebExtension, WebExtensionDescriptor, content_hash,
};
use mutsuki_web_protocol::{
    AssetEntry, EXTENSION_MANIFEST_VERSION, ExtensionManifest, WEB_PROTOCOL_VERSION,
    WebFrontendAssets,
};
use serde_json::Value;

pub const PLUGIN_ID: &str = "database";
pub const PLUGIN_VERSION: &str = "0.1.0";
pub const CAPABILITY_RUNTIME_READ: &str = "runtime.read";

pub struct DatabaseWebExtension {
    repository: Option<Arc<BotStateDbRepository>>,
    assets_root: Option<PathBuf>,
}

impl DatabaseWebExtension {
    #[must_use]
    pub fn new(repository: Option<Arc<BotStateDbRepository>>) -> Self {
        Self {
            repository,
            assets_root: None,
        }
    }

    #[must_use]
    pub fn with_frontend_assets(mut self, root: impl Into<PathBuf>) -> Self {
        self.assets_root = Some(root.into());
        self
    }
}

impl WebExtension for DatabaseWebExtension {
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
        let repository = self.repository.clone();
        registry.register_contextual("snapshot", {
            let repository = repository.clone();
            move |context, _params| {
                context.require(CAPABILITY_RUNTIME_READ)?;
                match &repository {
                    Some(repository) => {
                        serde_json::to_value(repository.inspect_snapshot().map_err(map_db_error)?)
                            .map_err(|error| {
                                ExtensionError::Registration(format!(
                                    "snapshot encode failed: {error}"
                                ))
                            })
                    }
                    None => Ok(Value::Null),
                }
            }
        });
        registry.register_contextual("rows", move |context, params| {
            context.require(CAPABILITY_RUNTIME_READ)?;
            let repository = repository.as_ref().ok_or_else(unavailable)?;
            let table = params
                .get("table")
                .and_then(Value::as_str)
                .ok_or_else(|| ExtensionError::Registration("missing table".into()))?;
            let after = params.get("after").and_then(Value::as_str);
            let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(50) as u32;
            serde_json::to_value(
                repository
                    .inspect_rows(table, after, limit)
                    .map_err(map_db_error)?,
            )
            .map_err(|error| ExtensionError::Registration(format!("rows encode failed: {error}")))
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

/// Writes the embedded database frontend and its content-addressed manifest.
///
/// # Errors
///
/// Returns an I/O error when directory creation, manifest encoding, or asset writes fail.
pub fn materialize_frontend_assets(out_dir: &Path) -> Result<PathBuf, std::io::Error> {
    std::fs::create_dir_all(out_dir)?;
    let js = include_str!("../assets/index.js");
    std::fs::write(out_dir.join("index.js"), js)?;
    let assets = vec![AssetEntry {
        path: "index.js".into(),
        content_hash: content_hash(js.as_bytes()),
        bytes: js.len() as u64,
    }];
    let manifest = serde_json::to_vec_pretty(&manifest(assets)).map_err(std::io::Error::other)?;
    std::fs::write(out_dir.join("manifest.json"), manifest)?;
    Ok(out_dir.to_path_buf())
}

fn manifest(assets: Vec<AssetEntry>) -> ExtensionManifest {
    ExtensionManifest {
        manifest_version: EXTENSION_MANIFEST_VERSION,
        id: PLUGIN_ID.into(),
        version: PLUGIN_VERSION.into(),
        entry: "index.js".into(),
        capabilities: vec![CAPABILITY_RUNTIME_READ.into()],
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

fn unavailable() -> ExtensionError {
    ExtensionError::Rpc {
        code: "database.unavailable".into(),
        message: "当前没有已接入的数据库".into(),
    }
}

fn map_db_error(error: BotStateDbError) -> ExtensionError {
    ExtensionError::Rpc {
        code: if matches!(error, BotStateDbError::InvalidConfiguration(_)) {
            "database.invalid_request"
        } else {
            "database.inspect_failed"
        }
        .into(),
        message: error.to_string(),
    }
}
